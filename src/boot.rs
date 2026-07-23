//! OGT2 — the after-the-fact boot decoder.
//!
//! Written FROM THE CAPTURES ALONE: the only inputs are a container's raw records
//! (the v1 event format, unchanged) and the platform's documented command
//! vocabulary. It reproduces the boot handshake — the mailbox commands with their
//! parameter blocks, the per-slot descriptor table, and the payload deliveries —
//! and so discharges observation-first's promise: that the raw record preserved
//! enough to decode later, without new instrumentation or a format change.
//!
//! The addresses are the platform's literal mailbox map, from its documented
//! developer interface: the command word at `0xF8000000` carries the `'CM'` tag
//! in its high half; the parameter block is the four words
//! `0xF8000020..0xF800002C`; the descriptor table is
//! `0xF8002000 + slot_id*8 -> [slot_id, size]`; a slot's payload lands at its own
//! address nibble.
//!
//! SPDX-License-Identifier: MIT OR Apache-2.0

use lucid_trace::{decode_record, FieldValue, RawRecord, Schema};

/// The `'CM'` tag in the high half of a command word (`0x434D` = b"CM").
const CM_TAG: u32 = 0x434D;
/// The command mailbox base — the command word address.
const CMD_WORD: u32 = 0xF800_0000;
/// The parameter block: `0xF8000020..0xF800002C`, four words.
const PARAM_BASE: u32 = 0xF800_0020;
/// The dataslot descriptor table base (`0xF8002000 + slot_id*8`).
const DESC_BASE: u32 = 0xF800_2000;
/// The top of the descriptor table region examined (`slot_id*8` for a few slots).
const DESC_TOP: u32 = 0xF800_2100;

/// The documented Host→Target command vocabulary. A code with
/// no documented name is rendered as unknown rather than guessed.
pub fn command_name(code: u16) -> &'static str {
    match code {
        0x0000 => "Request Status",
        0x0010 => "Reset Enter",
        0x0011 => "Reset Exit",
        0x0080 => "Data Slot Request Read",
        0x0082 => "Data Slot Request Write",
        0x008A => "Data Slot Update",
        0x008F => "Data Slot Access Complete",
        0x0090 => "Real-time Clock Data",
        0x00A0 => "Savestate: Start/Query",
        0x00A4 => "Savestate: Load/Query",
        0x00B0 => "OS Notify: Menu State",
        0x00B1 => "OS Notify: Cartridge Adapter",
        0x00B2 => "OS Notify: Docked State",
        0x00B8 => "OS Notify: Display Mode",
        _ => "(undocumented)",
    }
}

/// One decoded mailbox command: the `'CM'` command word and the parameter block
/// latched before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxCommand {
    /// The recorder sequence number of the command word.
    pub seq: u32,
    /// The recorder timestamp of the command word, in core clocks.
    pub timestamp: u32,
    /// The command code (`0x00XX`), the low half of the `'CM'` word.
    pub code: u16,
    /// The parameter block `0xF8000020..0xF800002C`, as latched when the command
    /// landed.
    pub params: [u32; 4],
}

/// One dataslot descriptor: `0xF8002000 + slot_id*8 -> [slot_id, size]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    pub slot_id: u32,
    pub size: u32,
    /// The recorder sequence of the id write.
    pub seq: u32,
}

/// A payload delivery: a run of writes at a slot's base address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    /// The slot's base address (its address nibble, e.g. `0x00000000`,
    /// `0x10000000`, `0x20000000`).
    pub base: u32,
    /// The number of payload writes observed in the run.
    pub writes: u32,
    /// The recorder sequence of the first payload write.
    pub first_seq: u32,
}

/// The decoded boot account: the handshake, the descriptor table, the deliveries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootRecord {
    pub commands: Vec<MailboxCommand>,
    pub descriptors: Vec<Descriptor>,
    pub deliveries: Vec<Delivery>,
}

fn field_u32(rec: &lucid_trace::Record, name: &str) -> Option<u32> {
    match rec.get(name)? {
        FieldValue::U(x) | FieldValue::Hex(x) | FieldValue::Enum(x) => u32::try_from(*x).ok(),
        _ => None,
    }
}

/// OGT2: decode the boot account from the raw records ALONE, through the
/// container's own schema. The command parameter block is latched as its words
/// are written and snapshotted onto each `'CM'` command; descriptor writes are
/// paired id→size; contiguous writes to a non-mailbox address are grouped into a
/// delivery run.
pub fn decode(records: &[RawRecord], schema: &Schema) -> BootRecord {
    let mut params = [0u32; 4];
    let mut rec = BootRecord::default();
    // pending descriptor id write, awaiting its size write
    let mut pending_desc: Option<(u32, u32, u32)> = None; // (table_addr, slot_id, seq)
    // current delivery run
    let mut run: Option<Delivery> = None;

    for raw in records {
        let Ok(d) = decode_record(schema, raw) else { continue };
        // a ring event is exactly the record that carries addr+data+seq; a SUMM
        // exception carries `gap` instead of `data`, so this selects events with no
        // dependence on the schema's record NAME (older containers differ).
        let (Some(addr), Some(data), Some(seq)) = (
            field_u32(&d, "addr"),
            field_u32(&d, "data"),
            field_u32(&d, "seq"),
        ) else {
            continue;
        };

        // a delivery run ends the moment a write leaves the slot's payload stream
        // (a mailbox/descriptor write, or a different slot's base nibble)
        if let Some(cur) = &run {
            let continues = addr < 0xF000_0000 && (addr & 0xF000_0000) == cur.base;
            if !continues {
                rec.deliveries.push(run.take().unwrap());
            }
        }

        match addr {
            a if (PARAM_BASE..PARAM_BASE + 16).contains(&a) && a % 4 == 0 => {
                params[((a - PARAM_BASE) / 4) as usize] = data;
            }
            CMD_WORD if (data >> 16) == CM_TAG => {
                rec.commands.push(MailboxCommand {
                    seq,
                    timestamp: raw.timestamp,
                    code: (data & 0xFFFF) as u16,
                    params,
                });
            }
            a if (DESC_BASE..DESC_TOP).contains(&a) => {
                // even offset = id write, odd (id+4) = size write
                if (a - DESC_BASE) % 8 == 0 {
                    pending_desc = Some((a, data, seq));
                } else if let Some((id_addr, slot_id, dseq)) = pending_desc.take() {
                    if a == id_addr + 4 {
                        rec.descriptors.push(Descriptor { slot_id, size: data, seq: dseq });
                    }
                }
            }
            a if a < 0xF000_0000 => {
                // a payload write; extend or open a delivery run at this base nibble
                let base = a & 0xF000_0000;
                match &mut run {
                    Some(cur) if cur.base == base => cur.writes += 1,
                    _ => {
                        run = Some(Delivery { base, writes: 1, first_seq: seq });
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(cur) = run.take() {
        rec.deliveries.push(cur);
    }
    rec
}
