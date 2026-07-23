//! OGT2 — the after-the-fact boot decoder, credentialed against the platform's
//! own log.
//!
//! The decoder (`lucid_host::boot`) is written from the captures ALONE — the v1
//! raw record and the documented command vocabulary, no new instrumentation and
//! no format change. This credential is the promise observation-first made coming
//! due: the decoded boot handshake reproduces the sequence the Pocket's OWN OS log
//! recorded, event-for-event where they overlap. A raw record that could not be
//! decoded later would be OGT2 failing — reported as the finding it is, not
//! patched around. It decodes; the promise holds.

use lucid_host::boot;
use lucid_host::capture::Capture;

/// The banked boot capture (digest `a94f6540`): the SD-launched boot of the O1
/// core. Committed as a fixture so the credential is reproducible from a clean
/// clone.
const CAP1_BOOT: &[u8] = include_bytes!("data/cap1-boot.lhc");

/// The Pocket's OWN command account for this boot, from its OS log, in order,
/// over the window the capture's ring covers (the log continues past the ring
/// into later menu activity):
///   Request Status · OS Notify Cartridge Adapter · Data Slot Request Write ×3
///   (slots 0,1,2) · Data Slot Access Complete · Real-time Clock · OS Notify
///   Docked State · Reset Exit · OS Notify Display Mode · OS Notify Menu State.
const OS_LOG_COMMANDS: &[u16] = &[
    0x0000, 0x00B1, 0x0082, 0x0082, 0x0082, 0x008F, 0x0090, 0x00B2, 0x0011, 0x00B8, 0x00B0,
];

#[test]
fn ogt2_decodes_the_boot_handshake_matching_the_os_log() {
    let cap = Capture::read(CAP1_BOOT).expect("the boot capture reads");
    let rec = boot::decode(&cap.records, &cap.schema);

    // THE CREDENTIAL: the decoded command sequence reproduces the platform's own
    // log, event-for-event, from the raw records alone.
    let codes: Vec<u16> = rec.commands.iter().map(|c| c.code).collect();
    assert_eq!(
        codes, OS_LOG_COMMANDS,
        "the decoded handshake matches the Pocket's OS log event-for-event"
    );

    // every command decodes to a documented name (nothing is an unnamed guess).
    assert!(
        rec.commands.iter().all(|c| boot::command_name(c.code) != "(undocumented)"),
        "every command in the boot handshake is in the documented vocabulary"
    );

    // the three Request Writes carry the slot ids and sizes the log records:
    // slot 0 (4 KiB), slot 1 (4 KiB), slot 2 (4 MiB).
    let writes: Vec<(u32, u32)> = rec
        .commands
        .iter()
        .filter(|c| c.code == 0x0082)
        .map(|c| (c.params[0], c.params[1]))
        .collect();
    assert_eq!(
        writes,
        vec![(0, 0x1000), (1, 0x1000), (2, 0x0040_0000)],
        "the Request Write params are slot id + size, matching the log"
    );

    // the per-slot descriptor table, reconstructed: id -> size.
    let descs: Vec<(u32, u32)> = rec.descriptors.iter().map(|d| (d.slot_id, d.size)).collect();
    assert_eq!(
        descs,
        vec![(0, 0x1000), (1, 0x1000), (2, 0x0040_0000)],
        "the descriptor table maps slot_id -> size at 0xF8002000 + slot_id*8"
    );

    // slots 0 and 1 delivered their 4 KiB (1024 writes each); slot 2 (4 MiB) was
    // announced but DEFERRED — no payload run at its base (the log says exactly so).
    let deliveries: Vec<(u32, u32)> = rec
        .deliveries
        .iter()
        .filter(|d| d.writes > 1)
        .map(|d| (d.base, d.writes))
        .collect();
    assert_eq!(
        deliveries,
        vec![(0x0000_0000, 1024), (0x1000_0000, 1024)],
        "slots 0 and 1 delivered 4 KiB; the deferred 4 MiB slot has no payload run"
    );
}
