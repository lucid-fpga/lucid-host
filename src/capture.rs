//! The capture container (D4 / HGT2 / HGT6), v1.
//!
//! Every capture is evidence, mechanically: a versioned, line-oriented header
//! carrying the full provenance (tool rev + dirty, dependency stack, run_mode,
//! the fabric's own IDENT and decoded header, the timestamp domain, seed),
//! followed by the event payload encoded in the suite's NATIVE record format —
//! a lucid-trace `LTRC` dump (`RawRecord`s + an embedded self-describing
//! schema). Native payloads mean a consumer reads the events with lucid-trace's
//! own `read_dump` and nothing bespoke (HGT6): the replay handoff needs no O1
//! decoder of its own.
//!
//! The first line is the version gate. A reader refuses a headerless file, and
//! refuses a version it does not implement, loudly (D4). The header stays text
//! so it diffs field-by-field (D10); the payload is length-prefixed binary.

use crate::provenance::Provenance;
use lucid_trace::dump::{read_dump, write_dump};
use lucid_trace::{RawRecord, Schema};
use std::io::{self, Write};

/// The container format version. A reader refuses a file whose version it does
/// not implement, so the format can grow without silent misreads.
pub const CONTAINER_VERSION: u16 = 1;

/// The magic first token — its presence is what a reader checks before trusting
/// a byte (a headerless capture is refused at read).
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

    /// Parse the header token.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "SD" => Some(RunMode::Sd),
            "JTAG" => Some(RunMode::Jtag),
            "SIM" => Some(RunMode::Sim),
            _ => None,
        }
    }
}

/// A v1 capture container. Provenance is held as owned strings so a container
/// read back from a file carries the WRITER's provenance, not this build's.
#[derive(Debug, Clone)]
pub struct Capture {
    /// The writing tool's git revision.
    pub tool_rev: String,
    /// `dirty`/`clean`/`unknown` at write time.
    pub tool_dirty: String,
    /// The composed dependency stack (`name@rev,…`).
    pub deps: String,
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
    /// The asset seed, where the capture drove a randomized delivery.
    pub seed: Option<u32>,
    /// A one-line decoded summary of the instrument's header (core_rev, filter,
    /// overflow block) — decoder-provided, so it is not O1-specific here.
    pub header_summary: Option<String>,
    /// A one-line decoded digest of the instrument's summary/aggregate region
    /// (O1's SUMM cadence: min/mean/max, histogram) — decoder-provided, so the
    /// container carries the aggregate without O1-specific code here. The
    /// per-item detail (located exceptions) rides `records` as native records.
    pub summary: Option<String>,
    /// The event schema (native lucid-trace).
    pub schema: Schema,
    /// The events, as native lucid-trace records.
    pub records: Vec<RawRecord>,
}

impl Capture {
    /// Build a writable capture from the current build's provenance and the
    /// native payload produced by an instrument's decoder.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_mode: RunMode,
        instrument_id: u16,
        instrument_version: u32,
        proto_version: u16,
        core_clock_hz: u32,
        seed: Option<u32>,
        header_summary: Option<String>,
        summary: Option<String>,
        schema: Schema,
        records: Vec<RawRecord>,
    ) -> Self {
        let p = Provenance::current();
        Capture {
            tool_rev: p.tool_rev.to_string(),
            tool_dirty: p.dirty.to_string(),
            deps: p.deps.to_string(),
            run_mode,
            instrument_id,
            instrument_version,
            proto_version,
            core_clock_hz,
            seed,
            header_summary,
            summary,
            schema,
            records,
        }
    }

    /// Write the container: the text header, then the length-prefixed native
    /// `LTRC` payload, then `END`.
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        writeln!(w, "{CONTAINER_MAGIC} v{CONTAINER_VERSION}")?;
        writeln!(w, "tool lucid-host {} {}", self.tool_rev, self.tool_dirty)?;
        writeln!(w, "deps {}", self.deps)?;
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
        if let Some(h) = &self.header_summary {
            writeln!(w, "instrument_header {h}")?;
        }
        if let Some(s) = &self.summary {
            writeln!(w, "instrument_summary {s}")?;
        }
        // Stamp the timestamp domain into the schema so the payload is fully
        // self-describing, then dump natively.
        let mut schema = self.schema.clone();
        schema.core_clock_hz = self.core_clock_hz;
        schema.schema_hash = schema.computed_hash();
        let dump = write_dump(&schema, &self.records);
        writeln!(w, "payload {}", dump.len())?;
        w.write_all(&dump)?;
        writeln!(w)?;
        writeln!(w, "END")?;
        Ok(())
    }

    /// Write to a byte vector (infallible to a Vec).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.write(&mut buf).expect("write to Vec is infallible");
        buf
    }

    /// Read a container back, refusing a headerless file, an unimplemented
    /// version, and a missing required field — each naming what is wrong (HGT4).
    pub fn read(bytes: &[u8]) -> Result<Self, crate::error::HostError> {
        use crate::error::HostError;

        // --- the version gate, first line ---
        let mut pos = 0usize;
        let first = read_line(bytes, &mut pos)
            .ok_or_else(|| HostError::Refused("empty file: no capture header".into()))?;
        let want = format!("{CONTAINER_MAGIC} v{CONTAINER_VERSION}");
        if first != want {
            if let Some(rest) = first.strip_prefix(&format!("{CONTAINER_MAGIC} v")) {
                return Err(HostError::Refused(format!(
                    "unsupported container version {rest} (this reader is v{CONTAINER_VERSION})"
                )));
            }
            return Err(HostError::Refused(
                "not a lucid-host capture: missing version gate on the first line".into(),
            ));
        }

        // --- the text header, up to the payload marker ---
        let mut tool_rev = None;
        let mut tool_dirty = None;
        let mut deps = None;
        let mut run_mode = None;
        let mut instrument = None; // (id, version, proto)
        let mut core_clock_hz = None;
        let mut seed = None;
        let mut header_summary = None;
        let mut summary = None;

        let payload_len: usize = loop {
            let line = read_line(bytes, &mut pos)
                .ok_or_else(|| HostError::Refused("truncated header: no payload marker".into()))?;
            if let Some(n) = line.strip_prefix("payload ") {
                break n
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| HostError::Refused("bad payload length".into()))?;
            } else if let Some(v) = line.strip_prefix("tool lucid-host ") {
                let mut it = v.rsplitn(2, ' ');
                tool_dirty = it.next().map(str::to_string);
                tool_rev = it.next().map(str::to_string);
            } else if let Some(v) = line.strip_prefix("deps ") {
                deps = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("run_mode ") {
                run_mode = RunMode::parse(v.trim());
            } else if let Some(v) = line.strip_prefix("instrument ") {
                instrument = parse_instrument(v);
            } else if let Some(v) = line.strip_prefix("timestamp_domain_hz ") {
                core_clock_hz = v.trim().parse::<u32>().ok();
            } else if let Some(v) = line.strip_prefix("seed ") {
                seed = if v.trim() == "none" {
                    Some(None)
                } else {
                    u32::from_str_radix(v.trim().trim_start_matches("0x"), 16)
                        .ok()
                        .map(Some)
                };
            } else if let Some(v) = line.strip_prefix("instrument_summary ") {
                summary = Some(v.to_string());
            } else if let Some(v) = line.strip_prefix("instrument_header ") {
                header_summary = Some(v.to_string());
            }
        };

        // required fields, each named if missing (HGT2/HGT4)
        let run_mode = run_mode.ok_or_else(|| HostError::Refused("header missing: run_mode".into()))?;
        let (instrument_id, instrument_version, proto_version) =
            instrument.ok_or_else(|| HostError::Refused("header missing: instrument".into()))?;
        let core_clock_hz =
            core_clock_hz.ok_or_else(|| HostError::Refused("header missing: timestamp_domain_hz".into()))?;

        // --- the native payload ---
        let end = pos + payload_len;
        if end > bytes.len() {
            return Err(HostError::Refused(
                "truncated capture: payload shorter than its declared length".into(),
            ));
        }
        let (schema, records) = read_dump(&bytes[pos..end])
            .map_err(|e| HostError::Decode(format!("native payload: {e}")))?;

        Ok(Capture {
            tool_rev: tool_rev.unwrap_or_else(|| "unknown".into()),
            tool_dirty: tool_dirty.unwrap_or_else(|| "unknown".into()),
            deps: deps.unwrap_or_default(),
            run_mode,
            instrument_id,
            instrument_version,
            proto_version,
            core_clock_hz,
            seed: seed.flatten(),
            header_summary,
            summary,
            schema,
            records,
        })
    }
}

/// Read one `\n`-terminated line as UTF-8, advancing `pos` past it. `None` at
/// end of input.
fn read_line(bytes: &[u8], pos: &mut usize) -> Option<String> {
    if *pos >= bytes.len() {
        return None;
    }
    let start = *pos;
    let rel = bytes[start..].iter().position(|&b| b == b'\n');
    let (line_end, next) = match rel {
        Some(i) => (start + i, start + i + 1),
        None => (bytes.len(), bytes.len()),
    };
    *pos = next;
    Some(String::from_utf8_lossy(&bytes[start..line_end]).into_owned())
}

/// Parse an `instrument 0xNNNN version V proto P` line into `(id, version, proto)`.
fn parse_instrument(v: &str) -> Option<(u16, u32, u16)> {
    let mut id = None;
    let mut ver = None;
    let mut proto = None;
    let mut it = v.split_whitespace();
    while let Some(tok) = it.next() {
        match tok {
            t if t.starts_with("0x") => id = u16::from_str_radix(&t[2..], 16).ok(),
            "version" => ver = it.next().and_then(|x| x.parse().ok()),
            "proto" => proto = it.next().and_then(|x| x.parse().ok()),
            _ => {}
        }
    }
    Some((id?, ver?, proto?))
}
