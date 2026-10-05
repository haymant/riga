#![doc = "Transport-free RIGA coding-agent kernel."]

/// Current public wire protocol version.
pub const PROTOCOL_VERSION: u16 = 1;

pub mod agent;
pub mod events;
pub mod persistence;
pub mod policy;
pub mod rig_compat;
pub mod state;
pub mod task;

pub use agent::{Agent, Health};
