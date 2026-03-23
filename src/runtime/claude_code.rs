//! Claude Code runtime adapter implementation.
//!
//! Subprocess-based adapter that spawns the `claude` CLI tool.
//! Unlike OpenHands/OpenCode which use HTTP APIs, this adapter
//! manages child processes for both query and session modes.

use std::collections::HashMap;
use std::io::Read as IoRead;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::error::{BatonError, Result};
use crate::types::Cost;

use super::{
    CompletionRequest, CompletionResult, HealthResult, RuntimeAdapter, SessionConfig,
    SessionHandle, SessionResult, SessionStatus,
};

// ─── Internal state ─────────────────────────────────────

/// State for a single running or completed session.
struct ChildState {
    child: Option<std::process::Child>,
    workspace_dir: std::path::PathBuf,
    stdout_data: Option<String>,
    stderr_data: Option<String>,
}

// Implement Debug manually since Child doesn't implement Debug usefully
impl std::fmt::Debug for ChildState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChildState")
            .field("workspace_dir", &self.workspace_dir)
            .field("has_child", &self.child.is_some())
            .field("has_stdout", &self.stdout_data.is_some())
            .field("has_stderr", &self.stderr_data.is_some())
            .finish()
    }
}

// ─── Claude Code adapter ────────────────────────────────

/// Subprocess-based runtime adapter for the Claude Code CLI.
///
/// Spawns `claude -p` for both one-shot completions and agent sessions.
/// Tracks child processes internally via a mutex-protected map.
pub struct ClaudeCodeAdapter {
    pub claude_path: String,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub timeout_seconds: u64,
    pub max_turns: u32,
    sessions: Mutex<HashMap<String, ChildState>>,
}

impl std::fmt::Debug for ClaudeCodeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCodeAdapter")
            .field("claude_path", &self.claude_path)
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field("default_model", &self.default_model)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("max_turns", &self.max_turns)
            .finish()
    }
}

impl ClaudeCodeAdapter {
    /// Creates a new adapter from connection parameters.
    ///
    /// `base_url` is used as the path to the `claude` binary.
    /// If `api_key_env` is provided and non-empty, the corresponding
    /// environment variable must be set or an error is returned.
    pub fn new(
        base_url: String,
        api_key_env: Option<&str>,
        default_model: Option<String>,
        timeout_seconds: u64,
        max_iterations: u32,
    ) -> Result<Self> {
        let api_key = match api_key_env {
            Some(env_name) if !env_name.is_empty() => {
                Some(std::env::var(env_name).map_err(|_| {
                    BatonError::ConfigError(format!(
                        "Runtime API key env var '{env_name}' is not set"
                    ))
                })?)
            }
            _ => None,
        };

        Ok(ClaudeCodeAdapter {
            claude_path: base_url,
            api_key,
            default_model,
            timeout_seconds,
            max_turns: max_iterations,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    /// Builds a base `Command` for the claude binary with common flags.
    fn build_command(&self, prompt: &str, model: Option<&str>) -> Command {
        let mut cmd = Command::new(&self.claude_path);
        cmd.arg("-p").arg(prompt);
        cmd.arg("--output-format").arg("json");

        if let Some(m) = model.or(self.default_model.as_deref()) {
            cmd.arg("--model").arg(m);
        }

        if self.max_turns > 0 {
            cmd.arg("--max-turns").arg(self.max_turns.to_string());
        }

        // Pass API key to subprocess if we have one
        if let Some(ref key) = self.api_key {
            cmd.env("ANTHROPIC_API_KEY", key);
        }

        cmd
    }
}

// ─── JSON output parsing ────────────────────────────────

/// Parsed output from Claude Code's `--output-format json`.
pub struct ParsedOutput {
    pub content: String,
    pub cost: Option<Cost>,
}

/// Parses Claude Code's JSON output format.
///
/// Expected format:
/// ```json
/// {
///   "type": "result",
///   "result": "response text",
///   "cost_usd": 0.05,
///   "usage": { "input_tokens": 1500, "output_tokens": 300 }
/// }
/// ```
pub fn parse_claude_output(json_str: &str) -> ParsedOutput {
    let body: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => {
            return ParsedOutput {
                content: json_str.to_string(),
                cost: None,
            };
        }
    };

    let content = body
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let input_tokens = body
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_i64());
    let output_tokens = body
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_i64());
    let estimated_usd = body.get("cost_usd").and_then(|v| v.as_f64());
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let cost = if input_tokens.is_some() || output_tokens.is_some() || estimated_usd.is_some() {
        Some(Cost {
            input_tokens,
            output_tokens,
            model,
            estimated_usd,
        })
    } else {
        None
    };

    ParsedOutput { content, cost }
}

// ─── RuntimeAdapter implementation ──────────────────────

impl RuntimeAdapter for ClaudeCodeAdapter {
    fn health_check(&self) -> Result<HealthResult> {
        let output = Command::new(&self.claude_path)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let version_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                Ok(HealthResult {
                    reachable: true,
                    version: if version_str.is_empty() {
                        None
                    } else {
                        Some(version_str)
                    },
                    models: None,
                    message: None,
                })
            }
            Ok(out) => Ok(HealthResult {
                reachable: false,
                version: None,
                models: None,
                message: Some(format!(
                    "claude exited with status {}",
                    out.status.code().unwrap_or(-1)
                )),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HealthResult {
                reachable: false,
                version: None,
                models: None,
                message: Some(format!("claude binary not found at '{}'", self.claude_path)),
            }),
            Err(e) => Ok(HealthResult {
                reachable: false,
                version: None,
                models: None,
                message: Some(format!("Failed to run claude: {e}")),
            }),
        }
    }

    fn create_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        // Create workspace directory
        let workspace_dir = tempfile::tempdir()
            .map_err(|e| {
                BatonError::RuntimeError(format!("Failed to create workspace directory: {e}"))
            })?
            .keep();

        // Copy files to workspace
        for (name, path) in &config.files {
            let content = std::fs::read(path).map_err(|e| {
                BatonError::ValidationError(format!(
                    "Failed to read file '{name}' at '{path}': {e}"
                ))
            })?;

            let dest = workspace_dir.join(name);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    BatonError::RuntimeError(format!(
                        "Failed to create directory for '{name}': {e}"
                    ))
                })?;
            }
            std::fs::write(&dest, content).map_err(|e| {
                BatonError::RuntimeError(format!("Failed to write file '{name}' to workspace: {e}"))
            })?;
        }

        // Spawn claude subprocess
        let mut cmd = self.build_command(&config.task, Some(&config.model));
        cmd.current_dir(&workspace_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| BatonError::RuntimeError(format!("Failed to spawn claude: {e}")))?;

        let session_id = uuid::Uuid::new_v4().to_string();
        let workspace_path = workspace_dir.to_string_lossy().to_string();

        let state = ChildState {
            child: Some(child),
            workspace_dir,
            stdout_data: None,
            stderr_data: None,
        };

        self.sessions
            .lock()
            .map_err(|e| BatonError::RuntimeError(format!("Session lock poisoned: {e}")))?
            .insert(session_id.clone(), state);

        Ok(SessionHandle {
            id: session_id,
            workspace_id: workspace_path,
        })
    }

    fn poll_status(&self, handle: &SessionHandle) -> Result<SessionStatus> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|e| BatonError::RuntimeError(format!("Session lock poisoned: {e}")))?;

        let state = sessions
            .get_mut(&handle.id)
            .ok_or_else(|| BatonError::RuntimeError(format!("Unknown session '{}'", handle.id)))?;

        let child = match state.child.as_mut() {
            Some(c) => c,
            None => {
                // Child already collected — check if we have stdout data
                return if state.stdout_data.is_some() {
                    Ok(SessionStatus::Completed)
                } else {
                    Ok(SessionStatus::Failed)
                };
            }
        };

        match child.try_wait() {
            Ok(None) => Ok(SessionStatus::Running),
            Ok(Some(status)) => {
                // Process exited — collect output now
                let mut stdout_str = String::new();
                if let Some(ref mut stdout) = child.stdout {
                    let _ = stdout.read_to_string(&mut stdout_str);
                }
                let mut stderr_str = String::new();
                if let Some(ref mut stderr) = child.stderr {
                    let _ = stderr.read_to_string(&mut stderr_str);
                }

                state.stdout_data = Some(stdout_str);
                state.stderr_data = Some(stderr_str);
                state.child = None;

                if status.success() {
                    Ok(SessionStatus::Completed)
                } else {
                    Ok(SessionStatus::Failed)
                }
            }
            Err(e) => Err(BatonError::RuntimeError(format!(
                "Failed to check session status: {e}"
            ))),
        }
    }

    fn collect_result(&self, handle: &SessionHandle) -> Result<SessionResult> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|e| BatonError::RuntimeError(format!("Session lock poisoned: {e}")))?;

        let state = sessions
            .get_mut(&handle.id)
            .ok_or_else(|| BatonError::RuntimeError(format!("Unknown session '{}'", handle.id)))?;

        // If child is still alive, take ownership and wait for it
        if let Some(child) = state.child.take() {
            let output = child.wait_with_output().map_err(|e| {
                BatonError::RuntimeError(format!("Failed to wait for claude process: {e}"))
            })?;

            state.stdout_data = Some(String::from_utf8_lossy(&output.stdout).to_string());
            state.stderr_data = Some(String::from_utf8_lossy(&output.stderr).to_string());

            let parsed = parse_claude_output(state.stdout_data.as_deref().unwrap_or(""));

            let status = if output.status.success() {
                SessionStatus::Completed
            } else {
                SessionStatus::Failed
            };

            return Ok(SessionResult {
                status,
                output: parsed.content,
                raw_log: state.stdout_data.clone().unwrap_or_default(),
                cost: parsed.cost,
            });
        }

        // Child already collected via poll_status
        let stdout = state.stdout_data.clone().unwrap_or_default();
        let parsed = parse_claude_output(&stdout);

        // Determine status from whether we got data
        let status =
            if state.stderr_data.as_deref().is_some_and(|s| !s.is_empty()) && stdout.is_empty() {
                SessionStatus::Failed
            } else {
                SessionStatus::Completed
            };

        Ok(SessionResult {
            status,
            output: parsed.content,
            raw_log: stdout,
            cost: parsed.cost,
        })
    }

    fn cancel(&self, handle: &SessionHandle) -> Result<()> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|e| BatonError::RuntimeError(format!("Session lock poisoned: {e}")))?;

        if let Some(state) = sessions.get_mut(&handle.id) {
            if let Some(ref mut child) = state.child {
                let _ = child.kill();
                let _ = child.wait(); // reap the zombie
            }
        }

        Ok(())
    }

    fn teardown(&self, handle: &SessionHandle) -> Result<()> {
        // Remove from sessions map
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|e| BatonError::RuntimeError(format!("Session lock poisoned: {e}")))?;

        if let Some(mut state) = sessions.remove(&handle.id) {
            // Kill child if still running
            if let Some(ref mut child) = state.child {
                let _ = child.kill();
                let _ = child.wait();
            }

            // Remove workspace directory
            let _ = std::fs::remove_dir_all(&state.workspace_dir);
        }

        Ok(())
    }

    fn post_completion(&self, request: CompletionRequest) -> Result<CompletionResult> {
        // Build prompt from messages
        let prompt = request
            .messages
            .iter()
            .filter_map(|m| m.get("content").and_then(|c| c.as_str()))
            .collect::<Vec<_>>()
            .join("\n\n");

        let model = if request.model.is_empty() {
            None
        } else {
            Some(request.model.as_str())
        };

        let mut cmd = self.build_command(&prompt, model);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Don't use max-turns for one-shot completions
        // Rebuild without max-turns
        let mut cmd = Command::new(&self.claude_path);
        cmd.arg("-p").arg(&prompt);
        cmd.arg("--output-format").arg("json");

        if let Some(m) = model.or(self.default_model.as_deref()) {
            cmd.arg("--model").arg(m);
        }

        if let Some(ref key) = self.api_key {
            cmd.env("ANTHROPIC_API_KEY", key);
        }

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = cmd
            .output()
            .map_err(|e| BatonError::RuntimeError(format!("Failed to spawn claude: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BatonError::RuntimeError(format!(
                "claude exited with status {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed = parse_claude_output(&stdout);

        if parsed.content.is_empty() {
            return Err(BatonError::ValidationError(
                "Claude Code returned empty response".into(),
            ));
        }

        Ok(CompletionResult {
            content: parsed.content,
            cost: parsed.cost,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════
    // parse_claude_output
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn parse_output_result_field() {
        let json = r#"{"type":"result","result":"PASS — code looks good"}"#;
        let parsed = parse_claude_output(json);
        assert_eq!(parsed.content, "PASS — code looks good");
        assert!(parsed.cost.is_none());
    }

    #[test]
    fn parse_output_cost_fields() {
        let json = r#"{
            "type": "result",
            "result": "PASS",
            "cost_usd": 0.05,
            "usage": {"input_tokens": 1500, "output_tokens": 300}
        }"#;
        let parsed = parse_claude_output(json);
        assert_eq!(parsed.content, "PASS");
        let cost = parsed.cost.unwrap();
        assert_eq!(cost.input_tokens, Some(1500));
        assert_eq!(cost.output_tokens, Some(300));
        assert_eq!(cost.estimated_usd, Some(0.05));
    }

    #[test]
    fn parse_output_missing_result() {
        let json = r#"{"type": "result"}"#;
        let parsed = parse_claude_output(json);
        assert_eq!(parsed.content, "");
    }

    #[test]
    fn parse_output_no_cost() {
        let json = r#"{"type": "result", "result": "hello"}"#;
        let parsed = parse_claude_output(json);
        assert!(parsed.cost.is_none());
    }

    #[test]
    fn parse_output_invalid_json() {
        let raw = "not json at all";
        let parsed = parse_claude_output(raw);
        assert_eq!(parsed.content, "not json at all");
        assert!(parsed.cost.is_none());
    }

    #[test]
    fn parse_output_partial_cost() {
        let json = r#"{"result": "ok", "cost_usd": 0.01}"#;
        let parsed = parse_claude_output(json);
        let cost = parsed.cost.unwrap();
        assert_eq!(cost.estimated_usd, Some(0.01));
        assert_eq!(cost.input_tokens, None);
        assert_eq!(cost.output_tokens, None);
    }

    #[test]
    fn parse_output_with_model() {
        let json = r#"{"result": "ok", "model": "claude-sonnet-4-20250514", "usage": {"input_tokens": 100, "output_tokens": 50}}"#;
        let parsed = parse_claude_output(json);
        let cost = parsed.cost.unwrap();
        assert_eq!(cost.model, Some("claude-sonnet-4-20250514".into()));
    }

    // ═══════════════════════════════════════════════════════════════
    // Constructor
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn new_stores_claude_path() {
        let adapter = ClaudeCodeAdapter::new("claude".into(), None, None, 600, 30).unwrap();
        assert_eq!(adapter.claude_path, "claude");
    }

    #[test]
    fn new_stores_config_fields() {
        let adapter = ClaudeCodeAdapter::new(
            "/usr/local/bin/claude".into(),
            None,
            Some("claude-sonnet".into()),
            300,
            15,
        )
        .unwrap();
        assert_eq!(adapter.claude_path, "/usr/local/bin/claude");
        assert_eq!(adapter.default_model, Some("claude-sonnet".into()));
        assert_eq!(adapter.timeout_seconds, 300);
        assert_eq!(adapter.max_turns, 15);
    }

    #[test]
    fn new_no_api_key_env() {
        let adapter = ClaudeCodeAdapter::new("claude".into(), None, None, 600, 30).unwrap();
        assert!(adapter.api_key.is_none());
    }

    #[test]
    fn new_empty_api_key_env() {
        let adapter = ClaudeCodeAdapter::new("claude".into(), Some(""), None, 600, 30).unwrap();
        assert!(adapter.api_key.is_none());
    }

    #[test]
    fn new_missing_env_var_returns_config_error() {
        let result = ClaudeCodeAdapter::new(
            "claude".into(),
            Some("BATON_TEST_CC_NONEXISTENT_KEY_12345"),
            None,
            600,
            30,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("BATON_TEST_CC_NONEXISTENT_KEY_12345"),
            "Error should mention env var: {err}"
        );
        assert!(err.contains("not set"), "Error should say 'not set': {err}");
    }

    #[test]
    fn new_valid_env_var_is_resolved() {
        let env_var = "BATON_TEST_CC_KEY";
        std::env::set_var(env_var, "test-key-123");
        let result = ClaudeCodeAdapter::new("claude".into(), Some(env_var), None, 600, 30);
        std::env::remove_var(env_var);
        assert!(result.is_ok());
        let adapter = result.unwrap();
        assert_eq!(adapter.api_key, Some("test-key-123".into()));
    }

    // ═══════════════════════════════════════════════════════════════
    // health_check
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn health_check_binary_not_found() {
        let adapter =
            ClaudeCodeAdapter::new("/nonexistent/path/to/claude".into(), None, None, 600, 30)
                .unwrap();
        let result = adapter.health_check().unwrap();
        assert!(!result.reachable);
        assert!(result.message.is_some());
        assert!(
            result.message.as_ref().unwrap().contains("not found"),
            "Message: {:?}",
            result.message
        );
    }

    #[test]
    fn health_check_success() {
        // Use 'echo' as a stand-in for claude --version
        let adapter = ClaudeCodeAdapter::new("echo".into(), None, None, 600, 30).unwrap();
        let result = adapter.health_check().unwrap();
        assert!(result.reachable);
        // echo --version prints "--version" to stdout
        assert!(result.version.is_some());
    }

    #[test]
    fn health_check_non_zero_exit() {
        // 'false' always exits with code 1
        let adapter = ClaudeCodeAdapter::new("false".into(), None, None, 600, 30).unwrap();
        let result = adapter.health_check().unwrap();
        assert!(!result.reachable);
        assert!(result.message.is_some());
    }

    // ═══════════════════════════════════════════════════════════════
    // create_session
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn create_session_spawn_failure() {
        let adapter =
            ClaudeCodeAdapter::new("/nonexistent/claude".into(), None, None, 600, 30).unwrap();

        let config = SessionConfig {
            task: "test".into(),
            files: std::collections::BTreeMap::new(),
            model: "test-model".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let result = adapter.create_session(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to spawn claude"), "Error: {err}");
    }

    #[test]
    fn create_session_file_read_error() {
        let adapter = ClaudeCodeAdapter::new("echo".into(), None, None, 600, 30).unwrap();

        let mut files = std::collections::BTreeMap::new();
        files.insert("test.py".into(), "/nonexistent/file.py".into());

        let config = SessionConfig {
            task: "test".into(),
            files,
            model: "test-model".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let result = adapter.create_session(config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("test.py"), "Error should mention file: {err}");
    }

    #[test]
    fn create_session_copies_files() {
        // Use 'sleep' as a stand-in so the process stays alive briefly
        let adapter = ClaudeCodeAdapter::new("sleep".into(), None, None, 600, 0).unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp.as_file().try_clone().unwrap(), b"file content")
            .unwrap();

        let mut files = std::collections::BTreeMap::new();
        files.insert("input.txt".into(), tmp.path().to_str().unwrap().to_string());

        let config = SessionConfig {
            task: "10".into(), // sleep 10
            files,
            model: "test-model".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();

        // Verify file was copied
        let workspace = std::path::Path::new(&handle.workspace_id);
        let copied = std::fs::read_to_string(workspace.join("input.txt")).unwrap();
        assert_eq!(copied, "file content");

        // Clean up
        let _ = adapter.cancel(&handle);
        let _ = adapter.teardown(&handle);
    }

    #[test]
    fn create_session_returns_handle() {
        let adapter = ClaudeCodeAdapter::new("sleep".into(), None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: "10".into(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        assert!(!handle.id.is_empty());
        assert!(!handle.workspace_id.is_empty());

        // Clean up
        let _ = adapter.cancel(&handle);
        let _ = adapter.teardown(&handle);
    }

    // ═══════════════════════════════════════════════════════════════
    // poll_status
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn poll_status_unknown_session() {
        let adapter = ClaudeCodeAdapter::new("echo".into(), None, None, 600, 30).unwrap();
        let handle = SessionHandle {
            id: "nonexistent".into(),
            workspace_id: "/tmp".into(),
        };
        let result = adapter.poll_status(&handle);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown session"), "Error: {err}");
    }

    #[test]
    fn poll_status_completed() {
        // 'true' exits immediately with code 0
        let adapter = ClaudeCodeAdapter::new("true".into(), None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: String::new(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();

        // Give the process a moment to exit
        std::thread::sleep(std::time::Duration::from_millis(100));

        let status = adapter.poll_status(&handle).unwrap();
        assert_eq!(status, SessionStatus::Completed);

        let _ = adapter.teardown(&handle);
    }

    #[test]
    fn poll_status_failed() {
        // 'false' exits immediately with code 1
        let adapter = ClaudeCodeAdapter::new("false".into(), None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: String::new(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));

        let status = adapter.poll_status(&handle).unwrap();
        assert_eq!(status, SessionStatus::Failed);

        let _ = adapter.teardown(&handle);
    }

    // ═══════════════════════════════════════════════════════════════
    // collect_result
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn collect_result_unknown_session() {
        let adapter = ClaudeCodeAdapter::new("echo".into(), None, None, 600, 30).unwrap();
        let handle = SessionHandle {
            id: "nonexistent".into(),
            workspace_id: "/tmp".into(),
        };
        let result = adapter.collect_result(&handle);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown session"), "Error: {err}");
    }

    #[test]
    fn collect_result_parses_json() {
        // Use a script that outputs JSON to stdout
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho '{\"type\":\"result\",\"result\":\"PASS - looks good\"}'",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: String::new(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        let result = adapter.collect_result(&handle).unwrap();
        assert_eq!(result.status, SessionStatus::Completed);
        assert_eq!(result.output, "PASS - looks good");

        let _ = adapter.teardown(&handle);
    }

    #[test]
    fn collect_result_extracts_cost() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho '{\"result\":\"ok\",\"cost_usd\":0.05,\"usage\":{\"input_tokens\":1000,\"output_tokens\":200}}'",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: String::new(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        let result = adapter.collect_result(&handle).unwrap();
        let cost = result.cost.unwrap();
        assert_eq!(cost.input_tokens, Some(1000));
        assert_eq!(cost.output_tokens, Some(200));
        assert_eq!(cost.estimated_usd, Some(0.05));

        let _ = adapter.teardown(&handle);
    }

    #[test]
    fn collect_result_non_json_output() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho 'plain text output'",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: String::new(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        let result = adapter.collect_result(&handle).unwrap();
        assert!(result.output.contains("plain text output"));
        assert!(result.cost.is_none());

        let _ = adapter.teardown(&handle);
    }

    #[test]
    fn collect_result_failed_status() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\nexit 1",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: String::new(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        let result = adapter.collect_result(&handle).unwrap();
        assert_eq!(result.status, SessionStatus::Failed);

        let _ = adapter.teardown(&handle);
    }

    // ═══════════════════════════════════════════════════════════════
    // cancel
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn cancel_unknown_session_ok() {
        let adapter = ClaudeCodeAdapter::new("echo".into(), None, None, 600, 30).unwrap();
        let handle = SessionHandle {
            id: "nonexistent".into(),
            workspace_id: "/tmp".into(),
        };
        assert!(adapter.cancel(&handle).is_ok());
    }

    #[test]
    fn cancel_kills_process() {
        let adapter = ClaudeCodeAdapter::new("sleep".into(), None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: "60".into(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        assert!(adapter.cancel(&handle).is_ok());

        // After cancel, poll should show non-running state
        std::thread::sleep(std::time::Duration::from_millis(100));
        let status = adapter.poll_status(&handle).unwrap();
        assert_ne!(status, SessionStatus::Running);

        let _ = adapter.teardown(&handle);
    }

    // ═══════════════════════════════════════════════════════════════
    // teardown
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn teardown_unknown_session_ok() {
        let adapter = ClaudeCodeAdapter::new("echo".into(), None, None, 600, 30).unwrap();
        let handle = SessionHandle {
            id: "nonexistent".into(),
            workspace_id: "/tmp".into(),
        };
        assert!(adapter.teardown(&handle).is_ok());
    }

    #[test]
    fn teardown_removes_workspace() {
        let adapter = ClaudeCodeAdapter::new("sleep".into(), None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: "60".into(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        let workspace_path = handle.workspace_id.clone();
        assert!(std::path::Path::new(&workspace_path).exists());

        adapter.teardown(&handle).unwrap();
        assert!(!std::path::Path::new(&workspace_path).exists());
    }

    #[test]
    fn teardown_removes_from_map() {
        let adapter = ClaudeCodeAdapter::new("sleep".into(), None, None, 600, 0).unwrap();

        let config = SessionConfig {
            task: "60".into(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 10,
            timeout_seconds: 30,
            env: std::collections::BTreeMap::new(),
        };

        let handle = adapter.create_session(config).unwrap();
        adapter.teardown(&handle).unwrap();

        // Should now return error for unknown session
        let result = adapter.poll_status(&handle);
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════════════════════════
    // post_completion
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn post_completion_success() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho '{\"type\":\"result\",\"result\":\"PASS - looks good\"}'",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 30).unwrap();

        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            model: "test".into(),
            temperature: 0.0,
            max_tokens: None,
        };

        let result = adapter.post_completion(request).unwrap();
        assert_eq!(result.content, "PASS - looks good");
    }

    #[test]
    fn post_completion_extracts_cost() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho '{\"result\":\"ok\",\"cost_usd\":0.03,\"usage\":{\"input_tokens\":500,\"output_tokens\":100}}'",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 30).unwrap();

        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            model: "test".into(),
            temperature: 0.0,
            max_tokens: None,
        };

        let result = adapter.post_completion(request).unwrap();
        let cost = result.cost.unwrap();
        assert_eq!(cost.input_tokens, Some(500));
        assert_eq!(cost.output_tokens, Some(100));
        assert_eq!(cost.estimated_usd, Some(0.03));
    }

    #[test]
    fn post_completion_empty_content_error() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho '{\"type\":\"result\",\"result\":\"\"}'",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 30).unwrap();

        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            model: "test".into(),
            temperature: 0.0,
            max_tokens: None,
        };

        let result = adapter.post_completion(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty response"), "Error: {err}");
    }

    #[test]
    fn post_completion_non_zero_exit() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho 'error message' >&2\nexit 1",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 30).unwrap();

        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            model: "test".into(),
            temperature: 0.0,
            max_tokens: None,
        };

        let result = adapter.post_completion(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("error message"), "Error: {err}");
    }

    #[test]
    fn post_completion_spawn_failure() {
        let adapter =
            ClaudeCodeAdapter::new("/nonexistent/claude".into(), None, None, 600, 30).unwrap();

        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "hello"})],
            model: "test".into(),
            temperature: 0.0,
            max_tokens: None,
        };

        let result = adapter.post_completion(request);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to spawn claude"), "Error: {err}");
    }

    #[test]
    fn post_completion_parses_content() {
        let script = tempfile::Builder::new().suffix(".sh").tempfile().unwrap();
        std::io::Write::write_all(
            &mut script.as_file().try_clone().unwrap(),
            b"#!/bin/sh\necho '{\"result\":\"FAIL - found issues in code\"}'",
        )
        .unwrap();
        let script_path = script.path().to_str().unwrap().to_string();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let adapter = ClaudeCodeAdapter::new(script_path, None, None, 600, 30).unwrap();

        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "review this"})],
            model: "test".into(),
            temperature: 0.0,
            max_tokens: None,
        };

        let result = adapter.post_completion(request).unwrap();
        assert_eq!(result.content, "FAIL - found issues in code");
    }
}
