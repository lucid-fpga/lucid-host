//! The render surface (HGT1: one flow, every transport — the render is the
//! same whether the `Tap` is a cable or a sim). These functions reproduce the
//! line format `apf-host/examples/o1_desk.rs` prints, so the parity credential
//! (run from apf-host's side) can compare them byte-for-byte. The ONLY
//! permitted difference is provenance — see PARITY.md.
//!
//! Format is deliberately fixed here as the stable, line-oriented contract
//! (D7); a human-pretty layer may sit on top but must not change these lines.

use lucid_sld::enumerate::Enumeration;
use lucid_sld::instrument::{Ident, RegionInfo};

/// The `[hub]` line plus one `[node]` line per enumerated node.
pub fn enumeration(e: &Enumeration) -> String {
    let mut s = format!(
        "[hub]   N={} n={} m={} vir_len={}",
        e.nodes.len(),
        e.n,
        e.m,
        e.vir_len()
    );
    for n in &e.nodes {
        s.push_str(&format!(
            "\n[node]  addr={} type=0x{:02X} mfg=0x{:03X} inst={}",
            n.address, n.node_id, n.manufacturer, n.instance
        ));
    }
    s
}

/// The instrument magic, decoded little-endian into its four ASCII bytes.
pub fn magic_ascii(magic: u32) -> String {
    magic.to_le_bytes().iter().map(|b| *b as char).collect()
}

/// The two `[IDENT]` lines.
pub fn ident(id: &Ident) -> String {
    format!(
        "[IDENT] magic=\"{}\" proto={} instrument=0x{:04X} version={} clk={} Hz\n\
         [IDENT] regions={} caps=0b{:03b}",
        magic_ascii(id.magic),
        id.proto_version,
        id.instrument_id,
        id.instrument_version,
        id.core_clock_hz,
        id.region_count,
        id.caps
    )
}

/// One `[region N]` line.
pub fn region(index: u8, ri: &RegionInfo) -> String {
    format!(
        "[region {index}] tag=\"{}\" {}-bit x {} words_per_vdr={} base=0x{:08X}",
        ri.tag_ascii(),
        ri.word_width(),
        ri.word_count,
        ri.words_per_vdr,
        ri.base_addr
    )
}

/// The O1 HEAD render — byte-identical to `o1_desk`'s `render_header`. Kept in
/// the O1 decoder's orbit (it names `o1host::Header`), exposed here so the
/// parity credential has one place to compare. Non-O1 instruments render their
/// own header through their decoder or RAW.
pub fn o1_header(h: &o1host::Header) -> String {
    format!(
        "core_rev={} ring={}x{}w policy={} armed={} overflowed={} \
         events={} dropped={} rollovers={}\n  filter: {}",
        h.core_rev_str(),
        h.ring_depth,
        h.words_per_event,
        h.ring_policy,
        h.armed as u8,
        h.overflowed as u8,
        h.event_count,
        h.dropped_count,
        h.rollover_count,
        h.filter_str()
    )
}

/// RAW render for an unidentified instrument or an undecoded region: the
/// literal word UNDECODED, the header hex, and byte-faithful words — never a
/// guess about an instrument the host cannot identify.
pub fn raw_region(region_index: u8, words: &[u32]) -> String {
    let mut s = format!("[region {region_index}] UNDECODED ({} words)", words.len());
    for (i, w) in words.iter().enumerate() {
        s.push_str(&format!("\n  [{i:>4}] 0x{w:08X}"));
    }
    s
}
