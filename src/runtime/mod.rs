pub mod openhands;

use std::collections::BTreeMap;
use std::fmt::Debug;

use crate::error::{BatonError, Result};
use crate::types::Cost;

// ─── Session types ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub task: String,
    pub files: BTreeMap<String, String>,
    pub model: String,
    pub sandbox: bool,
    pub max_iterations: u32,
    pub timeout_seconds: u64,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct SessionResult {
    pub status: SessionStatus,
    pub output: String,
    pub raw_log: String,
    pub cost: Option<Cost>,
}

#[derive(Debug, Clone)]
pub struct HealthResult {
    pub reachable: bool,
    pub version: Option<String>,
    pub models: Option<Vec<String>>,
    pub message: Option<String>,
}

// ─── RuntimeAdapter trait ────────────────────────────────

pub trait RuntimeAdapter: Send + Sync + Debug {
    fn health_check(&self) -> Result<HealthResult>;
    fn create_session(&self, config: SessionConfig) -> Result<SessionHandle>;
    fn poll_status(&self, handle: &SessionHandle) -> Result<SessionStatus>;
    fn collect_result(&self, handle: &SessionHandle) -> Result<SessionResult>;
    fn cancel(&self, handle: &SessionHandle) -> Result<()>;
    fn teardown(&self, handle: &SessionHandle) -> Result<()>;
}

// ─── Adapter registry ───────────────────────────────────

pub fn create_adapter(
    runtime_name: &str,
    runtime_config: &crate::config::Runtime,
) -> Result<Box<dyn RuntimeAdapter>> {
    match runtime_config.runtime_type.as_str() {
        "openhands" => {
            let adapter = openhands::OpenHandsAdapter::new(
                runtime_config.base_url.clone(),
                runtime_config.api_key_env.as_deref(),
                runtime_config.default_model.clone(),
                runtime_config.sandbox,
                runtime_config.timeout_seconds,
                runtime_config.max_iterations,
            )?;
            Ok(Box::new(adapter))
        }
        other => Err(BatonError::ConfigError(format!(
            "Unknown runtime type '{other}' for runtime '{runtime_name}'. Only 'openhands' is currently supported."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Adapter creation ───────────────────────────────

    #[test]
    fn create_adapter_unknown_type() {
        let config = crate::config::Runtime {
            runtime_type: "unknown".into(),
            base_url: "http://localhost".into(),
            api_key_env: None,
            default_model: None,
            sandbox: true,
            timeout_seconds: 600,
            max_iterations: 30,
        };
        let result = create_adapter("test", &config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown runtime type"), "Error: {err}");
    }

    #[test]
    fn create_openhands_adapter_no_auth() {
        let config = crate::config::Runtime {
            runtime_type: "openhands".into(),
            base_url: "http://localhost:3000".into(),
            api_key_env: None,
            default_model: Some("test-model".into()),
            sandbox: true,
            timeout_seconds: 600,
            max_iterations: 30,
        };
        let result = create_adapter("test", &config);
        assert!(result.is_ok());
    }

    #[test]
    fn create_openhands_adapter_empty_auth() {
        let config = crate::config::Runtime {
            runtime_type: "openhands".into(),
            base_url: "http://localhost:3000/".into(),
            api_key_env: Some("".into()),
            default_model: None,
            sandbox: false,
            timeout_seconds: 300,
            max_iterations: 10,
        };
        let result = create_adapter("test", &config);
        assert!(result.is_ok());
    }

    // ─── Shared type construction ───────────────────────

    #[test]
    fn session_config_construction() {
        let mut files = BTreeMap::new();
        files.insert("artifact".into(), "/tmp/test.py".into());

        let config = SessionConfig {
            task: "Review this code".into(),
            files,
            model: "claude-sonnet".into(),
            sandbox: true,
            max_iterations: 30,
            timeout_seconds: 600,
            env: BTreeMap::new(),
        };

        assert_eq!(config.task, "Review this code");
        assert_eq!(config.files.len(), 1);
        assert!(config.sandbox);
    }

    #[test]
    fn session_handle_construction() {
        let handle = SessionHandle {
            id: "sess-123".into(),
            workspace_id: "ws-456".into(),
        };
        assert_eq!(handle.id, "sess-123");
        assert_eq!(handle.workspace_id, "ws-456");
    }

    #[test]
    fn session_result_construction() {
        let result = SessionResult {
            status: SessionStatus::Completed,
            output: "PASS — code looks good".into(),
            raw_log: "full log here".into(),
            cost: Some(Cost {
                input_tokens: Some(1000),
                output_tokens: Some(200),
                model: Some("test".into()),
                estimated_usd: None,
            }),
        };
        assert_eq!(result.status, SessionStatus::Completed);
        assert!(result.cost.is_some());
    }

    #[test]
    fn health_result_construction() {
        let result = HealthResult {
            reachable: true,
            version: Some("1.0".into()),
            models: Some(vec!["model-a".into()]),
            message: None,
        };
        assert!(result.reachable);
        assert_eq!(result.version, Some("1.0".into()));
    }
}
