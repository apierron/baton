//! OpenHands runtime adapter implementation.
//!
//! Communicates with an OpenHands server via its REST API to create
//! agent sessions, poll status, and collect results.

use crate::error::{BatonError, Result};
use crate::types::Cost;

use super::{
    HealthResult, RuntimeAdapter, SessionConfig, SessionHandle, SessionResult, SessionStatus,
};

// ─── OpenHands adapter ──────────────────────────────────

/// HTTP client adapter for the OpenHands agent runtime.
#[derive(Debug)]
pub struct OpenHandsAdapter {
    base_url: String,
    api_key: Option<String>,
    pub default_model: Option<String>,
    pub sandbox: bool,
    pub timeout_seconds: u64,
    pub max_iterations: u32,
    client: reqwest::blocking::Client,
}

impl OpenHandsAdapter {
    /// Creates a new adapter from connection parameters.
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

        Ok(OpenHandsAdapter {
            base_url: base,
            api_key,
            default_model,
            sandbox,
            timeout_seconds,
            max_iterations,
            client,
        })
    }

    fn auth_headers(&self) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(ref key) = self.api_key {
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
        }
        headers
    }
}

impl RuntimeAdapter for OpenHandsAdapter {
    fn health_check(&self) -> Result<HealthResult> {
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

    fn create_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let workspace_id = uuid::Uuid::new_v4().to_string();

        // Upload files to workspace
        for (name, path) in &config.files {
            let file_content = std::fs::read(path).map_err(|e| {
                BatonError::ValidationError(format!(
                    "Failed to read file '{name}' at '{path}': {e}"
                ))
            })?;

            let url = format!(
                "{}/api/workspaces/{}/files",
                self.base_url, workspace_id
            );

            let part = reqwest::blocking::multipart::Part::bytes(file_content)
                .file_name(name.clone());
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
                BatonError::ValidationError(format!(
                    "Failed to create session on runtime: {e}"
                ))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().unwrap_or_default();
            return Err(BatonError::ValidationError(format!(
                "Failed to create session: HTTP {status}: {body_text}"
            )));
        }

        let resp_body: serde_json::Value = response.json().map_err(|e| {
            BatonError::ValidationError(format!(
                "Failed to parse session creation response: {e}"
            ))
        })?;

        let session_id = resp_body
            .get("session_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BatonError::ValidationError(
                    "Session creation response missing 'session_id'".into(),
                )
            })?
            .to_string();

        Ok(SessionHandle {
            id: session_id,
            workspace_id,
        })
    }

    fn poll_status(&self, handle: &SessionHandle) -> Result<SessionStatus> {
        let url = format!(
            "{}/api/sessions/{}/status",
            self.base_url, handle.id
        );

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

        Ok(map_openhands_status(status_str))
    }

    fn collect_result(&self, handle: &SessionHandle) -> Result<SessionResult> {
        let url = format!(
            "{}/api/sessions/{}/result",
            self.base_url, handle.id
        );

        let response = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .map_err(|e| {
                BatonError::ValidationError(format!(
                    "Failed to collect session result: {e}"
                ))
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

        let cost = extract_cost_from_openhands(&body);

        Ok(SessionResult {
            status: map_openhands_status(status_str),
            output,
            raw_log,
            cost,
        })
    }

    fn cancel(&self, handle: &SessionHandle) -> Result<()> {
        let url = format!("{}/api/sessions/{}", self.base_url, handle.id);

        // Idempotent: ignore errors on cancel
        let _ = self
            .client
            .delete(&url)
            .headers(self.auth_headers())
            .send();

        Ok(())
    }

    fn teardown(&self, handle: &SessionHandle) -> Result<()> {
        let url = format!(
            "{}/api/workspaces/{}",
            self.base_url, handle.workspace_id
        );

        // Idempotent: ignore errors on teardown
        let _ = self
            .client
            .delete(&url)
            .headers(self.auth_headers())
            .send();

        Ok(())
    }
}

// ─── Helpers ─────────────────────────────────────────────

fn map_openhands_status(status: &str) -> SessionStatus {
    match status.to_lowercase().as_str() {
        "running" | "pending" | "started" => SessionStatus::Running,
        "completed" | "finished" | "done" => SessionStatus::Completed,
        "failed" | "error" => SessionStatus::Failed,
        "timed_out" | "timeout" => SessionStatus::TimedOut,
        "cancelled" | "canceled" | "stopped" => SessionStatus::Cancelled,
        _ => SessionStatus::Failed,
    }
}

fn extract_cost_from_openhands(body: &serde_json::Value) -> Option<Cost> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ─── SessionStatus mapping ──────────────────────────

    #[test]
    fn map_status_running() {
        assert_eq!(map_openhands_status("running"), SessionStatus::Running);
        assert_eq!(map_openhands_status("pending"), SessionStatus::Running);
        assert_eq!(map_openhands_status("started"), SessionStatus::Running);
    }

    #[test]
    fn map_status_completed() {
        assert_eq!(map_openhands_status("completed"), SessionStatus::Completed);
        assert_eq!(map_openhands_status("finished"), SessionStatus::Completed);
        assert_eq!(map_openhands_status("done"), SessionStatus::Completed);
    }

    #[test]
    fn map_status_failed() {
        assert_eq!(map_openhands_status("failed"), SessionStatus::Failed);
        assert_eq!(map_openhands_status("error"), SessionStatus::Failed);
    }

    #[test]
    fn map_status_timed_out() {
        assert_eq!(map_openhands_status("timed_out"), SessionStatus::TimedOut);
        assert_eq!(map_openhands_status("timeout"), SessionStatus::TimedOut);
    }

    #[test]
    fn map_status_cancelled() {
        assert_eq!(map_openhands_status("cancelled"), SessionStatus::Cancelled);
        assert_eq!(map_openhands_status("canceled"), SessionStatus::Cancelled);
        assert_eq!(map_openhands_status("stopped"), SessionStatus::Cancelled);
    }

    #[test]
    fn map_status_unknown_defaults_to_failed() {
        assert_eq!(map_openhands_status("unknown"), SessionStatus::Failed);
        assert_eq!(map_openhands_status("garbage"), SessionStatus::Failed);
    }

    #[test]
    fn map_status_case_insensitive() {
        assert_eq!(map_openhands_status("RUNNING"), SessionStatus::Running);
        assert_eq!(map_openhands_status("Completed"), SessionStatus::Completed);
        assert_eq!(map_openhands_status("FAILED"), SessionStatus::Failed);
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
        let cost = extract_cost_from_openhands(&body).unwrap();
        assert_eq!(cost.input_tokens, Some(1500));
        assert_eq!(cost.output_tokens, Some(300));
        assert_eq!(cost.model, Some("claude-sonnet".into()));
        assert_eq!(cost.estimated_usd, Some(0.0045));
    }

    #[test]
    fn extract_cost_no_metrics() {
        let body = serde_json::json!({});
        assert!(extract_cost_from_openhands(&body).is_none());
    }

    #[test]
    fn extract_cost_empty_metrics() {
        let body = serde_json::json!({
            "metrics": {}
        });
        assert!(extract_cost_from_openhands(&body).is_none());
    }

    #[test]
    fn extract_cost_partial_metrics() {
        let body = serde_json::json!({
            "metrics": {
                "input_tokens": 500
            }
        });
        let cost = extract_cost_from_openhands(&body).unwrap();
        assert_eq!(cost.input_tokens, Some(500));
        assert_eq!(cost.output_tokens, None);
        assert_eq!(cost.model, None);
    }
}
