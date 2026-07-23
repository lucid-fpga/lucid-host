//! The capture container (D4 / HGT2 / HGT6), v1 — the writer.
//!
//! Every capture is evidence, mechanically: a versioned header carrying the
//! full provenance (tool rev + dirty, dependency stack, run_mode, the fabric's
//! own IDENT, the timestamp domain, seed) plus the RAW region payloads. Keeping
//! the payloads raw (including the instrument's HEAD region) makes the container
//! lossless and instrument-agnostic — `core_rev`, the filter and the overflow
//! block all live inside the HEAD payload and are re-decoded by the instrument's
//! decoder on read, never re-typed here.
//!
//! The format is line-oriented and self-describing (D7): the first line is the
//! version gate, so an H2 reader refuses a headerless file, and a v1 reader
//! refuses a v2 file, loudly. GT6 lineage: the container co-versions with
//! `lucid_trace::schema::FORMAT_VERSION`, the record format the overlay and
//! lucid-oracle read.

use crate::provenance::Provenance;
use std::io::{self, Write};

/// The container format version. A reader refuses a file whose version it does
/// not implement (D4 criterion), so the format can grow without silent
/// misreads.
pub const CONTAINER_VERSION: u16 = 1;

/// The magic first token — its presence is what a reader checks before trusting
/// a byte (HGT2: a headerless capture is refused at read).
pub const CONTAINER_MAGIC: &str = "LUCID-CAPTURE";

/// How the capture was produced — the standing hardware-scope reading, recorded
/// (a claim about the platform requires SD; a sim capture must say SIM).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// SD-launched on real silicon — the only mode that can make a platform claim.
    Sd,
    /// JTAG-configured (development) on real silicon.
    Jtag,
    /// A simulated transport — never a platform claim.
    Sim,
}

impl RunMode {
    /// The token written to / read from the header.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunMode::Sd => "SD",
            RunMode::Jtag => "JTAG",
            RunMode::Sim => "SIM",
        }
    }
}

/// One region's raw payload.
#[derive(Debug, Clone)]
pub struct RegionPayload {
    /// The region index.
    pub id: u8,
    /// The region's ASCII tag (e.g. `RING`, `HEAD`).
    pub tag: String,
    /// The region's 32-bit words, verbatim.
    pub words: Vec<u32>,
}

/// A v1 capture container, ready to write.
#[derive(Debug, Clone)]
pub struct Capture {
    /// Build-derived provenance (tool rev, dirty, deps).
    pub provenance: Provenance,
    /// How the capture was produced.
    pub run_mode: RunMode,
    /// The IDENT instrument id (`0x0001` = O1).
    pub instrument_id: u16,
    /// The IDENT instrument version.
    pub instrument_version: u32,
    /// The LIN proto version the instrument answered.
    pub proto_version: u16,
    /// The fabric core clock in Hz — the timestamp domain (HGT2).
    pub core_clock_hz: u32,
    /// The asset seed, where the capture drove a randomized delivery; `None`
    /// for a pure observation.
    pub seed: Option<u32>,
    /// The raw region payloads (including the instrument's HEAD region).
    pub regions: Vec<RegionPayload>,
}

impl Capture {
    /// Write the container in the versioned line-oriented v1 format.
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        writeln!(w, "{CONTAINER_MAGIC} v{CONTAINER_VERSION}")?;
        writeln!(
            w,
            "trace_format v{}",
            lucid_trace::schema::FORMAT_VERSION
        )?;
        writeln!(
            w,
            "tool lucid-host {} {}",
            self.provenance.tool_rev, self.provenance.dirty
        )?;
        writeln!(w, "deps {}", self.provenance.deps)?;
        writeln!(w, "run_mode {}", self.run_mode.as_str())?;
        writeln!(
            w,
            "instrument 0x{:04X} version {} proto {}",
            self.instrument_id, self.instrument_version, self.proto_version
        )?;
        writeln!(w, "timestamp_domain_hz {}", self.core_clock_hz)?;
        match self.seed {
            Some(s) => writeln!(w, "seed 0x{s:08X}")?,
            None => writeln!(w, "seed none")?,
        }
        for r in &self.regions {
            writeln!(w, "region {} {} {}", r.id, r.tag, r.words.len())?;
            for word in &r.words {
                writeln!(w, "{word:08X}")?;
            }
        }
        writeln!(w, "END")?;
        Ok(())
    }

    /// Write the container to a byte vector (convenience for tests and the bin).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // writing to a Vec<u8> is infallible
        self.write(&mut buf).expect("write to Vec is infallible");
        buf
    }
}
