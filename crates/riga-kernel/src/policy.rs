use serde::{Deserialize, Serialize};

use crate::state::{RigaError, RigaErrorCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolRisk {
    ReadOnly,
    WorkspaceWrite,
    ProcessExecution,
    NetworkAccess,
    Destructive,
    /// A change to the run's own capabilities, such as granting a subagent a
    /// tool it does not have by default. Requires approval, but is not workspace
    /// work.
    CapabilityChange,
}

impl ToolRisk {
    /// Classify a built-in tool by the risk it carries. Unknown tools (MCP and
    /// future tools) are treated as read-only: they are not workspace
    /// mutations, and the transport is local.
    pub fn for_tool(name: &str) -> Self {
        match name {
            "write" | "edit" | "apply_patch" => Self::WorkspaceWrite,
            "bash" | "shell" => Self::ProcessExecution,
            "web" | "webfetch" | "websearch" => Self::NetworkAccess,
            "grant_tools" => Self::CapabilityChange,
            _ => Self::ReadOnly,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRequest {
    pub tool_name: String,
    pub risk: ToolRisk,
    pub target: String,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    Allow,
    RequireApproval,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub allow_read_only: bool,
    pub allow_workspace_writes: bool,
    pub allow_process_execution: bool,
    pub allow_network: bool,
    pub allow_destructive: bool,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            allow_read_only: true,
            allow_workspace_writes: false,
            allow_process_execution: false,
            allow_network: false,
            allow_destructive: false,
        }
    }
}

impl ToolPolicy {
    pub fn evaluate(&self, request: &ToolRequest) -> ApprovalDecision {
        match request.risk {
            ToolRisk::ReadOnly if self.allow_read_only => ApprovalDecision::Allow,
            ToolRisk::WorkspaceWrite if self.allow_workspace_writes => ApprovalDecision::Allow,
            ToolRisk::ProcessExecution if self.allow_process_execution => ApprovalDecision::Allow,
            ToolRisk::NetworkAccess if self.allow_network => ApprovalDecision::Allow,
            ToolRisk::Destructive if self.allow_destructive => ApprovalDecision::Allow,
            ToolRisk::WorkspaceWrite
            | ToolRisk::ProcessExecution
            | ToolRisk::NetworkAccess
            | ToolRisk::CapabilityChange => ApprovalDecision::RequireApproval,
            ToolRisk::Destructive => ApprovalDecision::Deny,
            ToolRisk::ReadOnly => ApprovalDecision::Deny,
        }
    }

    pub fn authorize(&self, request: &ToolRequest, approved: bool) -> Result<(), RigaError> {
        match (self.evaluate(request), approved) {
            (ApprovalDecision::Allow, _) | (ApprovalDecision::RequireApproval, true) => Ok(()),
            (ApprovalDecision::RequireApproval, false) => Err(RigaError {
                code: RigaErrorCode::InvalidRequest,
                message: format!("approval required for tool `{}`", request.tool_name),
                retryable: false,
            }),
            (ApprovalDecision::Deny, _) => Err(RigaError {
                code: RigaErrorCode::InvalidRequest,
                message: format!("tool `{}` is denied by policy", request.tool_name),
                retryable: false,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApprovalDecision, ToolPolicy, ToolRequest, ToolRisk};

    fn request(risk: ToolRisk) -> ToolRequest {
        ToolRequest {
            tool_name: "workspace.write".into(),
            risk,
            target: "src/lib.rs".into(),
            explanation: "apply requested patch".into(),
        }
    }

    #[test]
    fn read_only_is_allowed_by_default() {
        assert_eq!(
            ToolPolicy::default().evaluate(&request(ToolRisk::ReadOnly)),
            ApprovalDecision::Allow
        );
    }

    #[test]
    fn workspace_write_requires_explicit_approval() {
        let policy = ToolPolicy::default();
        assert_eq!(
            policy.evaluate(&request(ToolRisk::WorkspaceWrite)),
            ApprovalDecision::RequireApproval
        );
        assert!(
            policy
                .authorize(&request(ToolRisk::WorkspaceWrite), false)
                .is_err()
        );
        assert!(
            policy
                .authorize(&request(ToolRisk::WorkspaceWrite), true)
                .is_ok()
        );
    }

    #[test]
    fn destructive_actions_are_denied_even_without_approval() {
        let policy = ToolPolicy::default();
        assert_eq!(
            policy.evaluate(&request(ToolRisk::Destructive)),
            ApprovalDecision::Deny
        );
        assert!(
            policy
                .authorize(&request(ToolRisk::Destructive), true)
                .is_err()
        );
    }

    #[test]
    fn built_in_tools_are_classified_by_risk() {
        assert_eq!(ToolRisk::for_tool("read"), ToolRisk::ReadOnly);
        assert_eq!(ToolRisk::for_tool("glob"), ToolRisk::ReadOnly);
        assert_eq!(ToolRisk::for_tool("write"), ToolRisk::WorkspaceWrite);
        assert_eq!(ToolRisk::for_tool("bash"), ToolRisk::ProcessExecution);
        assert_eq!(ToolRisk::for_tool("web"), ToolRisk::NetworkAccess);
        // An unknown/MCP tool is not a workspace mutation.
        assert_eq!(ToolRisk::for_tool("mcp_x_y"), ToolRisk::ReadOnly);
    }
}
