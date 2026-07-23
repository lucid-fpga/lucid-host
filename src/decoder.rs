//! The decoder interface: an instrument's decoder lives in its own
//! authoritative crate (o1host for the O1 observatory), and the host composes
//! it through a trait keyed off IDENT's `instrument_id`. Adding a decoder
//! touches no host code beyond registration. An unknown instrument renders RAW
//! + UNDECODED — never a guess.

use crate::error::HostError;
use crate::render;

/// One instrument's decode+render, owned by whoever owns that instrument's
/// FIELD-MAP. The host holds `dyn Decoder`; it never re-implements a decode.
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
