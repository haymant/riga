#![doc = "Transport-free RIGA coding-agent kernel."]

/// Current public wire protocol version.
pub const PROTOCOL_VERSION: u16 = 1;

pub mod events;
pub mod rig_compat;

/// Minimal health marker used by the Phase 0 compatibility scaffold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelVersion {
    pub protocol_version: u16,
}

impl Default for KernelVersion {
    fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
        }
    }
}
