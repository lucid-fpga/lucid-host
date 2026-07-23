//! The decoder interface: an instrument's decoder lives in its own
//! authoritative crate (o1host for the O1 observatory), and the host composes
//! it through a trait keyed off IDENT's `instrument_id`. Adding a decoder
//! touches no host code beyond registration. An unknown instrument renders RAW
//! + UNDECODED — never a guess.

use crate::error::HostError;
use crate::render;
use lucid_trace::schema::{Encoding, FieldDef, RecordDef};
use lucid_trace::{RawRecord, Schema};

/// The O1 event record tag inside the native lucid-trace payload (4-bit).
const O1_EVENT_TAG: u8 = 1;

/// The O1 event schema — defined once, here in the O1 decoder, so an O1 capture
/// carries a self-describing record layout and a downstream consumer decodes it
/// with no O1-specific code. Fields pack an o1host `Event` into 96 payload bits.
fn o1_event_schema(core_clock_hz: u32) -> Schema {
    let mut schema = Schema {
        format_version: lucid_trace::schema::FORMAT_VERSION,
        schema_version: 1,
        schema_hash: 0,
        core_clock_hz,
        records: vec![RecordDef {
            tag: O1_EVENT_TAG,
            name: "event".into(),
            fields: vec![
                FieldDef { name: "addr".into(), offset: 0, width: 32, encoding: Encoding::Hex },
                FieldDef { name: "data".into(), offset: 32, width: 32, encoding: Encoding::Hex },
                FieldDef { name: "kind".into(), offset: 64, width: 4, encoding: Encoding::Enum },
                FieldDef { name: "flags".into(), offset: 68, width: 8, encoding: Encoding::U },
                FieldDef { name: "seq".into(), offset: 76, width: 20, encoding: Encoding::U },
            ],
        }],
    };
    schema.schema_hash = schema.computed_hash();
    schema
}

/// Pack an o1host `Event` into the 96-bit payload the schema above describes.
fn pack_o1_event(e: &o1host::Event) -> u128 {
    (e.addr as u128)
        | ((e.data as u128) << 32)
        | ((u128::from(e.kind) & 0xF) << 64)
        | ((u128::from(e.flags)) << 68)
        | ((u128::from(e.seq) & 0xF_FFFF) << 76)
}

/// A ring overflow policy, in host-generic terms (the decoder maps it to the
/// instrument's own CTRL encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Refuse further events once full.
    Stop,
    /// Overwrite the oldest once full.
    Wrap,
}

/// A CTRL action a stranger can ask of any instrument in host-generic terms.
/// The decoder that owns the instrument translates it to that instrument's CTRL
/// word(s) — so the *encoding* stays in the instrument's authoritative crate,
/// never in the host (the same discipline as decode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ctrl {
    /// Begin recording.
    Arm,
    /// Stop recording.
    Disarm,
    /// Reset the ring, counters, seq, and sticky flags.
    Clear,
    /// Set the overflow policy.
    Policy(Policy),
    /// Set the event filter: a per-kind enable mask and an inclusive address
    /// range.
    Filter {
        /// The per-kind enable bitmask.
        kind_mask: u16,
        /// Inclusive low address bound.
        addr_lo: u32,
        /// Inclusive high address bound.
        addr_hi: u32,
    },
}

/// One instrument's decode+render+control, owned by whoever owns that
/// instrument's FIELD-MAP. The host holds `dyn Decoder`; it never re-implements
/// a decode or a CTRL encoding.
pub trait Decoder {
    /// The IDENT `instrument_id` this decoder claims.
    fn instrument_id(&self) -> u16;
    /// Human name for the render banner.
    fn name(&self) -> &'static str;
    /// Decode and render the header region's words. `Err(Decode)` names the
    /// malformation (HGT4) rather than rendering a guess.
    fn render_header(&self, head_words: &[u32]) -> Result<String, HostError>;
    /// Render a data region's words.
    fn render_region(&self, region: u8, words: &[u32]) -> String;
    /// Which region carries this instrument's header block.
    fn header_region(&self) -> u8;

    /// Render the instrument STATUS from its raw STATUS chunks. Default: the
    /// instrument publishes no decoded status.
    fn render_status(&self, _status_chunks: &[u64]) -> String {
        "status: (no decoder)".into()
    }

    /// Whether the recorder reports itself armed, from its STATUS chunks — the
    /// level the host polls after an ARM. `None` if the instrument has no such
    /// notion.
    fn is_armed(&self, _status_chunks: &[u64]) -> Option<bool> {
        None
    }

    /// The CTRL word(s) that effect `action` on this instrument, in write
    /// order, plus the nonce to carry into the next write. Consecutive words
    /// carry alternating nonces so each is an edge the fabric detects (CTRL is
    /// level-latched, edge-triggered). `Err(Refused)` if the instrument has no
    /// such control. Default: no control surface.
    fn ctrl_words(&self, _action: &Ctrl, _nonce: bool) -> Result<(Vec<u64>, bool), HostError> {
        Err(HostError::Refused(
            "this instrument exposes no CTRL surface".into(),
        ))
    }

    /// A one-line summary of the header for the capture container's text
    /// header (HGT2). Default: none.
    fn header_summary(&self, _head_words: &[u32]) -> Option<String> {
        None
    }

    /// Convert a data region's words into NATIVE lucid-trace records plus the
    /// schema describing them — the capture container's payload (HGT6). This is
    /// where the instrument's event packing meets the suite's record format, so
    /// it lives in the instrument's decoder, never in the host. `None` if the
    /// region has no record structure (e.g. a header region).
    fn to_records(
        &self,
        _region: u8,
        _words: &[u32],
    ) -> Option<(lucid_trace::Schema, Vec<lucid_trace::RawRecord>)> {
        None
    }
}

/// The O1 observatory decoder — thin over `o1host`, the one place O1 is
/// decoded. Registering it (rather than calling o1host inline) is the proof
/// the interface is real, not decoration (D3).
pub struct O1Decoder;

impl Decoder for O1Decoder {
    fn instrument_id(&self) -> u16 {
        0x0001
    }

    fn name(&self) -> &'static str {
        "O1 observatory"
    }

    fn header_region(&self) -> u8 {
        o1host::REGION_HEAD
    }

    fn render_header(&self, head_words: &[u32]) -> Result<String, HostError> {
        // Header::decode refuses without BOTH magics — a short or corrupt HEAD
        // is a decode fault naming itself, not a plausible-looking render.
        let h = o1host::Header::decode(head_words)
            .map_err(|e| HostError::Decode(format!("O1 HEAD: {e}")))?;
        Ok(render::o1_header(&h))
    }

    fn render_region(&self, region: u8, words: &[u32]) -> String {
        if region != o1host::REGION_RING {
            return render::raw_region(region, words);
        }
        let evs = o1host::Event::decode_all(words);
        let empty = evs.iter().filter(|e| e.is_empty()).count();
        let nonzero = words.iter().filter(|w| **w != 0).count();
        let mut s = format!(
            "{} events, {} empty, non-zero words {}",
            evs.len(),
            empty,
            nonzero
        );
        for e in evs.iter().filter(|e| !e.is_empty()) {
            s.push_str(&format!(
                "\n  t={:>10} {:<12} addr=0x{:08X} data=0x{:08X} seq={}",
                e.timestamp,
                o1host::kind_name(e.kind),
                e.addr,
                e.data,
                e.seq
            ));
        }
        s
    }

    fn header_summary(&self, head_words: &[u32]) -> Option<String> {
        o1host::Header::decode(head_words)
            .ok()
            .map(|h| render::o1_header(&h).replace('\n', " "))
    }

    fn render_status(&self, status_chunks: &[u64]) -> String {
        let Some(&chunk1) = status_chunks.get(1) else {
            return "status: short read".into();
        };
        let s = o1host::O1Status::decode(chunk1);
        format!(
            "armed={} overflowed={} wrapped_ever={} ring_full={} policy={} heartbeat={}",
            s.armed as u8,
            s.overflowed as u8,
            s.wrapped_ever as u8,
            s.ring_full as u8,
            if s.policy_wrap { "WRAP" } else { "STOP" },
            s.heartbeat
        )
    }

    fn is_armed(&self, status_chunks: &[u64]) -> Option<bool> {
        status_chunks
            .get(1)
            .map(|&c| o1host::O1Status::decode(c).armed)
    }

    fn ctrl_words(&self, action: &Ctrl, nonce: bool) -> Result<(Vec<u64>, bool), HostError> {
        use o1host::ctrl_bit as cb;
        let mut c = o1host::Ctrl { nonce };
        let words = match action {
            Ctrl::Arm => vec![c.build(&[cb::ARM], 0)],
            Ctrl::Disarm => vec![c.build(&[cb::DISARM], 0)],
            Ctrl::Clear => vec![c.build(&[cb::CLEAR], 0)],
            Ctrl::Policy(Policy::Stop) => vec![c.build(&[cb::POLICY_SET], 0)],
            Ctrl::Policy(Policy::Wrap) => vec![c.build(&[cb::POLICY_SET, cb::POLICY_VAL], 0)],
            Ctrl::Filter {
                kind_mask,
                addr_lo,
                addr_hi,
            } => vec![
                c.build(&[cb::FILT_MASK], u32::from(*kind_mask)),
                c.build(&[cb::FILT_LO], *addr_lo),
                c.build(&[cb::FILT_HI], *addr_hi),
            ],
        };
        Ok((words, c.nonce))
    }

    fn to_records(&self, region: u8, words: &[u32]) -> Option<(Schema, Vec<RawRecord>)> {
        if region != o1host::REGION_RING {
            return None;
        }
        // core_clock_hz is not in the ring words; the container header carries
        // the timestamp domain, and the schema's copy is filled by the writer.
        let schema = o1_event_schema(0);
        let raws = o1host::Event::decode_all(words)
            .iter()
            .filter(|e| !e.is_empty())
            .map(|e| RawRecord {
                tag: O1_EVENT_TAG,
                timestamp: e.timestamp,
                payload: pack_o1_event(e),
                payload_bits: 96,
            })
            .collect();
        Some((schema, raws))
    }
}

/// The decoder registry, keyed by `instrument_id`. Built with the suite's
/// known decoders; a stranger registers their own with [`Registry::register`].
pub struct Registry {
    decoders: Vec<Box<dyn Decoder>>,
}

impl Registry {
    /// The registry with the suite's built-in decoders (O1 today).
    pub fn with_builtins() -> Self {
        Registry {
            decoders: vec![Box::new(O1Decoder)],
        }
    }

    /// An empty registry — used to credential RAW rendering (no decoder
    /// registered → UNDECODED, and the bytes round-trip).
    pub fn empty() -> Self {
        Registry {
            decoders: Vec::new(),
        }
    }

    /// Register a decoder (the stranger's extension point, D3/HGT5).
    pub fn register(&mut self, decoder: Box<dyn Decoder>) {
        self.decoders.push(decoder);
    }

    /// The decoder for an instrument id, or `None` → the caller renders RAW.
    pub fn get(&self, instrument_id: u16) -> Option<&dyn Decoder> {
        self.decoders
            .iter()
            .map(|d| d.as_ref())
            .find(|d| d.instrument_id() == instrument_id)
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::with_builtins()
    }
}
