//! Shared implementation for session-based runtime adapters.
//!
//! Both OpenCode and OpenHands (and future session adapters) share identical
//! HTTP lifecycle logic: constructor, auth headers, health check, create session,
//! poll status, collect result, cancel, teardown, and post_completion. This module
//! provides `SessionAdapterBase` which encapsulates all of that shared code.

use crate::error::{BatonError, Result};
use crate::types::Cost;

use super::{
    CompletionRequest, CompletionResult, HealthResult, SessionConfig, SessionHandle, SessionResult,
    SessionStatus,
};

// ─── SessionAdapterBase ─────────────────────────────────

/// Shared implementation for session-based runtime adapters.
///
/// Holds connection state and implements all HTTP operations. Concrete
/// adapters (OpenCode, OpenHands) wrap this and delegate all trait methods.
#[derive(Debug)]
pub struct SessionAdapterBase {
    pub base_url: String,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub sandbox: bool,
    pub timeout_seconds: u64,
    pub max_iterations: u32,
    pub client: reqwest::blocking::Client,
}

impl SessionAdapterBase {
    /// Creates a new base adapter from connection parameters.
    ///
    /// If `api_key_env` is provided and non-empty, the corresponding
    /// environment variable must be set or an error is returned.
    pub fn new(
        base_url: String,
        api_key_env: Option<&str>,
        default_model: Option<String>,
        sandbox: bool,
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

        let mut base = base_url;
        if base.ends_with('/') {
            base.pop();
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds + 30))
            .build()
            .map_err(|e| {
                BatonError::ValidationError(format!("Failed to create HTTP client: {e}"))
            })?;

        Ok(SessionAdapterBase {
            base_url: base,
            api_key,
            default_model,
            sandbox,
            timeout_seconds,
            max_iterations,
            client,
        })
    }

    pub fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(ref key) = self.api_key {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
        }
        headers
    }

    pub fn health_check(&self) -> Result<HealthResult> {
        let url = format!("{}/api/health", self.base_url);
        let response = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .map_err(|e| {
                BatonError::ValidationError(format!(
                    "Cannot reach runtime at {}: {e}",
                    self.base_url
                ))
            })?;

        if response.status().is_success() {
            let body: serde_json::Value = response.json().unwrap_or_default();
            Ok(HealthResult {
                reachable: true,
                version: body
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                models: None,
                message: None,
            })
        } else {
            Ok(HealthResult {
                reachable: false,
                version: None,
                models: None,
                message: Some(format!("HTTP {}", response.status())),
            })
        }
    }

    pub fn create_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let workspace_id = uuid::Uuid::new_v4().to_string();

        // Upload files to workspace
        for (name, path) in &config.files {
            let file_content = std::fs::read(path).map_err(|e| {
                BatonError::ValidationError(format!(
                    "Failed to read file '{name}' at '{path}': {e}"
                ))
            })?;

            let url = format!("{}/api/workspaces/{}/files", self.base_url, workspace_id);

            let part =
                reqwest::blocking::multipart::Part::bytes(file_content).file_name(name.clone());
            let form = reqwest::blocking::multipart::Form::new().part("file", part);

            let response = self
                .client
                .post(&url)
                .headers(self.auth_headers())
                .multipart(form)
                .send()
                .map_err(|e| {
                    BatonError::ValidationError(format!(
                        "Failed to upload file '{name}' to runtime: {e}"
                    ))
                })?;

            if !response.status().is_success() {
                return Err(BatonError::ValidationError(format!(
                    "Failed to upload file '{name}': HTTP {}",
                    response.status()
                )));
            }
        }

        // Create session
        let url = format!("{}/api/sessions", self.base_url);
        let body = serde_json::json!({
            "workspace_id": workspace_id,
            "task": config.task,
            "model": config.model,
            "sandbox": config.sandbox,
            "max_iterations": config.max_iterations,
            "timeout": config.timeout_seconds,
        });

        let response = self
            .client
            .post(&url)
            .headers(self.auth_headers())
            .json(&body)
            .send()
            .map_err(|e| {
                BatonError::ValidationError(format!("Failed to create session on runtime: {e}"))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().unwrap_or_default();
            return Err(BatonError::ValidationError(format!(
                "Failed to create session: HTTP {status}: {body_text}"
            )));
        }

        let resp_body: serde_json::Value = response.json().map_err(|e| {
            BatonError::ValidationError(format!("Failed to parse session creation response: {e}"))
        })?;

        let session_id = resp_body
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BatonError::ValidationError("Session creation response missing 'session_id'".into())
            })?
            .to_string();

        Ok(SessionHandle {
            id: session_id,
            workspace_id,
        })
    }

    pub fn poll_status(&self, handle: &SessionHandle) -> Result<SessionStatus> {
        let url = format!("{}/api/sessions/{}/status", self.base_url, handle.id);

        let response = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .map_err(|e| {
                BatonError::ValidationError(format!("Failed to poll session status: {e}"))
            })?;

        if !response.status().is_success() {
            return Err(BatonError::ValidationError(format!(
                "Failed to poll session status: HTTP {}",
                response.status()
            )));
        }

        let body: serde_json::Value = response.json().map_err(|e| {
            BatonError::ValidationError(format!("Failed to parse status response: {e}"))
        })?;

        let status_str = body
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        Ok(map_session_status(status_str))
    }

    pub fn collect_result(&self, handle: &SessionHandle) -> Result<SessionResult> {
        let url = format!("{}/api/sessions/{}/result", self.base_url, handle.id);

        let response = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .map_err(|e| {
                BatonError::ValidationError(format!("Failed to collect session result: {e}"))
            })?;

        if !response.status().is_success() {
            return Err(BatonError::ValidationError(format!(
                "Failed to collect session result: HTTP {}",
                response.status()
            )));
        }

        let body: serde_json::Value = response.json().map_err(|e| {
            BatonError::ValidationError(format!("Failed to parse result response: {e}"))
        })?;

        let status_str = body
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("failed");

        let output = body
            .get("final_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let raw_log = body
            .get("full_log")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let cost = extract_cost_from_metrics(&body);

        Ok(SessionResult {
            status: map_session_status(status_str),
            output,
            raw_log,
            cost,
        })
    }

    pub fn cancel(&self, handle: &SessionHandle) -> Result<()> {
        let url = format!("{}/api/sessions/{}", self.base_url, handle.id);

        // Idempotent: ignore errors on cancel
        let _ = self.client.delete(&url).headers(self.auth_headers()).send();

        Ok(())
    }

    pub fn teardown(&self, handle: &SessionHandle) -> Result<()> {
        let url = format!("{}/api/workspaces/{}", self.base_url, handle.workspace_id);

        // Idempotent: ignore errors on teardown
        let _ = self.client.delete(&url).headers(self.auth_headers()).send();

        Ok(())
    }

    pub fn post_completion(&self, request: CompletionRequest) -> Result<CompletionResult> {
        let url = format!("{}/v1/chat/completions", self.base_url);

        let mut body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
        });

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }

        let mut req = self.client.post(&url).json(&body);
        req = req.headers(self.auth_headers());

        let response = req.send().map_err(|e| {
            BatonError::ValidationError(format!("Failed to send completion request: {e}"))
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().unwrap_or_default();
            return Err(BatonError::ValidationError(format!(
                "Completion request failed: HTTP {status}: {body_text}"
            )));
        }

        let resp_body: serde_json::Value = response.json().map_err(|e| {
            BatonError::ValidationError(format!("Failed to parse completion response: {e}"))
        })?;

        let content = resp_body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        let cost = crate::provider::extract_cost(&resp_body, &request.model);

        if content.is_empty() {
            return Err(BatonError::ValidationError(
                "Completion response had empty content".into(),
            ));
        }

        Ok(CompletionResult { content, cost })
    }
}

// ─── Shared helpers ─────────────────────────────────────

/// Maps a status string from a session runtime to a `SessionStatus`.
///
/// Case-insensitive. Unknown values default to `Failed` (conservative).
pub fn map_session_status(status: &str) -> SessionStatus {
    match status.to_lowercase().as_str() {
        "running" | "pending" | "started" => SessionStatus::Running,
        "completed" | "finished" | "done" => SessionStatus::Completed,
        "failed" | "error" => SessionStatus::Failed,
        "timed_out" | "timeout" => SessionStatus::TimedOut,
        "cancelled" | "canceled" | "stopped" => SessionStatus::Cancelled,
        _ => SessionStatus::Failed,
    }
}

/// Extracts cost metadata from a session result's `metrics` JSON field.
///
/// Returns `None` if no metrics are present or both token counts are missing.
pub fn extract_cost_from_metrics(body: &serde_json::Value) -> Option<Cost> {
    let metrics = body.get("metrics")?;

    let input_tokens = metrics.get("input_tokens").and_then(|v| v.as_i64());
    let output_tokens = metrics.get("output_tokens").and_then(|v| v.as_i64());
    let model = metrics
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let estimated_usd = metrics.get("cost").and_then(|v| v.as_f64());

    if input_tokens.is_none() && output_tokens.is_none() {
        return None;
    }

    Some(Cost {
        input_tokens,
        output_tokens,
        model,
        estimated_usd,
    })
}

// ─── Shared test macro ──────────────────────────────────

/// Generates the full test suite for any `SessionAdapterBase`-backed adapter.
///
/// Usage:
/// ```ignore
/// session_adapter_tests!(
///     AdapterType,                          // The concrete adapter struct
///     "ENV_VAR_PREFIX",                     // Unique prefix for test env vars
///     |server: &MockServer| { ... }        // Factory: build adapter from mock server
/// );
/// ```
#[cfg(test)]
macro_rules! session_adapter_tests {
    ($adapter_ty:ty, $env_prefix:expr, $adapter_factory:expr) => {
        use $crate::runtime::session_common::{
            extract_cost_from_metrics, map_session_status, SessionAdapterBase,
        };
        use $crate::runtime::{SessionConfig, SessionHandle, SessionStatus};

        // ─── SessionStatus mapping ──────────────────────────

        #[test]
        fn map_status_running() {
            assert_eq!(map_session_status("running"), SessionStatus::Running);
            assert_eq!(map_session_status("pending"), SessionStatus::Running);
            assert_eq!(map_session_status("started"), SessionStatus::Running);
        }

        #[test]
        fn map_status_completed() {
            assert_eq!(map_session_status("completed"), SessionStatus::Completed);
            assert_eq!(map_session_status("finished"), SessionStatus::Completed);
            assert_eq!(map_session_status("done"), SessionStatus::Completed);
        }

        #[test]
        fn map_status_failed() {
            assert_eq!(map_session_status("failed"), SessionStatus::Failed);
            assert_eq!(map_session_status("error"), SessionStatus::Failed);
        }

        #[test]
        fn map_status_timed_out() {
            assert_eq!(map_session_status("timed_out"), SessionStatus::TimedOut);
            assert_eq!(map_session_status("timeout"), SessionStatus::TimedOut);
        }

        #[test]
        fn map_status_cancelled() {
            assert_eq!(
                map_session_status("cancelled"),
                SessionStatus::Cancelled
            );
            assert_eq!(map_session_status("canceled"), SessionStatus::Cancelled);
            assert_eq!(map_session_status("stopped"), SessionStatus::Cancelled);
        }

        #[test]
        fn map_status_unknown_defaults_to_failed() {
            assert_eq!(map_session_status("unknown"), SessionStatus::Failed);
            assert_eq!(map_session_status("garbage"), SessionStatus::Failed);
        }

        #[test]
        fn map_status_case_insensitive() {
            assert_eq!(map_session_status("RUNNING"), SessionStatus::Running);
            assert_eq!(map_session_status("Completed"), SessionStatus::Completed);
            assert_eq!(map_session_status("FAILED"), SessionStatus::Failed);
        }

        #[test]
        fn map_status_empty_string_defaults_to_failed() {
            assert_eq!(map_session_status(""), SessionStatus::Failed);
        }

        #[test]
        fn map_status_mixed_case_variants() {
            assert_eq!(map_session_status("DONE"), SessionStatus::Completed);
            assert_eq!(
                map_session_status("Cancelled"),
                SessionStatus::Cancelled
            );
            assert_eq!(map_session_status("TIMED_OUT"), SessionStatus::TimedOut);
            assert_eq!(map_session_status("Pending"), SessionStatus::Running);
            assert_eq!(map_session_status("Error"), SessionStatus::Failed);
            assert_eq!(map_session_status("Stopped"), SessionStatus::Cancelled);
        }

        // ─── Cost extraction ────────────────────────────────

        #[test]
        fn extract_cost_with_metrics() {
            let body = serde_json::json!({
                "metrics": {
                    "input_tokens": 1500,
                    "output_tokens": 300,
                    "model": "claude-sonnet",
                    "cost": 0.0045
                }
            });
            let cost = extract_cost_from_metrics(&body).unwrap();
            assert_eq!(cost.input_tokens, Some(1500));
            assert_eq!(cost.output_tokens, Some(300));
            assert_eq!(cost.model, Some("claude-sonnet".into()));
            assert_eq!(cost.estimated_usd, Some(0.0045));
        }

        #[test]
        fn extract_cost_no_metrics() {
            let body = serde_json::json!({});
            assert!(extract_cost_from_metrics(&body).is_none());
        }

        #[test]
        fn extract_cost_empty_metrics() {
            let body = serde_json::json!({ "metrics": {} });
            assert!(extract_cost_from_metrics(&body).is_none());
        }

        #[test]
        fn extract_cost_partial_metrics() {
            let body = serde_json::json!({ "metrics": { "input_tokens": 500 } });
            let cost = extract_cost_from_metrics(&body).unwrap();
            assert_eq!(cost.input_tokens, Some(500));
            assert_eq!(cost.output_tokens, None);
            assert_eq!(cost.model, None);
        }

        #[test]
        fn extract_cost_only_output_tokens() {
            let body = serde_json::json!({ "metrics": { "output_tokens": 250 } });
            let cost = extract_cost_from_metrics(&body).unwrap();
            assert_eq!(cost.input_tokens, None);
            assert_eq!(cost.output_tokens, Some(250));
        }

        #[test]
        fn extract_cost_non_numeric_tokens_returns_none() {
            let body = serde_json::json!({
                "metrics": {
                    "input_tokens": "five hundred",
                    "output_tokens": "two hundred"
                }
            });
            assert!(extract_cost_from_metrics(&body).is_none());
        }

        #[test]
        fn extract_cost_all_fields_present() {
            let body = serde_json::json!({
                "metrics": {
                    "input_tokens": 1000,
                    "output_tokens": 200,
                    "model": "gpt-4o",
                    "cost": 0.012
                }
            });
            let cost = extract_cost_from_metrics(&body).unwrap();
            assert_eq!(cost.input_tokens, Some(1000));
            assert_eq!(cost.output_tokens, Some(200));
            assert_eq!(cost.model, Some("gpt-4o".into()));
            assert_eq!(cost.estimated_usd, Some(0.012));
        }

        #[test]
        fn extract_cost_metrics_is_not_object() {
            let body = serde_json::json!({ "metrics": "not an object" });
            assert!(extract_cost_from_metrics(&body).is_none());
        }

        #[test]
        fn extract_cost_metrics_null() {
            let body = serde_json::json!({ "metrics": null });
            assert!(extract_cost_from_metrics(&body).is_none());
        }

        // ─── Adapter::new ───────────────────────────────────

        #[test]
        fn new_strips_trailing_slash() {
            let adapter =
                <$adapter_ty>::new("http://localhost:3000/".into(), None, None, true, 600, 30)
                    .unwrap();
            let debug = format!("{:?}", adapter);
            assert!(
                debug.contains("http://localhost:3000\""),
                "Expected trailing slash to be stripped, got: {debug}"
            );
        }

        #[test]
        fn new_strips_trailing_slash_preserves_path() {
            let adapter = <$adapter_ty>::new(
                "http://localhost:3000/api/v1/".into(),
                None,
                None,
                true,
                600,
                30,
            )
            .unwrap();
            let debug = format!("{:?}", adapter);
            assert!(
                debug.contains("http://localhost:3000/api/v1\""),
                "Expected only trailing slash stripped, got: {debug}"
            );
        }

        #[test]
        fn new_no_trailing_slash_unchanged() {
            let adapter =
                <$adapter_ty>::new("http://localhost:3000".into(), None, None, true, 600, 30)
                    .unwrap();
            let debug = format!("{:?}", adapter);
            assert!(
                debug.contains("http://localhost:3000\""),
                "URL without trailing slash should be unchanged, got: {debug}"
            );
        }

        #[test]
        fn new_missing_env_var_returns_config_error() {
            let env_var = concat!("BATON_TEST_", $env_prefix, "_NONEXISTENT_KEY_12345");
            let result = <$adapter_ty>::new(
                "http://localhost".into(),
                Some(env_var),
                None,
                true,
                600,
                30,
            );
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains(env_var), "Error should mention the env var name, got: {err}");
            assert!(err.contains("not set"), "Error should say 'not set', got: {err}");
        }

        #[test]
        fn new_empty_env_var_name_treated_as_none() {
            let result =
                <$adapter_ty>::new("http://localhost".into(), Some(""), None, true, 600, 30);
            assert!(result.is_ok(), "Empty env var name should succeed");
            let debug = format!("{:?}", result.unwrap());
            assert!(
                debug.contains("api_key: None"),
                "Empty env var name should result in no api key, got: {debug}"
            );
        }

        #[test]
        fn new_valid_env_var_is_resolved() {
            let env_var = concat!("BATON_TEST_", $env_prefix, "_KEY");
            std::env::set_var(env_var, "test-secret-456");
            let result = <$adapter_ty>::new(
                "http://localhost".into(),
                Some(env_var),
                None,
                true,
                600,
                30,
            );
            std::env::remove_var(env_var);
            assert!(result.is_ok(), "Valid env var should succeed");
            let debug = format!("{:?}", result.unwrap());
            assert!(
                debug.contains("test-secret-456"),
                "Adapter should contain the resolved key, got: {debug}"
            );
        }

        #[test]
        fn new_stores_default_model() {
            let adapter = <$adapter_ty>::new(
                "http://localhost".into(),
                None,
                Some("gpt-4o".into()),
                false,
                300,
                10,
            )
            .unwrap();
            assert_eq!(adapter.base.default_model, Some("gpt-4o".into()));
            assert!(!adapter.base.sandbox);
            assert_eq!(adapter.base.timeout_seconds, 300);
            assert_eq!(adapter.base.max_iterations, 10);
        }

        // ─── auth_headers ───────────────────────────────────

        #[test]
        fn adapter_without_api_key_has_no_auth_in_debug() {
            let adapter =
                <$adapter_ty>::new("http://localhost".into(), None, None, true, 600, 30).unwrap();
            let headers = adapter.base.auth_headers();
            assert!(headers.is_empty(), "No API key should produce empty headers");
        }

        #[test]
        fn adapter_with_api_key_has_auth_header() {
            let env_var = concat!("BATON_TEST_", $env_prefix, "_AUTH_HEADER_KEY");
            std::env::set_var(env_var, "my-key");
            let adapter = <$adapter_ty>::new(
                "http://localhost".into(),
                Some(env_var),
                None,
                true,
                600,
                30,
            )
            .unwrap();
            std::env::remove_var(env_var);
            let headers = adapter.base.auth_headers();
            assert_eq!(headers.len(), 1);
            let auth_val = headers.get(reqwest::header::AUTHORIZATION).unwrap();
            assert_eq!(auth_val.to_str().unwrap(), "Bearer my-key");
        }

        // ─── HTTP-level tests (httpmock) ────────────────────

        fn test_adapter(server: &httpmock::MockServer) -> $adapter_ty {
            let factory: fn(&httpmock::MockServer) -> $adapter_ty = $adapter_factory;
            factory(server)
        }

        fn test_handle() -> SessionHandle {
            SessionHandle {
                id: "sess-123".into(),
                workspace_id: "ws-456".into(),
            }
        }

        // ─── health_check HTTP tests ────────────────────────

        #[test]
        fn http_health_check_success() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET).path("/api/health");
                then.status(200)
                    .json_body(serde_json::json!({"version": "1.2.3"}));
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.health_check().unwrap();
            assert!(result.reachable);
            assert_eq!(result.version, Some("1.2.3".into()));
            mock.assert();
        }

        #[test]
        fn http_health_check_http_error() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET).path("/api/health");
                then.status(503);
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.health_check().unwrap();
            assert!(!result.reachable);
            assert!(result.message.unwrap().contains("503"));
            mock.assert();
        }

        #[test]
        fn http_health_check_malformed_json() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET).path("/api/health");
                then.status(200).body("not json");
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.health_check().unwrap();
            assert!(result.reachable);
            assert_eq!(result.version, None);
            mock.assert();
        }

        #[test]
        fn http_health_check_connection_refused() {
            let base = $crate::runtime::session_common::SessionAdapterBase {
                base_url: "http://127.0.0.1:1".into(),
                api_key: None,
                default_model: None,
                sandbox: false,
                timeout_seconds: 30,
                max_iterations: 10,
                client: reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .unwrap(),
            };

            let result = base.health_check();
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("Cannot reach"), "Error: {err}");
        }

        // ─── create_session HTTP tests ──────────────────────

        #[test]
        fn http_create_session_success_no_files() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST).path("/api/sessions");
                then.status(200)
                    .json_body(serde_json::json!({"session_id": "s1"}));
            });

            let adapter = test_adapter(&server);
            let config = SessionConfig {
                task: "Review code".into(),
                files: std::collections::BTreeMap::new(),
                model: "test-model".into(),
                sandbox: false,
                max_iterations: 10,
                timeout_seconds: 30,
                env: std::collections::BTreeMap::new(),
            };

            let handle = adapter.base.create_session(config).unwrap();
            assert_eq!(handle.id, "s1");
            mock.assert();
        }

        #[test]
        fn http_create_session_success_with_files() {
            let server = httpmock::MockServer::start();

            let upload_mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST)
                    .path_includes("/api/workspaces/")
                    .path_includes("/files");
                then.status(200).body("ok");
            });

            let session_mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST).path("/api/sessions");
                then.status(200)
                    .json_body(serde_json::json!({"session_id": "s2"}));
            });

            let tmp = tempfile::NamedTempFile::new().unwrap();
            std::io::Write::write_all(
                &mut tmp.as_file().try_clone().unwrap(),
                b"file content",
            )
            .unwrap();

            let mut files = std::collections::BTreeMap::new();
            files.insert("test.py".into(), tmp.path().to_str().unwrap().to_string());

            let adapter = test_adapter(&server);
            let config = SessionConfig {
                task: "Review code".into(),
                files,
                model: "test-model".into(),
                sandbox: false,
                max_iterations: 10,
                timeout_seconds: 30,
                env: std::collections::BTreeMap::new(),
            };

            let handle = adapter.base.create_session(config).unwrap();
            assert_eq!(handle.id, "s2");
            upload_mock.assert();
            session_mock.assert();
        }

        #[test]
        fn http_create_session_file_upload_http_error() {
            let server = httpmock::MockServer::start();
            let _mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST)
                    .path_includes("/api/workspaces/");
                then.status(500).body("upload failed");
            });

            let tmp = tempfile::NamedTempFile::new().unwrap();
            std::io::Write::write_all(
                &mut tmp.as_file().try_clone().unwrap(),
                b"content",
            )
            .unwrap();

            let mut files = std::collections::BTreeMap::new();
            files.insert("bad.py".into(), tmp.path().to_str().unwrap().to_string());

            let adapter = test_adapter(&server);
            let config = SessionConfig {
                task: "task".into(),
                files,
                model: "m".into(),
                sandbox: false,
                max_iterations: 10,
                timeout_seconds: 30,
                env: std::collections::BTreeMap::new(),
            };

            let result = adapter.base.create_session(config);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("bad.py"), "Error should mention file: {err}");
        }

        #[test]
        fn http_create_session_http_error() {
            let server = httpmock::MockServer::start();
            let _mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST).path("/api/sessions");
                then.status(400).body("bad request");
            });

            let adapter = test_adapter(&server);
            let config = SessionConfig {
                task: "task".into(),
                files: std::collections::BTreeMap::new(),
                model: "m".into(),
                sandbox: false,
                max_iterations: 10,
                timeout_seconds: 30,
                env: std::collections::BTreeMap::new(),
            };

            let result = adapter.base.create_session(config);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("400"), "Error should mention status: {err}");
        }

        #[test]
        fn http_create_session_missing_session_id() {
            let server = httpmock::MockServer::start();
            let _mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST).path("/api/sessions");
                then.status(200).json_body(serde_json::json!({}));
            });

            let adapter = test_adapter(&server);
            let config = SessionConfig {
                task: "task".into(),
                files: std::collections::BTreeMap::new(),
                model: "m".into(),
                sandbox: false,
                max_iterations: 10,
                timeout_seconds: 30,
                env: std::collections::BTreeMap::new(),
            };

            let result = adapter.base.create_session(config);
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(
                err.contains("session_id"),
                "Error should mention missing field: {err}"
            );
        }

        #[test]
        fn http_create_session_unparseable_json() {
            let server = httpmock::MockServer::start();
            let _mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST).path("/api/sessions");
                then.status(200).body("not json");
            });

            let adapter = test_adapter(&server);
            let config = SessionConfig {
                task: "task".into(),
                files: std::collections::BTreeMap::new(),
                model: "m".into(),
                sandbox: false,
                max_iterations: 10,
                timeout_seconds: 30,
                env: std::collections::BTreeMap::new(),
            };

            let result = adapter.base.create_session(config);
            assert!(result.is_err());
        }

        #[test]
        fn http_create_session_body_contents() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::POST)
                    .path("/api/sessions")
                    .json_body_includes(r#"{"task": "Review code"}"#)
                    .json_body_includes(r#"{"model": "gpt-4o"}"#)
                    .json_body_includes(r#"{"sandbox": true}"#);
                then.status(200)
                    .json_body(serde_json::json!({"session_id": "s3"}));
            });

            let adapter = test_adapter(&server);
            let config = SessionConfig {
                task: "Review code".into(),
                files: std::collections::BTreeMap::new(),
                model: "gpt-4o".into(),
                sandbox: true,
                max_iterations: 10,
                timeout_seconds: 30,
                env: std::collections::BTreeMap::new(),
            };

            adapter.base.create_session(config).unwrap();
            mock.assert();
        }

        // ─── poll_status HTTP tests ─────────────────────────

        #[test]
        fn http_poll_status_running() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/status");
                then.status(200)
                    .json_body(serde_json::json!({"status": "running"}));
            });

            let adapter = test_adapter(&server);
            let status = adapter.base.poll_status(&test_handle()).unwrap();
            assert_eq!(status, SessionStatus::Running);
            mock.assert();
        }

        #[test]
        fn http_poll_status_completed() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/status");
                then.status(200)
                    .json_body(serde_json::json!({"status": "completed"}));
            });

            let adapter = test_adapter(&server);
            let status = adapter.base.poll_status(&test_handle()).unwrap();
            assert_eq!(status, SessionStatus::Completed);
            mock.assert();
        }

        #[test]
        fn http_poll_status_http_error() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/status");
                then.status(500);
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.poll_status(&test_handle());
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("poll"), "Error: {err}");
            mock.assert();
        }

        #[test]
        fn http_poll_status_missing_status_field() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/status");
                then.status(200).json_body(serde_json::json!({}));
            });

            let adapter = test_adapter(&server);
            let status = adapter.base.poll_status(&test_handle()).unwrap();
            assert_eq!(status, SessionStatus::Failed);
            mock.assert();
        }

        #[test]
        fn http_poll_status_unparseable_json() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/status");
                then.status(200).body("not json");
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.poll_status(&test_handle());
            assert!(result.is_err());
            mock.assert();
        }

        // ─── collect_result HTTP tests ──────────────────────

        #[test]
        fn http_collect_result_success() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/result");
                then.status(200).json_body(serde_json::json!({
                    "status": "completed",
                    "final_message": "PASS — all good",
                    "full_log": "log line 1\nlog line 2",
                    "metrics": {
                        "input_tokens": 500,
                        "output_tokens": 100,
                        "model": "gpt-4o",
                        "cost": 0.005
                    }
                }));
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.collect_result(&test_handle()).unwrap();
            assert_eq!(result.status, SessionStatus::Completed);
            assert_eq!(result.output, "PASS — all good");
            assert!(result.raw_log.contains("log line 1"));
            let cost = result.cost.unwrap();
            assert_eq!(cost.input_tokens, Some(500));
            assert_eq!(cost.output_tokens, Some(100));
            mock.assert();
        }

        #[test]
        fn http_collect_result_missing_fields() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/result");
                then.status(200).json_body(serde_json::json!({}));
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.collect_result(&test_handle()).unwrap();
            assert_eq!(result.output, "");
            assert_eq!(result.raw_log, "");
            assert!(result.cost.is_none());
            mock.assert();
        }

        #[test]
        fn http_collect_result_http_error() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/result");
                then.status(500);
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.collect_result(&test_handle());
            assert!(result.is_err());
            mock.assert();
        }

        #[test]
        fn http_collect_result_unparseable_json() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/api/sessions/sess-123/result");
                then.status(200).body("not json");
            });

            let adapter = test_adapter(&server);
            let result = adapter.base.collect_result(&test_handle());
            assert!(result.is_err());
            mock.assert();
        }

        // ─── cancel HTTP tests ──────────────────────────────

        #[test]
        fn http_cancel_sends_delete() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::DELETE)
                    .path("/api/sessions/sess-123");
                then.status(200);
            });

            let adapter = test_adapter(&server);
            adapter.base.cancel(&test_handle()).unwrap();
            mock.assert();
        }

        #[test]
        fn http_cancel_ignores_errors() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::DELETE)
                    .path("/api/sessions/sess-123");
                then.status(500);
            });

            let adapter = test_adapter(&server);
            adapter.base.cancel(&test_handle()).unwrap();
            mock.assert();
        }

        // ─── teardown HTTP tests ────────────────────────────

        #[test]
        fn http_teardown_sends_delete() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::DELETE)
                    .path("/api/workspaces/ws-456");
                then.status(200);
            });

            let adapter = test_adapter(&server);
            adapter.base.teardown(&test_handle()).unwrap();
            mock.assert();
        }

        #[test]
        fn http_teardown_ignores_errors() {
            let server = httpmock::MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(httpmock::Method::DELETE)
                    .path("/api/workspaces/ws-456");
                then.status(500);
            });

            let adapter = test_adapter(&server);
            adapter.base.teardown(&test_handle()).unwrap();
            mock.assert();
        }
    };
}

#[cfg(test)]
pub(crate) use session_adapter_tests;
