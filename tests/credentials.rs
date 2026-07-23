//! Credentials — each watched failing before trusted (the negative half is
//! written, not implied). These exercise lucid-host's own logic: the boundary
//! error mapping, the decoder interface including RAW, derived provenance, the
//! versioned container writer, and the render format. The full flow over a live
//! transport is credentialed from the sim crate's side (the parity check) and
//! at the bench.

use lucid_host::capture::{Capture, RegionPayload, RunMode};
use lucid_host::decoder::{O1Decoder, Registry};
use lucid_host::error::{exit, HostError};
use lucid_host::{render, Provenance};

/// A real pre-arm HEAD block captured from silicon — a genuine v1 block with
/// both magics present, so `Header::decode` accepts it.
const GOLDEN_HEAD: [u32; 16] = [
    0x4441_4548, // "HEAD"
    0x0000_0001, // header_version = 1
    0xF4F4_42D2, // core_rev
    0x0000_1000, // ring_depth = 4096
    0x0000_0004, // words_per_event = 4
    0x0000_0000, // flags
    0x0000_0000, // rollover_count
    0x0000_0000, // dropped_count
    0x0000_0000, // first_drop_seq
    0x0000_0000, // first_drop_time
    0x0000_FFFF, // filter_kind_mask
    0x0000_0000, // filter_lo
    0xFFFF_FFFF, // filter_hi
    0x0000_0000, // event_count
    0x0000_0000, // write_index
    0x4448_3149, // "I1HD"
];

// ---- HGT4: refuse-before-use maps to typed errors that NAME the mismatch ----

#[test]
fn hub_less_device_is_refused_naming_the_0x7ff_signature() {
    // A device with no SLD hub reads manufacturer 0x7FF; lucid-sld raises it and
    // lucid-host must surface it as a REFUSAL (not a transport fault), naming it.
    let err: HostError = lucid_sld::Error::Manufacturer {
        expected: 0x06E,
        got: 0x7FF,
    }
    .into();
    assert_eq!(err.exit_code(), exit::REFUSED);
    let msg = err.to_string();
    assert!(msg.contains("0x7FF"), "message must name the 0x7FF signature: {msg}");
    assert!(msg.contains("not Altera"), "and say why: {msg}");
}

#[test]
fn a_protocol_mismatch_is_a_refusal_not_a_transport_fault() {
    let err: HostError =
        lucid_sld::Error::Protocol("no instrument node (id 0x08) on the hub".into()).into();
    assert_eq!(err.exit_code(), exit::REFUSED);
    assert!(err.to_string().contains("instrument node"));
}

#[test]
fn a_shift_width_overflow_is_a_transport_fault() {
    let err: HostError = lucid_sld::Error::Width { bits: 80, max: 64 }.into();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
    assert!(err.to_string().contains("80"));
}

#[test]
fn exit_codes_are_distinct_per_class() {
    // D7: a script branches on these, so they must not collide.
    let codes = [
        exit::OK,
        exit::REFUSED,
        exit::TRANSPORT,
        exit::DECODE,
        exit::USAGE,
    ];
    let mut seen = codes.to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), codes.len(), "exit codes must be distinct");
}

// ---- HGT3 / D3: unknown → RAW+UNDECODED byte-faithful; known → decodes ----

#[test]
fn an_unregistered_instrument_renders_raw_and_the_bytes_round_trip() {
    let registry = Registry::empty();
    assert!(registry.get(0x0001).is_none(), "empty registry knows nothing");

    let words: Vec<u32> = vec![0xDEAD_BEEF, 0x0000_0000, 0x1234_5678, 0xFFFF_FFFF];
    let rendered = render::raw_region(0, &words);
    assert!(rendered.contains("UNDECODED"), "raw render must say UNDECODED");

    // byte-faithful: parse the hex back out and confirm it equals the input.
    let round: Vec<u32> = rendered
        .lines()
        .filter_map(|l| l.trim().strip_prefix("[").map(|_| l))
        .filter_map(|l| l.rsplit("0x").next())
        .filter_map(|h| u32::from_str_radix(h.trim(), 16).ok())
        .collect();
    assert_eq!(round, words, "RAW hex must round-trip byte-for-byte");
}

#[test]
fn the_registered_o1_decoder_decodes_a_valid_head() {
    let registry = Registry::with_builtins();
    let dec = registry.get(0x0001).expect("O1 is registered");
    assert_eq!(dec.name(), "O1 observatory");
    let rendered = dec.render_header(&GOLDEN_HEAD).expect("valid HEAD decodes");
    assert!(rendered.contains("core_rev=f4f442d2"), "core_rev decoded: {rendered}");
    assert!(rendered.contains("ring=4096x4w"), "ring shape decoded: {rendered}");
}

#[test]
fn a_corrupt_head_is_a_decode_fault_naming_itself_not_a_guess() {
    let mut bad = GOLDEN_HEAD;
    bad[0] = 0x0000_0000; // scrub the HEAD magic
    let dec = O1Decoder;
    use lucid_host::decoder::Decoder;
    let err = dec.render_header(&bad).expect_err("a magic-less HEAD must refuse");
    assert_eq!(err.exit_code(), exit::DECODE);
}

// ---- HGT1: the render format is the stable contract ----

#[test]
fn the_o1_header_render_is_byte_stable() {
    // This exact string is what apf-host's parity credential compares against
    // o1_desk's render_header for the same Header.
    let h = o1host::Header::decode(&GOLDEN_HEAD).expect("decode");
    let expected = "core_rev=f4f442d2 ring=4096x4w policy=STOP armed=0 overflowed=0 \
                    events=0 dropped=0 rollovers=0\n  filter: ".to_string();
    let got = render::o1_header(&h);
    assert!(got.starts_with(&expected), "header render drifted:\n got: {got}\nwant: {expected}…");
}

// ---- D9: provenance is DERIVED, never typed ----

#[test]
fn provenance_is_derived_and_names_the_dep_stack() {
    let p = Provenance::current();
    assert!(!p.tool_rev.is_empty(), "tool rev is captured");
    // the dep stack, from the lockfile, names every composed crate
    let names: Vec<&str> = p.dep_pairs().map(|(n, _)| n).collect();
    for want in ["lucid-sld", "blaster2", "o1host", "lucid-trace"] {
        assert!(names.contains(&want), "dep stack must name {want}: {}", p.deps);
    }
    // has_local_path_dep is consistent with the deps string
    assert_eq!(p.has_local_path_dep(), p.deps.contains("LOCAL-PATH"));
}

// ---- D4 / HGT2: the container is versioned and carries every provenance field ----

#[test]
fn the_container_writer_is_versioned_and_complete() {
    let cap = Capture {
        provenance: Provenance::current(),
        run_mode: RunMode::Sim,
        instrument_id: 0x0001,
        instrument_version: 1,
        proto_version: 1,
        core_clock_hz: 74_250_000,
        seed: None,
        regions: vec![RegionPayload {
            id: 2,
            tag: "HEAD".into(),
            words: GOLDEN_HEAD.to_vec(),
        }],
    };
    let bytes = cap.to_bytes();
    let text = String::from_utf8(bytes).unwrap();

    // the version gate is the FIRST line (a headerless reader refuses; a v1
    // reader refuses a v2 file) — H2 credentials the refusal side.
    assert!(text.starts_with("LUCID-CAPTURE v1\n"), "version gate first: {text:.40}");
    for field in [
        "tool lucid-host ",
        "deps ",
        "run_mode SIM",
        "instrument 0x0001 version 1 proto 1",
        "timestamp_domain_hz 74250000",
        "seed none",
        "region 2 HEAD 16",
        "END",
    ] {
        assert!(text.contains(field), "container missing HGT2 field: {field}");
    }
}
