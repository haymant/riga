use std::sync::Arc;

use riga_kernel::KernelVersion;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct RigaState {
    pub kernel_version: Arc<KernelVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub protocol_version: u16,
}

pub fn build_state() -> RigaState {
    RigaState {
        kernel_version: Arc::new(KernelVersion::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::{HealthResponse, build_state};

    #[test]
    fn desktop_state_exposes_the_kernel_protocol_version() {
        let state = build_state();
        let response = HealthResponse {
            protocol_version: state.kernel_version.protocol_version,
        };
        assert_eq!(response.protocol_version, 1);
        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"protocol_version":1}"#
        );
    }
}
