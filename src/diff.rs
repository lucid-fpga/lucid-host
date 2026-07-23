//! `lucid-host diff` (D10): two capture containers in, their divergences out —
//! headers field-by-field, and the FIRST event divergence located by index. It
//! is the flat diff a scripted A/B needs so no ad-hoc analysis script ever
//! touches a capture again; the deeper localization stays lucid-oracle's.
//!
//! Refusals are inherited: each side is read through [`Capture::read`], so a
//! version mismatch or a missing header refuses here exactly as it does on a
//! plain read.

use crate::capture::Capture;
use crate::error::HostError;

/// One header field that differs between two captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDiff {
    /// The field name.
    pub field: String,
    /// The value on side A.
    pub a: String,
    /// The value on side B.
    pub b: String,
}

/// Where two captures' event streams first diverge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDivergence {
    /// The 0-based event index of the first divergence.
    pub index: usize,
    /// A human description of what differs there.
    pub detail: String,
}

/// The result of diffing two captures.
#[derive(Debug, Clone, Default)]
pub struct Diff {
    /// Header fields that differ.
    pub header: Vec<FieldDiff>,
    /// The event counts on each side.
    pub events_a: usize,
    /// The event counts on each side.
    pub events_b: usize,
    /// The first event divergence, if any.
    pub first_event: Option<EventDivergence>,
}

impl Diff {
    /// True if the two captures are identical in every compared field and event.
    pub fn is_empty(&self) -> bool {
        self.header.is_empty() && self.first_event.is_none() && self.events_a == self.events_b
    }
}

fn push_if_ne(out: &mut Vec<FieldDiff>, field: &str, a: String, b: String) {
    if a != b {
        out.push(FieldDiff {
            field: field.into(),
            a,
            b,
        });
    }
}

/// Diff two already-read captures.
pub fn diff(a: &Capture, b: &Capture) -> Diff {
    let mut header = Vec::new();
    push_if_ne(&mut header, "run_mode", a.run_mode.as_str().into(), b.run_mode.as_str().into());
    push_if_ne(&mut header, "instrument_id", format!("0x{:04X}", a.instrument_id), format!("0x{:04X}", b.instrument_id));
    push_if_ne(&mut header, "instrument_version", a.instrument_version.to_string(), b.instrument_version.to_string());
    push_if_ne(&mut header, "proto_version", a.proto_version.to_string(), b.proto_version.to_string());
    push_if_ne(&mut header, "timestamp_domain_hz", a.core_clock_hz.to_string(), b.core_clock_hz.to_string());
    push_if_ne(&mut header, "seed", format!("{:?}", a.seed), format!("{:?}", b.seed));
    push_if_ne(&mut header, "instrument_header", format!("{:?}", a.header_summary), format!("{:?}", b.header_summary));
    push_if_ne(&mut header, "tool_rev", a.tool_rev.clone(), b.tool_rev.clone());
    push_if_ne(&mut header, "deps", a.deps.clone(), b.deps.clone());

    let first_event = a
        .records
        .iter()
        .zip(b.records.iter())
        .enumerate()
        .find(|(_, (ra, rb))| {
            ra.tag != rb.tag
                || ra.timestamp != rb.timestamp
                || ra.payload != rb.payload
                || ra.payload_bits != rb.payload_bits
        })
        .map(|(index, (ra, rb))| EventDivergence {
            index,
            detail: format!(
                "tag {}/{}, t {}/{}, payload 0x{:X}/0x{:X}",
                ra.tag, rb.tag, ra.timestamp, rb.timestamp, ra.payload, rb.payload
            ),
        })
        .or_else(|| {
            // identical on the common prefix — a length difference diverges at
            // the end of the shorter stream.
            if a.records.len() != b.records.len() {
                Some(EventDivergence {
                    index: a.records.len().min(b.records.len()),
                    detail: format!("event count {} vs {}", a.records.len(), b.records.len()),
                })
            } else {
                None
            }
        });

    Diff {
        header,
        events_a: a.records.len(),
        events_b: b.records.len(),
        first_event,
    }
}

/// Read both sides (inheriting the read refusals) and diff them.
pub fn diff_bytes(a: &[u8], b: &[u8]) -> Result<Diff, HostError> {
    let ca = Capture::read(a)?;
    let cb = Capture::read(b)?;
    Ok(diff(&ca, &cb))
}

/// Render a diff for the line-oriented output (D7). Empty diffs say so.
pub fn render(d: &Diff) -> String {
    if d.is_empty() {
        return "diff: IDENTICAL (headers and events match)".into();
    }
    let mut s = String::from("diff: DIVERGENT");
    for f in &d.header {
        s.push_str(&format!("\n  header {}: {} != {}", f.field, f.a, f.b));
    }
    s.push_str(&format!("\n  events: {} vs {}", d.events_a, d.events_b));
    if let Some(e) = &d.first_event {
        s.push_str(&format!("\n  first event divergence at index {}: {}", e.index, e.detail));
    }
    s
}
