//! Transport-neutral embedding facade for the RIGA agent.
//!
//! A host — Tauri IPC, an HTTP route, a CLI — owns one [`Agent`] and maps its
//! own command surface onto these methods. Nothing in this module knows about a
//! transport, so the same agent can be embedded anywhere and the host's glue
//! stays small. Everything a host needs to embed RIGA should live behind this
//! type rather than being re-declared per transport.

use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;

/// A running RIGA kernel agent.
///
/// Construct one per host, keep it in the host's shared state, and forward
/// commands to it. Because it is `Copy + Send + Sync`, a host can store it in
/// Tauri's managed state, behind an `Arc`, or inline without ceremony.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Agent {
    protocol_version: u16,
}

impl Default for Agent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent {
    /// Create an agent speaking the current wire protocol version.
    pub fn new() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
        }
    }

    /// Health payload a host returns from its own health command or route.
    pub fn health(&self) -> Health {
        Health {
            protocol_version: self.protocol_version,
        }
    }
}

/// Serializable health payload shared by every transport.
///
/// Declared once here so a Tauri command and an HTTP route cannot drift into
/// returning different shapes for the same probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub protocol_version: u16,
}

#[cfg(test)]
mod tests {
    use super::Agent;
    use crate::PROTOCOL_VERSION;

    #[test]
    fn agent_reports_the_current_protocol_version() {
        let agent = Agent::new();
        assert_eq!(agent.health().protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn health_serializes_to_the_wire_shape_hosts_depend_on() {
        // Hosts and the desktop UI both read `protocol_version`, so the field
        // name is part of the contract and is pinned here.
        let json = serde_json::to_string(&Agent::new().health()).unwrap();
        assert_eq!(json, r#"{"protocol_version":1}"#);
    }
}
