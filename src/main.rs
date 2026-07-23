//! `lucid-host` — the thin bin over the composition library.
//!
//! Flag-parsing and rendering only; every real operation is a library call, so
//! a sibling can dev-depend on the lib without the bin. The command surface is
//! scope-split: read-only commands a scripted battery may run unattended, and
//! CTRL-writing commands that need care. The split is visible in `--help`. Exit
//! codes are typed.

use std::process::ExitCode;

use lucid_host::capture::{Capture, RunMode};
use lucid_host::decoder::{Ctrl, Policy, Registry};
use lucid_host::error::HostError;
use lucid_host::{diff, host, render, Provenance};

/// The Analogue Pocket's Cyclone V JTAG IR length — supplied to CableTap out of
/// band (the SLD layers never see it).
const CYCLONE_V_IR_LEN: usize = 10;

fn usage() -> String {
    format!(
        "lucid-host {} — bench host for LUCID instrument nodes\n\
         \n\
         usage: lucid-host <command> [args]\n\
         \n\
         READ-ONLY commands (safe to script unattended):\n\
         \x20 doctor              cable health + hub walk + instrument IDENT\n\
         \x20 enumerate           the SLD hub walk: hub + nodes\n\
         \x20 ident               the instrument IDENT block\n\
         \x20 head                the instrument header, decoded (or RAW if unknown)\n\
         \x20 status              the instrument STATUS\n\
         \x20 drain <region>      drain a region and render it (decoded or RAW)\n\
         \x20 capture <file> [sd|jtag]   drain to a capture container file\n\
         \n\
         CTRL commands (write to the instrument — need care):\n\
         \x20 arm | disarm        arm/disarm the recorder\n\
         \x20 clear               clear the ring, counters, and sticky flags\n\
         \x20 filter <mask> <lo> <hi>   set the event filter (hex)\n\
         \x20 policy <stop|wrap>  set the overflow policy\n\
         \x20 threshold <ticks>  set the SUMM exception threshold (dec or 0xhex)\n\
         \n\
         FILE commands (no cable):\n\
         \x20 show <file>         read a capture container and render it\n\
         \x20 diff <a> <b>        diff two capture containers\n\
         \n\
         other:\n\
         \x20 version             print version and provenance\n\
         \x20 --help              this text\n\
         \n\
         exit codes: 0 ok · 2 refused · 3 transport · 4 decode · 64 usage",
        lucid_host::version()
    )
}

fn provenance_banner() -> String {
    let p = Provenance::current();
    let mut s = p.lines();
    if p.has_local_path_dep() {
        s.push_str("\n  WARNING: a dependency resolved to a LOCAL-PATH — this build is not reproducible");
    }
    s
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lucid-host: {e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn run(args: &[String]) -> Result<(), HostError> {
    let cmd = args.first().map(String::as_str).unwrap_or("--help");
    match cmd {
        "--help" | "-h" | "help" => {
            println!("{}", usage());
            Ok(())
        }
        "version" => {
            println!("lucid-host {}", lucid_host::version());
            println!("{}", provenance_banner());
            Ok(())
        }
        // read-only + CTRL + capture all touch the cable
        "doctor" | "enumerate" | "ident" | "head" | "status" | "drain" | "capture" | "arm"
        | "disarm" | "clear" | "filter" | "policy" | "threshold" => run_device(cmd, args),
        // file commands need no cable
        "show" => run_show(args),
        "diff" => run_diff(args),
        other => Err(HostError::Usage(format!(
            "unknown command '{other}' (try --help)"
        ))),
    }
}

fn arg<'a>(args: &'a [String], i: usize, what: &str) -> Result<&'a str, HostError> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| HostError::Usage(format!("missing argument: {what}")))
}

fn parse_hex(s: &str, what: &str) -> Result<u32, HostError> {
    u32::from_str_radix(s.trim_start_matches("0x"), 16)
        .map_err(|_| HostError::Usage(format!("{what} must be hex")))
}

/// Parse a u32 written either as decimal or as `0x`-prefixed hex.
fn parse_u32(s: &str, what: &str) -> Result<u32, HostError> {
    let parsed = match s.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => s.parse::<u32>(),
    };
    parsed.map_err(|_| HostError::Usage(format!("{what} must be a u32 (decimal or 0xhex)")))
}

/// Open the real cable and run a device command. The cable is only touched here,
/// at runtime — the build/test/clippy gate never needs one.
fn run_device(cmd: &str, args: &[String]) -> Result<(), HostError> {
    let cable = blaster2::Cable::open()
        .map_err(|e| HostError::Transport(format!("open USB-Blaster II: {e}")))?;
    let mut tap = lucid_sld::blaster2_tap::CableTap::new(cable, CYCLONE_V_IR_LEN);

    let a = host::attach(&mut tap)?;
    let registry = Registry::with_builtins();
    let decoder = registry.get(a.ident.instrument_id);

    println!("=== provenance ===\n{}", provenance_banner());

    match cmd {
        "enumerate" => println!("{}", render::enumeration(&a.enumeration)),
        "ident" => println!("{}", render::ident(&a.ident)),
        "doctor" => {
            println!("{}", render::enumeration(&a.enumeration));
            println!("{}", render::ident(&a.ident));
            for r in 0..a.ident.region_count {
                let ri = host::region_info(&mut tap, &a.node, r)?;
                println!("{}", render::region(r, &ri));
            }
        }
        "head" => {
            let hr = decoder.map(|d| d.header_region()).unwrap_or(0);
            let words = host::drain_region(&mut tap, &a.node, hr)?;
            match decoder {
                Some(d) => println!("{}", d.render_header(&words)?),
                None => println!("{}", render::raw_region(hr, &words)),
            }
        }
        "status" => {
            let chunks = host::status_chunks(&mut tap, &a.node)?;
            match decoder {
                Some(d) => println!("{}", d.render_status(&chunks)),
                None => println!("status: (no decoder for instrument 0x{:04X})", a.ident.instrument_id),
            }
        }
        "drain" => {
            let region: u8 = arg(args, 1, "region index")?
                .parse()
                .map_err(|_| HostError::Usage("region index must be a number".into()))?;
            let words = host::drain_region(&mut tap, &a.node, region)?;
            match decoder {
                Some(d) => {
                    // the decoder renders only valid events, so it needs the header
                    let head = host::drain_region(&mut tap, &a.node, d.header_region())?;
                    println!("{}", d.render_region(region, &words, &head));
                }
                None => println!("{}", render::raw_region(region, &words)),
            }
        }
        "capture" => {
            let path = arg(args, 1, "output file")?;
            let run_mode = match args.get(2).map(String::as_str) {
                Some("sd") => RunMode::Sd,
                Some("jtag") | None => RunMode::Jtag,
                Some(other) => return Err(HostError::Usage(format!("run mode must be sd|jtag, got {other}"))),
            };
            let d = decoder
                .ok_or_else(|| HostError::Refused("no decoder for this instrument — cannot capture".into()))?;
            let cap = host::capture(&mut tap, &a.node, &a.ident, d, run_mode, None)?;
            std::fs::write(path, cap.to_bytes())
                .map_err(|e| HostError::Transport(format!("write {path}: {e}")))?;
            println!("wrote {} ({} events, {} bytes)", path, cap.records.len(), cap.to_bytes().len());
        }
        "arm" => {
            let d = decoder.ok_or_else(|| HostError::Refused("no decoder — cannot arm".into()))?;
            let (polls, _) = host::arm_and_wait(&mut tap, &a.node, d, false)?;
            println!("armed (reported after {polls} status polls)");
        }
        "disarm" | "clear" => {
            let d = decoder.ok_or_else(|| HostError::Refused("no decoder for CTRL".into()))?;
            let action = if cmd == "disarm" { Ctrl::Disarm } else { Ctrl::Clear };
            host::apply_ctrl(&mut tap, &a.node, d, &action, false)?;
            println!("{cmd}: done");
        }
        "policy" => {
            let d = decoder.ok_or_else(|| HostError::Refused("no decoder for CTRL".into()))?;
            let policy = match arg(args, 1, "stop|wrap")? {
                "stop" => Policy::Stop,
                "wrap" => Policy::Wrap,
                other => return Err(HostError::Usage(format!("policy must be stop|wrap, got {other}"))),
            };
            host::apply_ctrl(&mut tap, &a.node, d, &Ctrl::Policy(policy), false)?;
            println!("policy: set");
        }
        "filter" => {
            let d = decoder.ok_or_else(|| HostError::Refused("no decoder for CTRL".into()))?;
            let kind_mask = parse_hex(arg(args, 1, "kind mask")?, "kind mask")? as u16;
            let addr_lo = parse_hex(arg(args, 2, "addr lo")?, "addr lo")?;
            let addr_hi = parse_hex(arg(args, 3, "addr hi")?, "addr hi")?;
            host::apply_ctrl(
                &mut tap,
                &a.node,
                d,
                &Ctrl::Filter { kind_mask, addr_lo, addr_hi },
                false,
            )?;
            println!("filter: set");
        }
        "threshold" => {
            let d = decoder.ok_or_else(|| HostError::Refused("no decoder for CTRL".into()))?;
            let ticks = parse_u32(arg(args, 1, "ticks")?, "ticks")?;
            host::apply_ctrl(&mut tap, &a.node, d, &Ctrl::Threshold(ticks), false)?;
            println!("threshold: set to {ticks} ticks");
        }
        _ => unreachable!("run_device only called for device commands"),
    }
    Ok(())
}

fn run_show(args: &[String]) -> Result<(), HostError> {
    let path = arg(args, 1, "capture file")?;
    let bytes = std::fs::read(path).map_err(|e| HostError::Transport(format!("read {path}: {e}")))?;
    let cap = Capture::read(&bytes)?;
    println!("tool: lucid-host {} ({})", cap.tool_rev, cap.tool_dirty);
    println!("deps: {}", cap.deps);
    println!(
        "run_mode: {}  instrument: 0x{:04X} v{} proto {}  clock: {} Hz",
        cap.run_mode.as_str(), cap.instrument_id, cap.instrument_version, cap.proto_version, cap.core_clock_hz
    );
    if let Some(h) = &cap.header_summary {
        println!("header: {h}");
    }
    if let Some(s) = &cap.summary {
        println!("summary: {s}");
    }
    println!("events: {} native records", cap.records.len());
    // `show <file> events` decodes each native record through lucid-trace's own
    // decoder and prints its fields — the container's events, read the same way
    // a downstream consumer would, with no O1-specific code here. Two record
    // kinds share the payload: ring `event`s and SUMM `exception`s (a located
    // over-threshold gap); each is rendered from the fields its schema names.
    if args.iter().any(|a| a == "events") {
        for raw in &cap.records {
            match lucid_trace::decode_record(&cap.schema, raw) {
                Ok(rec) => {
                    let f = |n: &str| -> u128 {
                        match rec.get(n) {
                            Some(lucid_trace::FieldValue::Hex(v))
                            | Some(lucid_trace::FieldValue::U(v))
                            | Some(lucid_trace::FieldValue::Enum(v)) => *v,
                            _ => 0,
                        }
                    };
                    if rec.get("gap").is_some() {
                        // a SUMM exception: the stall, LOCATED at its byte offset
                        println!(
                            "  exception gap={} @ 0x{:08X} write#{} seq={}",
                            f("gap"), f("addr") as u32, f("write_ordinal"), f("seq")
                        );
                    } else {
                        println!(
                            "  t={:>10} kind={} addr=0x{:08X} data=0x{:08X} seq={}",
                            rec.timestamp, f("kind"), f("addr") as u32, f("data") as u32, f("seq")
                        );
                    }
                }
                Err(e) => println!("  (record decode: {e})"),
            }
        }
    }
    // `show <file> boot` runs OGT2 — the after-the-fact boot decoder — and prints
    // the reconstructed handshake, descriptor table and deliveries as machine-
    // parseable lines (a leading tag per row) so the boot-record generator renders
    // its tables from this output rather than re-decoding the container itself.
    if args.iter().any(|a| a == "boot") {
        let rec = lucid_host::boot::decode(&cap.records, &cap.schema);
        let mut prev_ts = 0u32;
        for (i, c) in rec.commands.iter().enumerate() {
            let gap = if i == 0 { 0 } else { c.timestamp.wrapping_sub(prev_ts) };
            prev_ts = c.timestamp;
            println!(
                "CMD seq={} t={} gap={} code=0x{:04X} name=\"{}\" p0=0x{:08X} p1=0x{:08X}",
                c.seq, c.timestamp, gap, c.code,
                lucid_host::boot::command_name(c.code), c.params[0], c.params[1]
            );
        }
        for de in &rec.descriptors {
            println!("DESC slot_id={} size=0x{:08X} seq={}", de.slot_id, de.size, de.seq);
        }
        for dl in &rec.deliveries {
            println!("DELIVERY base=0x{:08X} writes={} first_seq={}", dl.base, dl.writes, dl.first_seq);
        }
    }
    Ok(())
}

fn run_diff(args: &[String]) -> Result<(), HostError> {
    let pa = arg(args, 1, "capture A")?;
    let pb = arg(args, 2, "capture B")?;
    let a = std::fs::read(pa).map_err(|e| HostError::Transport(format!("read {pa}: {e}")))?;
    let b = std::fs::read(pb).map_err(|e| HostError::Transport(format!("read {pb}: {e}")))?;
    // The diff itself succeeding is exit 0 (the tool worked); a script branches
    // on the stable first token, "IDENTICAL" vs "DIVERGENT". Only a refusal
    // (bad/missing header, version mismatch) inherited from the read is non-zero.
    let d = diff::diff_bytes(&a, &b)?;
    println!("{}", diff::render(&d));
    Ok(())
}
