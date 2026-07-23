//! Credentials — each watched failing before trusted (the negative half is
//! written, not implied). H1's boundary/decode/provenance/render credentials
//! plus H2's: the CTRL edges, the native container round-trip, the reader's
//! refusals both directions, and the diff both directions. The full flow over a
//! live transport (and the fabric-level CTRL idempotence) is credentialed from
//! the sim crate's side; the bench re-witnesses on silicon.

use lucid_host::capture::{Capture, RunMode};
use lucid_host::decoder::{Ctrl, Decoder, O1Decoder, Policy, Registry};
use lucid_host::error::{exit, HostError};
use lucid_host::{diff, render, Provenance};

/// A real pre-arm HEAD block captured from silicon — a genuine v1 block with
/// both magics present, so `Header::decode` accepts it.
const GOLDEN_HEAD: [u32; 16] = [
    0x4441_4548, 0x0000_0001, 0xF4F4_42D2, 0x0000_1000, 0x0000_0004, 0x0000_0000, 0x0000_0000,
    0x0000_0000, 0x0000_0000, 0x0000_0000, 0x0000_FFFF, 0x0000_0000, 0xFFFF_FFFF, 0x0000_0000,
    0x0000_0000, 0x4448_3149,
];

/// Craft an o1host RING event as its 4 words (kind!=0 so it is non-empty).
fn event_words(ts: u32, addr: u32, data: u32, kind: u8, flags: u8, seq: u32) -> [u32; 4] {
    [
        ts,
        addr,
        data,
        (u32::from(kind) << 28) | (u32::from(flags) << 20) | (seq & 0x000F_FFFF),
    ]
}

/// A small non-empty RING: two events, native-convertible.
fn sample_ring() -> Vec<u32> {
    let mut w = Vec::new();
    w.extend(event_words(100, 0xF800_0000, 0x0000_0082, 1, 0, 1));
    w.extend(event_words(204, 0x1000_0000, 0x0000_0010, 1, 0, 2));
    w
}

/// `GOLDEN_HEAD` with `event_count` and `write_index` set to `n` — so
/// `valid_events` knows how many of the ring's words belong to this capture.
fn head_with_count(n: u32) -> [u32; 16] {
    let mut h = GOLDEN_HEAD;
    h[13] = n; // EVENT_COUNT
    h[14] = n; // WRITE_INDEX
    h
}

/// A well-formed SUMM region: aggregate + a couple of histogram hits + two
/// LOCATED exceptions (the first is a large stall at byte offset 0x00400000).
fn sample_summ() -> Vec<u32> {
    use o1host::summ;
    let mut w = vec![0u32; summ::AGG_WORDS + summ::HIST_BUCKETS + 2 * summ::EXC_WORDS];
    w[summ::MAGIC] = o1host::MAGIC_SUMM;
    w[summ::MAGIC2] = o1host::MAGIC_SUMM2;
    w[summ::SUMMARY_VERSION] = 1;
    w[summ::GAP_MIN] = 40;
    w[summ::GAP_MAX] = 1_521_000; // a large stall, in ticks
    w[summ::GAP_SUM_LO] = 0x0000_2000;
    w[summ::WRITE_COUNT] = 8;
    w[summ::FIRST_ADDR] = 0x0000_0000;
    w[summ::LAST_ADDR] = 0x0000_001C;
    w[summ::NONSEQ_COUNT] = 0;
    w[summ::THRESHOLD] = 1024;
    w[summ::EXC_COUNT] = 2;
    w[summ::EXC_DROPPED] = 0;
    w[summ::HIST_BUCKET_COUNT] = summ::HIST_BUCKETS as u32;
    w[summ::EXC_CAPACITY] = summ::EXC_ENTRIES as u32;
    w[summ::AGG_WORDS + 100] = 5; // two nonzero histogram buckets
    w[summ::AGG_WORDS + 180] = 1;
    let e0 = summ::AGG_WORDS + summ::HIST_BUCKETS;
    w[e0 + summ::E_GAP] = 1_521_000;
    w[e0 + summ::E_ADDR] = 0x0040_0000; // LOCATED here
    w[e0 + summ::E_ORDINAL] = 6;
    w[e0 + summ::E_SEQ] = 6;
    let e1 = e0 + summ::EXC_WORDS;
    w[e1 + summ::E_GAP] = 2048;
    w[e1 + summ::E_ADDR] = 0x0040_0004;
    w[e1 + summ::E_ORDINAL] = 7;
    w[e1 + summ::E_SEQ] = 7;
    w
}

fn sample_capture() -> Capture {
    let (schema, records) = O1Decoder
        .to_records(o1host::REGION_RING, &sample_ring(), &head_with_count(2))
        .expect("O1 RING converts to native records");
    let header = O1Decoder.header_summary(&GOLDEN_HEAD);
    Capture::new(RunMode::Sim, 0x0001, 1, 1, 74_250_000, None, header, None, schema, records)
}

// ---- the fix: only THIS capture's valid events, never stale ring words ----

#[test]
fn the_container_records_only_valid_events_never_stale_ring_words() {
    // Reproduce the bench finding at the desk: a ring where a CLEAR reset
    // the counters to N=2 and 2 NEW events were written at positions 0..2,
    // OVERWRITING the first 2 of a previous capture — but the previous capture's
    // events at positions 2..5 LINGER (CLEAR does not scrub the ring words).
    let mut polluted = Vec::new();
    polluted.extend(event_words(500, 0x0000_0000, 0x0000_0082, 1, 0, 10)); // new #1
    polluted.extend(event_words(604, 0x1000_0000, 0x0000_0010, 1, 0, 11)); // new #2
    polluted.extend(event_words(100, 0xF800_0000, 0x0000_00AA, 1, 0, 1)); // stale
    polluted.extend(event_words(204, 0xF800_0004, 0x0000_00BB, 1, 0, 2)); // stale
    polluted.extend(event_words(308, 0xF800_0008, 0x0000_00CC, 1, 0, 3)); // stale
    let header = head_with_count(2); // the fabric says: 2 events this capture

    // OLD behaviour (decode_all over the whole region) OVER-COUNTS — the disease:
    let over = o1host::Event::decode_all(&polluted)
        .iter()
        .filter(|e| !e.is_empty())
        .count();
    assert_eq!(over, 5, "decode_all sees the stale words too — the pollution");

    // THE FIX: to_records records only the header's valid_events == 2, and they
    // are exactly the NEW events (seq 10, 11), not the stale ones.
    let (_schema, records) = O1Decoder
        .to_records(o1host::REGION_RING, &polluted, &header)
        .expect("RING converts");
    assert_eq!(records.len(), 2, "record count EQUALS the header event_count");
    // seq is packed at payload bits [76..96]
    let seqs: Vec<u32> = records.iter().map(|r| (r.payload >> 76) as u32 & 0xF_FFFF).collect();
    assert_eq!(seqs, vec![10, 11], "the records are exactly THIS capture's new events");
}

#[test]
fn a_fresh_ring_capture_is_unaffected_by_the_fix() {
    // capture-1 class: event_count == the ring's real contents, no stale words.
    let (_s, records) = O1Decoder
        .to_records(o1host::REGION_RING, &sample_ring(), &head_with_count(2))
        .expect("RING converts");
    assert_eq!(records.len(), 2, "a fresh-ring count is unchanged by the fix");
}

// ---- SUMM: the cadence renders, and every stall is LOCATED and carried ----

#[test]
fn the_summ_region_renders_min_mean_max_and_locates_each_exception() {
    let r = O1Decoder.render_region(o1host::REGION_SUMM, &sample_summ(), &GOLDEN_HEAD);
    assert!(r.contains("writes=8"), "the render states the write count: {r}");
    assert!(r.contains("gap[min/mean/max]"), "min/mean/max are rendered: {r}");
    assert!(r.contains("histogram"), "the histogram the mean hides is rendered: {r}");
    // the load-bearing fact: the stall is LOCATED at its byte offset in the render
    assert!(r.contains("0x00400000"), "the exception is LOCATED at its offset: {r}");
    assert!(r.contains("exceptions: 2 logged"), "both exceptions are shown: {r}");
}

#[test]
fn summ_exceptions_convert_to_native_records_at_their_offsets() {
    let (schema, recs) = O1Decoder
        .to_records(o1host::REGION_SUMM, &sample_summ(), &GOLDEN_HEAD)
        .expect("a SUMM with exceptions yields native records");
    assert_eq!(recs.len(), 2, "one native record per logged exception");
    assert!(recs.iter().all(|r| r.tag == 2), "an exception record is tag 2, not a ring event");
    // the located byte offset survives into the payload (addr at bits [32..64])
    assert_eq!((recs[0].payload >> 32) as u32, 0x0040_0000, "the stall's byte offset is in the record");
    // and it decodes back through lucid-trace with no O1-specific code (HGT6)
    let rec = lucid_trace::decode_record(&schema, &recs[0]).expect("native decode");
    assert!(rec.get("gap").is_some(), "the exception record names its gap downstream");
}

#[test]
fn a_reserved_summ_region_yields_no_records_and_names_itself() {
    // summary_version 0 is the RESERVED, not-implemented state — the decoder must
    // not manufacture a cadence from it, and must say so rather than guess.
    let mut w = sample_summ();
    w[o1host::summ::SUMMARY_VERSION] = 0;
    assert!(
        O1Decoder.to_records(o1host::REGION_SUMM, &w, &GOLDEN_HEAD).is_none(),
        "a reserved SUMM contributes no records"
    );
    let r = O1Decoder.render_region(o1host::REGION_SUMM, &w, &GOLDEN_HEAD);
    assert!(r.contains("undecoded"), "a reserved block names itself, not a guess: {r}");
}

#[test]
fn a_capture_carries_the_summ_digest_and_every_exception_round_trips() {
    // Built the way host::capture composes it: ring events AND the SUMM located
    // exceptions in ONE native payload, plus the SUMM digest in the header.
    let (schema, mut records) = O1Decoder
        .to_records(o1host::REGION_RING, &sample_ring(), &head_with_count(2))
        .expect("RING converts");
    let (_s, mut exc) = O1Decoder
        .to_records(o1host::REGION_SUMM, &sample_summ(), &GOLDEN_HEAD)
        .expect("SUMM converts");
    records.append(&mut exc);
    let summary = O1Decoder.summary(o1host::REGION_SUMM, &sample_summ());
    assert!(summary.is_some(), "the decoder digests the SUMM aggregate for the header");
    let cap = Capture::new(
        RunMode::Sim, 0x0001, 1, 1, 74_250_000, None,
        O1Decoder.header_summary(&GOLDEN_HEAD), summary, schema, records,
    );

    let back = Capture::read(&cap.to_bytes()).expect("a SUMM-carrying capture round-trips");
    assert_eq!(back.summary, cap.summary, "the SUMM digest survives the container");
    assert!(back.summary.as_deref().unwrap().contains("writes=8"), "the digest carries the cadence");
    assert_eq!(back.records.len(), 4, "2 ring events + 2 located exceptions");
    assert_eq!(
        back.records.iter().filter(|r| r.tag == 2).count(),
        2,
        "both located exceptions survive as native, re-decodable records"
    );
}

// ---- refuse-before-use maps to typed errors that NAME the mismatch ----

#[test]
fn hub_less_device_is_refused_naming_the_0x7ff_signature() {
    let err: HostError = lucid_sld::Error::Manufacturer { expected: 0x06E, got: 0x7FF }.into();
    assert_eq!(err.exit_code(), exit::REFUSED);
    assert!(err.to_string().contains("0x7FF"));
    assert!(err.to_string().contains("not Altera"));
}

#[test]
fn a_shift_width_overflow_is_a_transport_fault() {
    let err: HostError = lucid_sld::Error::Width { bits: 80, max: 64 }.into();
    assert_eq!(err.exit_code(), exit::TRANSPORT);
}

#[test]
fn exit_codes_are_distinct_per_class() {
    let codes = [exit::OK, exit::REFUSED, exit::TRANSPORT, exit::DECODE, exit::USAGE];
    let mut seen = codes.to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), codes.len());
}

// ---- decoder: unknown → RAW+UNDECODED byte-faithful; known → decodes ----

#[test]
fn an_unregistered_instrument_renders_raw_and_the_bytes_round_trip() {
    let registry = Registry::empty();
    assert!(registry.get(0x0001).is_none());
    let words: Vec<u32> = vec![0xDEAD_BEEF, 0x0000_0000, 0x1234_5678, 0xFFFF_FFFF];
    let rendered = render::raw_region(0, &words);
    assert!(rendered.contains("UNDECODED"));
    let round: Vec<u32> = rendered
        .lines()
        .filter(|l| l.trim_start().starts_with('['))
        .filter_map(|l| l.rsplit("0x").next())
        .filter_map(|h| u32::from_str_radix(h.trim(), 16).ok())
        .collect();
    assert_eq!(round, words);
}

#[test]
fn the_registered_o1_decoder_decodes_a_valid_head() {
    let dec = Registry::with_builtins().get(0x0001).map(|d| d.name());
    assert_eq!(dec, Some("O1 observatory"));
    let rendered = O1Decoder.render_header(&GOLDEN_HEAD).expect("valid HEAD decodes");
    assert!(rendered.contains("core_rev=f4f442d2"));
    assert!(rendered.contains("ring=4096x4w"));
}

#[test]
fn a_corrupt_head_is_a_decode_fault_not_a_guess() {
    let mut bad = GOLDEN_HEAD;
    bad[0] = 0;
    assert_eq!(O1Decoder.render_header(&bad).unwrap_err().exit_code(), exit::DECODE);
}

#[test]
fn the_o1_header_render_is_byte_stable() {
    let h = o1host::Header::decode(&GOLDEN_HEAD).unwrap();
    let want = "core_rev=f4f442d2 ring=4096x4w policy=STOP armed=0 overflowed=0 \
                events=0 dropped=0 rollovers=0\n  filter: ";
    assert!(render::o1_header(&h).starts_with(want));
}

// ---- CTRL: levels, not pulses (the host emits an edge per write) ----

#[test]
fn ctrl_arm_twice_produces_two_edges_both_setting_the_level() {
    // Arm is a LEVEL bit; two consecutive arms must both set it, and each must
    // carry a DIFFERENT nonce so the fabric sees two edges (not one). The host's
    // contribution to idempotence; the fabric-level "armed stays 1" is the sim
    // crate's re-witness.
    let (w0, n0) = O1Decoder.ctrl_words(&Ctrl::Arm, false).unwrap();
    let (w1, _n1) = O1Decoder.ctrl_words(&Ctrl::Arm, n0).unwrap();
    let arm = 1u64 << o1host::ctrl_bit::ARM;
    let nonce = 1u64 << o1host::ctrl_bit::NONCE;
    assert_eq!(w0.len(), 1);
    assert_eq!(w1.len(), 1);
    assert!(w0[0] & arm != 0 && w1[0] & arm != 0, "both set the ARM level");
    assert_ne!(w0[0] & nonce, w1[0] & nonce, "consecutive writes toggle the nonce edge");
}

#[test]
fn filter_is_three_writes_and_policy_encodes_stop_vs_wrap() {
    let (words, _) = O1Decoder
        .ctrl_words(&Ctrl::Filter { kind_mask: 0x000A, addr_lo: 0, addr_hi: u32::MAX }, false)
        .unwrap();
    assert_eq!(words.len(), 3, "mask + lo + hi are three CTRL writes");
    let (stop, _) = O1Decoder.ctrl_words(&Ctrl::Policy(Policy::Stop), false).unwrap();
    let (wrap, _) = O1Decoder.ctrl_words(&Ctrl::Policy(Policy::Wrap), false).unwrap();
    let val = 1u64 << o1host::ctrl_bit::POLICY_VAL;
    assert_eq!(stop[0] & val, 0, "STOP clears POLICY_VAL");
    assert_ne!(wrap[0] & val, 0, "WRAP sets POLICY_VAL");
}

#[test]
fn threshold_is_one_write_with_ticks_in_the_payload_and_is_idempotent() {
    // THRESH is a config latch that survives CLEAR: ONE CTRL word, bit 13 set,
    // the tick count carried in the payload (CTRL[63:32]). The host's job is the
    // correct word; the fabric-level "log only the outlier vs overflow at 1024"
    // is the sim crate's re-witness (both directions).
    let (w0, n0) = O1Decoder.ctrl_words(&Ctrl::Threshold(1_000_000), false).unwrap();
    let (w1, _n1) = O1Decoder.ctrl_words(&Ctrl::Threshold(1_000_000), n0).unwrap();
    let thresh = 1u64 << o1host::ctrl_bit::THRESH;
    let nonce = 1u64 << o1host::ctrl_bit::NONCE;
    assert_eq!(w0.len(), 1, "threshold is a single CTRL write");
    assert!(w0[0] & thresh != 0, "the THRESH bit is set");
    assert_eq!(
        w0[0] >> o1host::ctrl_bit::PAYLOAD_LSB,
        1_000_000,
        "the tick count rides the payload (CTRL[63:32])"
    );
    // both directions encode their own value, no clamping: the raised ~1M and the
    // default 1024 each land verbatim.
    let (dflt, _) = O1Decoder.ctrl_words(&Ctrl::Threshold(1024), false).unwrap();
    assert_eq!(dflt[0] >> o1host::ctrl_bit::PAYLOAD_LSB, 1024, "default 1024 encodes verbatim");
    // idempotent at the host level: a repeated identical set still toggles the
    // nonce so the fabric sees a fresh edge.
    assert_ne!(w0[0] & nonce, w1[0] & nonce, "a repeated set toggles the nonce edge");
}

#[test]
fn an_instrument_with_no_decoder_refuses_ctrl() {
    // the trait default: no CTRL surface -> Refused
    struct Bare;
    impl Decoder for Bare {
        fn instrument_id(&self) -> u16 { 0xFFFF }
        fn name(&self) -> &'static str { "bare" }
        fn render_header(&self, _: &[u32]) -> Result<String, HostError> { Ok(String::new()) }
        fn render_region(&self, _: u8, _: &[u32], _: &[u32]) -> String { String::new() }
        fn header_region(&self) -> u8 { 0 }
    }
    assert_eq!(
        Bare.ctrl_words(&Ctrl::Arm, false).unwrap_err().exit_code(),
        exit::REFUSED
    );
}

// ---- the native container: round-trip, and the refusals both directions ----

#[test]
fn the_container_round_trips_through_native_lucid_trace() {
    let cap = sample_capture();
    let bytes = cap.to_bytes();
    assert!(bytes.starts_with(b"LUCID-CAPTURE v1\n"), "version gate first");

    let back = Capture::read(&bytes).expect("a well-formed capture reads");
    assert_eq!(back.run_mode, RunMode::Sim);
    assert_eq!(back.instrument_id, 0x0001);
    assert_eq!(back.core_clock_hz, 74_250_000);
    assert_eq!(back.records.len(), cap.records.len());
    for (a, b) in cap.records.iter().zip(back.records.iter()) {
        assert_eq!((a.tag, a.timestamp, a.payload, a.payload_bits),
                   (b.tag, b.timestamp, b.payload, b.payload_bits), "native payload survives");
    }
}

#[test]
fn a_headerless_file_is_refused_at_read() {
    let err = Capture::read(b"just some bytes, no capture header\n").unwrap_err();
    assert_eq!(err.exit_code(), exit::REFUSED);
    assert!(err.to_string().contains("version gate"));
}

#[test]
fn a_stripped_required_field_is_refused_naming_it() {
    let bytes = sample_capture().to_bytes();
    // remove the run_mode line (it sits in the text header, before the payload)
    let needle = b"run_mode SIM\n";
    let at = bytes.windows(needle.len()).position(|w| w == needle).expect("run_mode line present");
    let mut stripped = bytes.clone();
    stripped.drain(at..at + needle.len());
    let err = Capture::read(&stripped).unwrap_err();
    assert_eq!(err.exit_code(), exit::REFUSED);
    assert!(err.to_string().contains("run_mode"), "refusal names the missing field: {err}");
}

#[test]
fn a_v2_stamped_file_is_refused_loudly_by_the_v1_reader() {
    let mut bytes = sample_capture().to_bytes();
    let (v1, v2) = (b"LUCID-CAPTURE v1\n".as_slice(), b"LUCID-CAPTURE v2\n".as_slice());
    bytes.splice(0..v1.len(), v2.iter().copied());
    let err = Capture::read(&bytes).unwrap_err();
    assert_eq!(err.exit_code(), exit::REFUSED);
    assert!(err.to_string().contains("version 2"), "names the unsupported version: {err}");
}

// ---- diff, both directions, first divergence located ----

#[test]
fn diff_of_the_same_capture_is_empty() {
    let bytes = sample_capture().to_bytes();
    let d = diff::diff_bytes(&bytes, &bytes).expect("both read");
    assert!(d.is_empty(), "identical captures diff empty");
    assert!(diff::render(&d).contains("IDENTICAL"));
}

#[test]
fn a_perturbed_copy_diffs_nonempty_with_the_divergence_located() {
    let a = sample_capture();
    // perturb the SECOND event's data
    let mut b = a.clone();
    b.records[1].payload ^= 0xFF;
    let d = diff::diff(&a, &b);
    assert!(!d.is_empty());
    let ev = d.first_event.expect("a located event divergence");
    assert_eq!(ev.index, 1, "the divergence is located at the injected position");
}

#[test]
fn diff_inherits_the_read_refusals() {
    let good = sample_capture().to_bytes();
    let err = diff::diff_bytes(b"not a capture\n", &good).unwrap_err();
    assert_eq!(err.exit_code(), exit::REFUSED);
}

// ---- provenance is derived ----

#[test]
fn provenance_is_derived_and_names_the_dep_stack() {
    let p = Provenance::current();
    assert!(!p.tool_rev.is_empty());
    let names: Vec<&str> = p.dep_pairs().map(|(n, _)| n).collect();
    for want in ["lucid-sld", "blaster2", "o1host", "lucid-trace"] {
        assert!(names.contains(&want), "dep stack names {want}: {}", p.deps);
    }
}
