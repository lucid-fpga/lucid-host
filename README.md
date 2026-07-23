# lucid-host

A bench host for **LUCID instrument nodes** — small logic instruments that live
on an FPGA's JTAG hub (an SLD Virtual JTAG node), buffer timestamped events into
a ring, and are drained over a cable. `lucid-host` attaches to a live instrument
over any transport, identifies it, drives it, drains it, and renders its capture
as evidence.

It is the one committed place that composition lives, so it never has to be
rebuilt ad hoc at a bench. Dual-licensed **MIT OR Apache-2.0**.

## What it is — and is not

It **is** a bench host: attach, identify, drain, render, and write a versioned
capture container. It is **not** a debugger, not SignalTap, not a waveform
viewer. It makes no claims about an instrument it cannot identify — an unknown
device renders `RAW` and the literal word `UNDECODED`, never a guess.

## The command surface

Commands are split into two groups, and the split is visible in `--help`:

- **Read-only** (`doctor`, `enumerate`, `ident`, `head`, `status`, `drain`) —
  they only read the instrument, so a scripted battery can run them unattended.
- **CTRL** (`arm`, `disarm`, `clear`, `filter`, `policy`) — they write to the
  instrument and are meant to be run with care.

Every command prints a provenance banner: the tool's own revision and dirty
state, and the exact revision of each composed crate — all derived at build
time, never typed. A capture built against a local-path dependency says so,
because such a build is not reproducible.

Exit codes are a stable contract for scripting:

| code | meaning |
| --- | --- |
| 0 | ok |
| 2 | refused (a boundary check failed: proto/magic/manufacturer/bounds) |
| 3 | transport fault (the cable or sim failed) |
| 4 | decode fault (the bytes would not decode) |
| 64 | usage |

## Point it at YOUR instrument

The `Tap` trait (from `lucid-sld`) is the seam. To use `lucid-host` with your
own transport and instrument:

1. **Implement `Tap`** for your transport (a cable, a probe, a sim). You inherit
   the whole host.
2. **Write your instrument's field map and a drift test** that checks the code
   constants against it, so the decode has one authoritative source.
3. **Register your decoder** through the decoder interface, keyed off the IDENT
   `instrument_id`. Adding a decoder touches no host code beyond registration.

## The capture container

A capture is written in a versioned, line-oriented, self-describing format: a
header carrying the full provenance (tool, dependency stack, run mode, the
fabric's own identity and revision, the timestamp domain, and a seed where the
delivery was randomized), followed by the raw region payloads. Keeping the
payloads raw makes the container lossless and instrument-agnostic — a decoder
re-decodes them on read. The first line is a version gate, so a reader refuses a
headerless file, and refuses a file whose version it does not implement.

## Honest scope

What has been witnessed on silicon, and what has not, is stated in
[`PARITY.md`](PARITY.md). Read it before citing a capture as evidence.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at
your option.
