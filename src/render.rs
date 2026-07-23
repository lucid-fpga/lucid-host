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

/// The lower gap bound (ticks) a log2 histogram bucket represents. Buckets are
/// `{leading_one_position[4:0], three_mantissa_bits}` (FIELD-MAP §Histogram):
/// bucket 0 is a zero (or single-tick) gap; otherwise the leading one sits at
/// `b>>3` and the next three bits refine it, so a real gap resolves to within
/// `2^(lead-3)` ticks. Rendered as a lower bound so the distribution the mean
/// hides is legible without pretending to more precision than the encoding has.
pub fn hist_bucket_gap_lo(b: usize) -> u64 {
    if b == 0 {
        return 0;
    }
    let lead = (b >> 3) as u32;
    let mant = (b & 0x7) as u64;
    let base = 1u64 << lead;
    if lead >= 3 {
        base + mant * (1u64 << (lead - 3))
    } else {
        base
    }
}

/// The full O1 SUMM cadence render (min/mean/max, the histogram, and the
/// exception log with every over-threshold gap LOCATED at its byte offset).
/// This is the live `drain 1` view; the capture container carries the same
/// facts as a digest line plus the exceptions as native records.
pub fn o1_summary(s: &o1host::Summary) -> String {
    let mut out = format!(
        "core cadence: writes={} gap[min/mean/max]={}/{:.1}/{} ticks nonseq={} threshold={}",
        s.write_count, s.gap_min, s.gap_mean(), s.gap_max, s.nonseq_count, s.threshold
    );
    // The aggregate is over ALL observed writes — SUMM is ungated by the filter
    // (independent of the ring WINDOW by construction), so mean/max include the
    // boot-mailbox idle gaps and are NOT delivery figures. The delivery cadence
    // lives in the histogram and the located exceptions below. `gap_min` is safe
    // to read as the native rate (the smallest gap is always a payload gap).
    out.push_str(
        "\n  (min/mean/max span ALL observed writes, incl. boot-mailbox idle — for the \
         delivery cadence read the histogram + exceptions below, not mean/max)",
    );
    out.push_str(&format!(
        "\n  addr span: 0x{:08X} -> 0x{:08X}",
        s.first_addr, s.last_addr
    ));

    let nonzero: Vec<(usize, u32)> = s
        .histogram
        .iter()
        .enumerate()
        .filter(|(_, &c)| c != 0)
        .map(|(b, &c)| (b, c))
        .collect();
    out.push_str(&format!(
        "\n  histogram (log2 buckets, {} of {} nonzero):",
        nonzero.len(),
        s.histogram.len()
    ));
    for (b, c) in &nonzero {
        out.push_str(&format!(
            "\n    bucket {b:>3} (>= {:>10} ticks): {c}",
            hist_bucket_gap_lo(*b)
        ));
    }

    out.push_str(&format!(
        "\n  exceptions: {} logged, {} dropped",
        s.exc_count, s.exc_dropped
    ));
    for (i, e) in s.exceptions.iter().enumerate() {
        out.push_str(&format!(
            "\n    #{i} gap={} @ 0x{:08X}  (write #{}, seq {})",
            e.gap, e.addr, e.write_ordinal, e.seq
        ));
    }
    out
}

/// A one-line SUMM digest for the capture container header — the aggregate
/// scalars plus the nonzero histogram buckets, so the container carries the
/// distribution and the cadence in a diffable line (D10). The located
/// exceptions ride the payload as native records, not this line.
pub fn o1_summary_line(s: &o1host::Summary) -> String {
    let hist: Vec<String> = s
        .histogram
        .iter()
        .enumerate()
        .filter(|(_, &c)| c != 0)
        .map(|(b, &c)| format!("{b}:{c}"))
        .collect();
    format!(
        "writes={} gap_min={} gap_mean={:.1} gap_max={} nonseq={} threshold={} exc={} exc_dropped={} hist=[{}]",
        s.write_count,
        s.gap_min,
        s.gap_mean(),
        s.gap_max,
        s.nonseq_count,
        s.threshold,
        s.exc_count,
        s.exc_dropped,
        hist.join(",")
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
