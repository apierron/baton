//! Runtime adapter abstraction for validators.
//!
//! Defines the [`RuntimeAdapter`] trait, session lifecycle types, and
//! completion request/result types. Supports API, Claude Code, OpenHands,
//! and OpenCode as runtime backends.

pub mod api;
pub mod claude_code;
pub mod opencode;
pub mod openhands;
pub(crate) mod session_common;

use std::collections::BTreeMap;
use std::fmt::Debug;

use crate::error::{BatonError, Result};
use crate::types::Cost;

// ─── Session types ───────────────────────────────────────

/// Configuration for creating an agent session.
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

/// Opaque handle to a running agent session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub id: String,
    pub workspace_id: String,
}

/// Lifecycle state of an agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Running,
    Completed,
    Failed,
    TimedOut,
    Cancelled,
}

/// Collected output from a completed agent session.
#[derive(Debug, Clone)]
pub struct SessionResult {
    pub status: SessionStatus,
    pub output: String,
    pub raw_log: String,
    pub cost: Option<Cost>,
}

/// Result of a runtime health check.
#[derive(Debug, Clone)]
pub struct HealthResult {
    pub reachable: bool,
    pub version: Option<String>,
    pub models: Option<Vec<String>>,
    pub message: Option<String>,
}

// ─── Completion types ────────────────────────────────────

/// Request for a one-shot chat completion.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub messages: Vec<serde_json::Value>,
    pub model: String,
    pub temperature: f64,
    pub max_tokens: Option<u32>,
}

/// Result from a one-shot chat completion.
#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub content: String,
    pub cost: Option<Cost>,
}

// ─── RuntimeAdapter trait ────────────────────────────────

/// Interface for agent runtime backends.
///
/// Implementations manage the full session lifecycle: creation, polling,
/// result collection, cancellation, and cleanup.
///
/// # Session lifecycle
///
/// 1. [`health_check`](RuntimeAdapter::health_check) — verify the runtime is reachable
/// 2. [`create_session`](RuntimeAdapter::create_session) — start an agent session
/// 3. [`poll_status`](RuntimeAdapter::poll_status) — wait for completion
/// 4. [`collect_result`](RuntimeAdapter::collect_result) — retrieve the output
/// 5. [`teardown`](RuntimeAdapter::teardown) — clean up resources
///
/// For one-shot queries without a session,
/// use [`post_completion`](RuntimeAdapter::post_completion) instead.
pub trait RuntimeAdapter: Send + Sync + Debug {
    /// Checks whether the runtime is reachable and returns version info.
    fn health_check(&self) -> Result<HealthResult>;
    /// Creates a new agent session with the given configuration.
    fn create_session(&self, config: SessionConfig) -> Result<SessionHandle>;
    /// Polls the current status of a running session.
    fn poll_status(&self, handle: &SessionHandle) -> Result<SessionStatus>;
    /// Collects the final output from a completed session.
    fn collect_result(&self, handle: &SessionHandle) -> Result<SessionResult>;
    /// Cancels a running session. Idempotent.
    fn cancel(&self, handle: &SessionHandle) -> Result<()>;
    /// Cleans up session resources (workspace, files). Idempotent.
    fn teardown(&self, handle: &SessionHandle) -> Result<()>;

    /// Sends a one-shot chat completion request.
    ///
    /// Default implementation returns an error indicating the runtime
    /// does not support completions.
    fn post_completion(&self, _request: CompletionRequest) -> Result<CompletionResult> {
        Err(BatonError::RuntimeError(
            "This runtime does not support one-shot completions.".into(),
        ))
    }
}

// ─── Adapter registry ───────────────────────────────────

/// Creates the appropriate [`RuntimeAdapter`] for the given runtime configuration.
pub fn create_adapter(
    runtime_name: &str,
    runtime_config: &crate::config::Runtime,
) -> Result<Box<dyn RuntimeAdapter>> {
    match runtime_config.runtime_type.as_str() {
        "api" => {
            let adapter = api::ApiAdapter::new(
                runtime_config.base_url.clone(),
                runtime_config.api_key_env.as_deref(),
                runtime_config.default_model.clone(),
                runtime_config.timeout_seconds,
            )?;
            Ok(Box::new(adapter))
        }
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
        "opencode" => {
            let adapter = opencode::OpenCodeAdapter::new(
                runtime_config.base_url.clone(),
                runtime_config.api_key_env.as_deref(),
                runtime_config.default_model.clone(),
                runtime_config.sandbox,
                runtime_config.timeout_seconds,
                runtime_config.max_iterations,
            )?;
            Ok(Box::new(adapter))
        }
        "claude-code" => {
            let adapter = claude_code::ClaudeCodeAdapter::new(
                runtime_config.base_url.clone(),
                runtime_config.api_key_env.as_deref(),
                runtime_config.default_model.clone(),
                runtime_config.timeout_seconds,
                runtime_config.max_iterations,
            )?;
            Ok(Box::new(adapter))
        }
        other => Err(BatonError::ConfigError(format!(
            "Unknown runtime type '{other}' for runtime '{runtime_name}'. Supported: api, claude-code, openhands, opencode."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════
    // Internal implementation tests
    // ═══════════════════════════════════════════════════════════════

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

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // ═══════════════════════════════════════════════════════════════

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
    fn create_opencode_adapter_no_auth() {
        let config = crate::config::Runtime {
            runtime_type: "opencode".into(),
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
    fn create_opencode_adapter_empty_auth() {
        let config = crate::config::Runtime {
            runtime_type: "opencode".into(),
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

    #[test]
    fn create_claude_code_adapter() {
        let config = crate::config::Runtime {
            runtime_type: "claude-code".into(),
            base_url: "claude".into(),
            api_key_env: None,
            default_model: Some("sonnet".into()),
            sandbox: false,
            timeout_seconds: 120,
            max_iterations: 10,
        };
        let result = create_adapter("test", &config);
        assert!(result.is_ok());
    }
}
