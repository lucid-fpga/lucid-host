//! Provenance is DERIVED, never typed. This script captures, at build time:
//! the tool's own git revision and dirty state, and the git revision of every
//! composed dependency read from `Cargo.lock`. A dependency resolved to a local
//! path (no git revision in the lock) is surfaced as `LOCAL-PATH`, because a
//! path dep is not reproducible and a capture must say so rather than imply a
//! pinned build.

use std::process::Command;

const TRACKED_DEPS: &[&str] = &["lucid-sld", "blaster2", "o1host", "lucid-trace"];

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Parse `Cargo.lock` for each tracked dependency's short git revision, or
/// `LOCAL-PATH` if it resolved without one.
fn dep_stack() -> String {
    let lock = std::fs::read_to_string("Cargo.lock").unwrap_or_default();
    let mut name: Option<String> = None;
    let mut source: Option<String> = None;
    let mut revs: Vec<(String, String)> = Vec::new();

    let flush = |name: &Option<String>, source: &Option<String>, revs: &mut Vec<(String, String)>| {
        if let Some(n) = name {
            if TRACKED_DEPS.contains(&n.as_str()) {
                let rev = match source {
                    Some(s) if s.contains("git+") => s
                        .split('#')
                        .nth(1)
                        .map(|h| h.chars().take(8).collect::<String>())
                        .unwrap_or_else(|| "git".into()),
                    _ => "LOCAL-PATH".into(),
                };
                revs.push((n.clone(), rev));
            }
        }
    };

    for line in lock.lines() {
        if line == "[[package]]" {
            flush(&name, &source, &mut revs);
            name = None;
            source = None;
        } else if let Some(v) = line.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = line.strip_prefix("source = ") {
            source = Some(v.trim_matches('"').to_string());
        }
    }
    flush(&name, &source, &mut revs);

    // stable order: as listed in TRACKED_DEPS
    let mut ordered: Vec<String> = Vec::new();
    for want in TRACKED_DEPS {
        if let Some((n, r)) = revs.iter().find(|(n, _)| n == want) {
            ordered.push(format!("{n}@{r}"));
        }
    }
    ordered.join(",")
}

fn main() {
    let rev = git(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| "unknown".into());
    // The dirty flag must distinguish success-with-EMPTY-output (a clean tree —
    // the common case) from a failed command (unknown). The `git()` helper above
    // collapses empty output to None, which is right for `rev-parse` but wrong
    // here: a clean `git status --porcelain` is empty and MUST read `clean`, not
    // `unknown` (the header quirk). So this check runs git directly.
    let dirty = match Command::new("git")
        .args(["status", "--porcelain"])
        .output()
    {
        Ok(out) if out.status.success() => {
            if String::from_utf8_lossy(&out.stdout).trim().is_empty() {
                "clean"
            } else {
                "dirty"
            }
        }
        _ => "unknown",
    };

    println!("cargo:rustc-env=LUCID_HOST_REV={rev}");
    println!("cargo:rustc-env=LUCID_HOST_DIRTY={dirty}");
    println!("cargo:rustc-env=LUCID_HOST_DEPS={}", dep_stack());
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
