//! The attach/drain flow — the composition proper. Generic over any [`Tap`],
//! so the same flow serves a real cable and a simulated one. Every boundary
//! refuses before use.

use crate::capture::{Capture, RunMode};
use crate::decoder::{Ctrl, Decoder};
use crate::error::{HostError, Result};
use lucid_sld::enumerate::{enumerate, Enumeration};
use lucid_sld::instrument::{self as lin, Ident, RegionInfo};
use lucid_sld::node::Node;
use lucid_sld::tap::Tap;

/// Everything the read-only commands render from, gathered by one attach.
pub struct Attached {
    /// The hub walk (refused if the hub is not Altera).
    pub enumeration: Enumeration,
    /// The LIN instrument node.
    pub node: Node,
    /// The checked IDENT (refused on magic/proto mismatch).
    pub ident: Ident,
}

/// Attach to a live instrument: enumerate the hub (refuses a non-Altera
/// device, the silicon-witnessed 0x7FF case), attach to the instrument node
/// (refuses a hub with no instrument), and read+check IDENT (refuses a
/// proto/magic mismatch). Each refusal is a typed [`crate::error::HostError`].
pub fn attach<T: Tap>(tap: &mut T) -> Result<Attached> {
    let enumeration = enumerate(tap)?;
    let node = lin::attach(tap)?;
    let ident = lin::ident_checked(tap, &node)?;
    Ok(Attached {
        enumeration,
        node,
        ident,
    })
}

/// A region's descriptor.
pub fn region_info<T: Tap>(tap: &mut T, node: &Node, region: u8) -> Result<RegionInfo> {
    Ok(lin::region_info(tap, node, region)?)
}

/// Drain a region's full contents as 32-bit words.
///
/// Rounds the VDR count UP and truncates, so a region whose `word_count` is not
/// a multiple of `words_per_vdr` still drains every word — a stranger's
/// instrument need not have evenly-sized regions.
pub fn drain_region<T: Tap>(tap: &mut T, node: &Node, region: u8) -> Result<Vec<u32>> {
    let ri = lin::region_info(tap, node, region)?;
    let wpv = u32::from(ri.words_per_vdr.max(1));
    let vdrs = ri.word_count.div_ceil(wpv) as usize;
    let raw = lin::drain(tap, node, region, 0, vdrs)?;
    let mut words = lin::split64(&raw);
    words.truncate(ri.word_count as usize);
    Ok(words)
}

/// The instrument's raw STATUS chunks.
pub fn status_chunks<T: Tap>(tap: &mut T, node: &Node) -> Result<Vec<u64>> {
    Ok(lin::status_chunks(tap, node)?)
}

/// Apply a CTRL action through the instrument's decoder, writing its word(s) in
/// order. Returns the nonce to carry into the next write (so a caller doing
/// several writes keeps producing edges — CTRL is level-latched).
pub fn apply_ctrl<T: Tap>(
    tap: &mut T,
    node: &Node,
    decoder: &dyn Decoder,
    action: &Ctrl,
    nonce: bool,
) -> Result<bool> {
    let (words, next) = decoder.ctrl_words(action, nonce)?;
    for w in words {
        lin::ctrl_write(tap, node, w)?;
    }
    Ok(next)
}

/// ARM, then poll STATUS until the instrument reports itself armed — the
/// documented idiom, because CTRL crosses into the core clock through a
/// synchroniser and `armed` rises a few clocks after the latching write.
pub fn arm_and_wait<T: Tap>(
    tap: &mut T,
    node: &Node,
    decoder: &dyn Decoder,
    nonce: bool,
) -> Result<(u32, bool)> {
    let next = apply_ctrl(tap, node, decoder, &Ctrl::Arm, nonce)?;
    for polls in 1..=64u32 {
        let chunks = status_chunks(tap, node)?;
        if decoder.is_armed(&chunks) == Some(true) {
            return Ok((polls, next));
        }
    }
    Err(HostError::Transport(
        "the instrument never reported itself armed".into(),
    ))
}

/// Drain a live instrument into a capture container: the decoded header summary
/// and the data region as native records. Refuses an instrument whose decoder
/// exposes no capturable record region.
pub fn capture<T: Tap>(
    tap: &mut T,
    node: &Node,
    ident: &Ident,
    decoder: &dyn Decoder,
    run_mode: RunMode,
    seed: Option<u32>,
) -> Result<Capture> {
    let head = drain_region(tap, node, decoder.header_region())?;
    let header_summary = decoder.header_summary(&head);

    // Every non-header region contributes: ring events AND the SUMM located
    // exceptions land in ONE native payload (they share one self-describing
    // schema, tagged per record kind), and a summary/aggregate region also
    // yields its one-line digest for the container header. No region is
    // skipped, so a capture carries all the instrument publishes.
    let mut schema = None;
    let mut records: Vec<lucid_trace::RawRecord> = Vec::new();
    let mut summary = None;
    for r in 0..ident.region_count {
        if r == decoder.header_region() {
            continue;
        }
        let words = drain_region(tap, node, r)?;
        // pass the header so the decoder records only THIS capture's valid
        // events, never stale ring words a CLEAR left behind (the fix)
        if let Some((s, mut recs)) = decoder.to_records(r, &words, &head) {
            schema.get_or_insert(s);
            records.append(&mut recs);
        }
        if summary.is_none() {
            summary = decoder.summary(r, &words);
        }
    }
    let schema = schema.ok_or_else(|| {
        HostError::Refused("instrument exposes no capturable record region".into())
    })?;

    Ok(Capture::new(
        run_mode,
        ident.instrument_id,
        ident.instrument_version,
        ident.proto_version,
        ident.core_clock_hz,
        seed,
        header_summary,
        summary,
        schema,
        records,
    ))
}
