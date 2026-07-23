# PARITY.md — what is witnessed, and what is not

`lucid-host` renders captures as evidence. Evidence is only as good as what has
actually been observed, so this file states plainly what has been witnessed on
silicon and what has not. It is meant to be read before any capture is cited.

## Witnessed on silicon

- **The transport primitives.** The `Tap` over a USB-Blaster II cable, the SLD
  hub enumeration, the node walk, and the LIN instrument client (identify, region
  descriptors, drain) have been exercised on a real FPGA (an Analogue Pocket,
  Cyclone V) over a direct-attached clone cable. A single-node hub and a two-node
  hub have both enumerated correctly, including a device with **no** SLD hub
  refusing at the manufacturer check.

- **A single-device JTAG chain.** One device on the chain, read at its own IR
  length.

## NOT yet witnessed — do not cite as if it were

- **`lucid-host`'s own end-to-end drive of the O1 observatory on silicon.** The
  library composes primitives that are each silicon-proven, and the full flow is
  credentialed against a **simulated** transport (a byte-for-byte parity check
  run from the simulator's own crate). Re-witnessing that same flow on real
  silicon, through this tool, is a bench session and has not happened yet. Until
  it does, an O1 capture rendered by `lucid-host` is a composition of witnessed
  parts, not itself a silicon-witnessed measurement.

- **Multi-device JTAG chains.** Chains with more than one device require IR/DR
  padding around the target. That path is **unwitnessed** and ungated.

- **Non-clone cables.** Only a direct-attached clone cable has been used. Other
  USB-Blaster-family cables and adapters are unwitnessed.

## The standing rule

A measurement produced by a tool with no committed source is not evidence. This
tool exists so that every capture names the exact committed tool and the exact
composed crate revisions that produced it — printed on every run, and written
into every capture header. If a capture cannot name its tool and its revisions,
it is not evidence, whatever it shows.
