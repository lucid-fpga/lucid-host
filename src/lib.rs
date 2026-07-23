//! `lucid-host` — a bench host for LUCID instrument nodes.
//!
//! Attach to a live instrument over any [`Tap`], identify it, drive it, drain
//! it, and render evidence. The library is generic over the transport, so one
//! flow serves a real cable and a simulated one, and it never names a simulator
//! itself — a parity check runs from the sim's own crate, which owns it.
//!
//! The extension story and the honest scope are in `README.md` and `PARITY.md`.
#![forbid(unsafe_code)]

pub mod capture;
pub mod decoder;
pub mod error;
pub mod host;
pub mod provenance;
pub mod render;

pub use error::{HostError, Result};
pub use lucid_sld::tap::Tap;
pub use provenance::Provenance;

/// The crate version, for the provenance header.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
