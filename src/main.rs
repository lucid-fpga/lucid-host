//! `lucid-host` — the thin bin over the composition library.
//!
//! Flag-parsing and rendering only; every real operation is a library call, so
//! apf-host can dev-depend on the lib for the parity credential without the bin
//! (D1). The command surface is scope-split (D5): read-only commands a scripted
//! battery may run unattended, and CTRL-writing commands that need care. The
//! split is visible in `--help`. Exit codes are typed (D7).

use std::process::ExitCode;

use lucid_host::decoder::Registry;
use lucid_host::error::HostError;
use lucid_host::{host, render, Provenance};

/// The Analogue Pocket's Cyclone V JTAG IR length — supplied to CableTap out of
/// band (the SLD layers never see it).
const CYCLONE_V_IR_LEN: usize = 10;

fn usage() -> String {
    format!(
        "lucid-host {} — bench host for LUCID instrument nodes\n\
         \n\
         usage: lucid-host <command> [args]\n\
         \n\
         READ-ONLY commands (safe to script unattended — D5):\n\
         \x20 doctor            cable health + hub walk + instrument IDENT (read-only)\n\
         \x20 enumerate         the SLD hub walk: hub + nodes\n\
         \x20 ident             the instrument IDENT block\n\
         \x20 head              the instrument header, decoded (or RAW if unknown)\n\
         \x20 status            the instrument STATUS\n\
         \x20 drain <region>    drain a region and render it (decoded or RAW)\n\
         \n\
         CTRL commands (write to the instrument — need care; H2):\n\
         \x20 arm | disarm      arm/disarm the recorder\n\
         \x20 clear             clear flags\n\
         \x20 filter ...        set the event filter\n\
         \x20 policy <stop|wrap> set the overflow policy\n\
         \n\
         other:\n\
         \x20 version           print version and provenance\n\
         \x20 --help            this text\n\
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
        "doctor" | "enumerate" | "ident" | "head" | "status" | "drain" => run_device(cmd, args),
        "arm" | "disarm" | "clear" | "filter" | "policy" => Err(HostError::Usage(format!(
            "'{cmd}' is a CTRL command — the read-only half ships first; CTRL lands next"
        ))),
        other => Err(HostError::Usage(format!(
            "unknown command '{other}' (try --help)"
        ))),
    }
}

/// Open the real cable and run a read-only device command. The cable is only
/// touched here, at runtime — the build/test/clippy gate never needs one.
fn run_device(cmd: &str, args: &[String]) -> Result<(), HostError> {
    let cable = blaster2::Cable::open()
        .map_err(|e| HostError::Transport(format!("open USB-Blaster II: {e}")))?;
    let mut tap = lucid_sld::blaster2_tap::CableTap::new(cable, CYCLONE_V_IR_LEN);

    let a = host::attach(&mut tap)?;
    let registry = Registry::with_builtins();

    // Every device command prints the provenance banner first (HGT2/D9).
    println!("=== provenance ===\n{}", provenance_banner());

    match cmd {
        "enumerate" => {
            println!("{}", render::enumeration(&a.enumeration));
        }
        "ident" => {
            println!("{}", render::ident(&a.ident));
        }
        "doctor" => {
            println!("{}", render::enumeration(&a.enumeration));
            println!("{}", render::ident(&a.ident));
            for r in 0..a.ident.region_count {
                let ri = host::region_info(&mut tap, &a.node, r)?;
                println!("{}", render::region(r, &ri));
            }
        }
        "head" | "status" => {
            let dec = registry.get(a.ident.instrument_id);
            let hr = dec.map(|d| d.header_region()).unwrap_or(0);
            let words = host::drain_region(&mut tap, &a.node, hr)?;
            match dec {
                Some(d) => println!("{}", d.render_header(&words)?),
                None => println!("{}", render::raw_region(hr, &words)),
            }
        }
        "drain" => {
            let region: u8 = args
                .get(1)
                .ok_or_else(|| HostError::Usage("drain needs a region index".into()))?
                .parse()
                .map_err(|_| HostError::Usage("region index must be a number".into()))?;
            let words = host::drain_region(&mut tap, &a.node, region)?;
            match registry.get(a.ident.instrument_id) {
                Some(d) => println!("{}", d.render_region(region, &words)),
                None => println!("{}", render::raw_region(region, &words)),
            }
        }
        _ => unreachable!("run_device only called for device commands"),
    }
    Ok(())
}
