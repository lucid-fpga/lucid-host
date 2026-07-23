//! Typed errors and the process exit codes they map to (D7: built to be
//! scripted — a caller branches on the exit, never on parsing prose).

use thiserror::Error;

/// Exit codes are a stable contract (D7). A script distinguishes a refusal
/// (the tool worked, the device/input was wrong) from a transport fault (the
/// cable/sim failed) from a decode fault (the bytes were malformed) without
/// reading a word of output.
pub mod exit {
    /// Everything the command was asked to do succeeded.
    pub const OK: i32 = 0;
    /// A boundary refusal (HGT4): proto/magic/manufacturer/region-bounds/
    /// decoder-id mismatch, or a headerless capture. The tool is fine; the
    /// device or the input is not what was claimed.
    pub const REFUSED: i32 = 2;
    /// The transport failed: the cable would not open, a shift errored, the
    /// sim faulted. Nothing was concluded about the instrument.
    pub const TRANSPORT: i32 = 3;
    /// The bytes were read but would not decode: a malformed header, a short
    /// region. Distinct from REFUSED because the boundary check passed and the
    /// payload is the problem.
    pub const DECODE: i32 = 4;
    /// The command line could not be understood.
    pub const USAGE: i32 = 64;
}

/// Everything that can go wrong driving an instrument, typed so the bin maps
/// each to its [`exit`] code and the message names the mismatch (HGT4).
#[derive(Debug, Error)]
pub enum HostError {
    /// A refuse-before-use boundary tripped; the string names what mismatched.
    #[error("refused: {0}")]
    Refused(String),
    /// The transport layer (cable/sim) failed.
    #[error("transport: {0}")]
    Transport(String),
    /// Read bytes would not decode.
    #[error("decode: {0}")]
    Decode(String),
    /// The command line was malformed.
    #[error("usage: {0}")]
    Usage(String),
}

impl HostError {
    /// The process exit code for this error (D7).
    pub fn exit_code(&self) -> i32 {
        match self {
            HostError::Refused(_) => exit::REFUSED,
            HostError::Transport(_) => exit::TRANSPORT,
            HostError::Decode(_) => exit::DECODE,
            HostError::Usage(_) => exit::USAGE,
        }
    }
}

/// Map a lucid-sld error to a host error, preserving the boundary/transport
/// distinction: a manufacturer/protocol mismatch is a *refusal* (HGT4), a
/// width or shift failure is a *transport* fault.
impl From<lucid_sld::Error> for HostError {
    fn from(e: lucid_sld::Error) -> Self {
        use lucid_sld::Error as E;
        match e {
            E::Manufacturer { expected, got } => HostError::Refused(format!(
                "hub manufacturer 0x{got:03X} is not Altera 0x{expected:03X} \
                 (a device with no SLD hub reads 0x7FF)"
            )),
            E::Protocol(m) => HostError::Refused(format!("protocol: {m}")),
            E::Width { bits, max } => {
                HostError::Transport(format!("shift width {bits} exceeds max {max}"))
            }
            E::Transport(m) => HostError::Transport(m),
        }
    }
}

/// A convenience alias for host results.
pub type Result<T> = std::result::Result<T, HostError>;
