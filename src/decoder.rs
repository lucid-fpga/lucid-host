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
/// The O1 SUMM exception record tag — a located over-threshold gap. The
/// exception log is CHNK-shaped in the fabric precisely so it carries here as a
/// native record, re-decodable downstream with no O1-specific code (HGT6).
const O1_EXCEPTION_TAG: u8 = 2;

/// The O1 capture schema — defined once, here in the O1 decoder, so an O1
/// capture carries a self-describing record layout and a downstream consumer
/// decodes it with no O1-specific code. It describes both record kinds a capture
/// can hold: ring `event`s (96 payload bits) and SUMM `exception`s (128 bits).
fn o1_schema(core_clock_hz: u32) -> Schema {
    let mut schema = Schema {
        format_version: lucid_trace::schema::FORMAT_VERSION,
        schema_version: 1,
        schema_hash: 0,
        core_clock_hz,
        records: vec![
            RecordDef {
                tag: O1_EVENT_TAG,
                name: "event".into(),
                fields: vec![
                    FieldDef { name: "addr".into(), offset: 0, width: 32, encoding: Encoding::Hex },
                    FieldDef { name: "data".into(), offset: 32, width: 32, encoding: Encoding::Hex },
                    FieldDef { name: "kind".into(), offset: 64, width: 4, encoding: Encoding::Enum },
                    FieldDef { name: "flags".into(), offset: 68, width: 8, encoding: Encoding::U },
                    FieldDef { name: "seq".into(), offset: 76, width: 20, encoding: Encoding::U },
                ],
            },
            RecordDef {
                tag: O1_EXCEPTION_TAG,
                name: "exception".into(),
                fields: vec![
                    FieldDef { name: "gap".into(), offset: 0, width: 32, encoding: Encoding::U },
                    FieldDef { name: "addr".into(), offset: 32, width: 32, encoding: Encoding::Hex },
                    FieldDef { name: "write_ordinal".into(), offset: 64, width: 32, encoding: Encoding::U },
                    FieldDef { name: "seq".into(), offset: 96, width: 32, encoding: Encoding::U },
                ],
            },
        ],
    };
    schema.schema_hash = schema.computed_hash();
    schema
}

/// Pack an o1host `Event` into the 96-bit payload the event schema describes.
fn pack_o1_event(e: &o1host::Event) -> u128 {
    (e.addr as u128)
        | ((e.data as u128) << 32)
        | ((u128::from(e.kind) & 0xF) << 64)
        | ((u128::from(e.flags)) << 68)
        | ((u128::from(e.seq) & 0xF_FFFF) << 76)
}

/// Pack an o1host SUMM `Exception` into the 128-bit payload the exception schema
/// describes — the LOCATED stall: its gap, the byte offset it happened at, and
/// the write/seq ordinals that pin it to a point in the stream.
fn pack_o1_exception(e: &o1host::Exception) -> u128 {
    (e.gap as u128)
        | ((e.addr as u128) << 32)
        | ((e.write_ordinal as u128) << 64)
        | ((e.seq as u128) << 96)
}

/// The events that belong to THIS capture — `Header::valid_events`, authoritative
/// via the header's `event_count`/`write_index`, never the whole ring region.
///
/// This is the one seam where a capture's identity is decided. `CLEAR` resets the
/// recorder's counters but does NOT scrub the ring words, so decoding the whole
/// region (`Event::decode_all`) would re-count a previous capture's events — the
/// bench finding (fabric `event_count` said 1047, the region held 2119
/// non-empty words). Reading only `valid_events` is the fix, in one place. Returns
/// empty if the header will not decode.
fn valid_ring_events(ring_words: &[u32], header_words: &[u32]) -> Vec<o1host::Event> {
    match o1host::Header::decode(header_words) {
        Ok(h) => h.valid_events(ring_words),
        Err(_) => Vec::new(),
    }
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
    /// Set the summariser's exception threshold, in clk ticks: an inter-write
    /// gap at or above this is LOCATED in the exception log. Config, not capture
    /// state — the fabric latches it, so it survives CLEAR. Raising it above the
    /// recurring small stalls keeps the bounded exception log from overflowing,
    /// so the single largest gap is retained with its byte offset.
    Threshold(u32),
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
    /// Render a data region's words. `header_words` is the instrument's header
    /// region, so a decoder can render only THIS capture's valid events (never
    /// stale ring words a `CLEAR` left behind).
    fn render_region(&self, region: u8, words: &[u32], header_words: &[u32]) -> String;
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

    /// A one-line digest of a summary/aggregate region (e.g. O1's SUMM
    /// cadence) for the capture container's text header — the aggregate facts a
    /// reader wants at a glance, diffable field-by-field. The region's per-item
    /// detail (located exceptions) rides the payload as native records; this is
    /// the scalar overview. Default: none.
    fn summary(&self, _region: u8, _words: &[u32]) -> Option<String> {
        None
    }

    /// Convert a data region's words into NATIVE lucid-trace records plus the
    /// schema describing them — the capture container's payload (HGT6). This is
    /// where the instrument's event packing meets the suite's record format, so
    /// it lives in the instrument's decoder, never in the host. `header_words`
    /// is the header region, so only THIS capture's valid events are recorded —
    /// never stale ring words. `None` if the region has no record structure.
    fn to_records(
        &self,
        _region: u8,
        _words: &[u32],
        _header_words: &[u32],
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

    fn render_region(&self, region: u8, words: &[u32], header_words: &[u32]) -> String {
        if region == o1host::REGION_SUMM {
            // The summariser names its own malformation rather than rendering a
            // guess; a version-0 (reserved) block says so plainly.
            return match o1host::Summary::decode(words) {
                Ok(s) => render::o1_summary(&s),
                Err(e) => format!("[region {region}] SUMM undecoded: {e}"),
            };
        }
        if region != o1host::REGION_RING {
            return render::raw_region(region, words);
        }
        // only THIS capture's valid events — never stale post-CLEAR ring words
        let evs = valid_ring_events(words, header_words);
        let mut s = format!("{} events (valid_events, from the header)", evs.len());
        for e in &evs {
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
            // THRESH (bit 13) with the tick count in the payload (CTRL[63:32]).
            Ctrl::Threshold(ticks) => vec![c.build(&[cb::THRESH], *ticks)],
        };
        Ok((words, c.nonce))
    }

    fn to_records(
        &self,
        region: u8,
        words: &[u32],
        header_words: &[u32],
    ) -> Option<(Schema, Vec<RawRecord>)> {
        // core_clock_hz is not in the region words; the container header carries
        // the timestamp domain, and the schema's copy is filled by the writer.
        let schema = o1_schema(0);
        match region {
            r if r == o1host::REGION_RING => {
                // only THIS capture's valid events — the container never banks
                // stale ring words a CLEAR left behind (the fix, in one place).
                let raws = valid_ring_events(words, header_words)
                    .iter()
                    .map(|e| RawRecord {
                        tag: O1_EVENT_TAG,
                        timestamp: e.timestamp,
                        payload: pack_o1_event(e),
                        payload_bits: 96,
                    })
                    .collect();
                Some((schema, raws))
            }
            r if r == o1host::REGION_SUMM => {
                // The located over-threshold gaps. The exception log has no
                // absolute tick (it stores gap + seq), so timestamp is 0 — the
                // payload carries what LOCATES the stall (gap, addr, ordinal,
                // seq). A version-0 or malformed block yields no records.
                let s = o1host::Summary::decode(words).ok()?;
                if s.exceptions.is_empty() {
                    return None;
                }
                let raws = s
                    .exceptions
                    .iter()
                    .map(|e| RawRecord {
                        tag: O1_EXCEPTION_TAG,
                        timestamp: 0,
                        payload: pack_o1_exception(e),
                        payload_bits: 128,
                    })
                    .collect();
                Some((schema, raws))
            }
            _ => None,
        }
    }

    fn summary(&self, region: u8, words: &[u32]) -> Option<String> {
        if region != o1host::REGION_SUMM {
            return None;
        }
        o1host::Summary::decode(words).ok().map(|s| render::o1_summary_line(&s))
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
