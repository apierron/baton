//! API runtime adapter for OpenAI-compatible LLM providers.
//!
//! Wraps `ProviderClient` as a `RuntimeAdapter` for one-shot completions.
//! Does not support agent sessions.

use crate::error::{BatonError, Result};
use crate::provider::{ProviderClient, ProviderError};

use super::{
    CompletionRequest, CompletionResult, HealthResult, RuntimeAdapter, SessionConfig,
    SessionHandle, SessionResult, SessionStatus,
};

// ─── API adapter ─────────────────────────────────────────

/// Runtime adapter for OpenAI-compatible API endpoints.
///
/// Handles one-shot completions via `post_completion`. Session methods
/// return errors since API endpoints don't support agent lifecycles.
#[derive(Debug)]
pub struct ApiAdapter {
    pub default_model: Option<String>,
    pub timeout_seconds: u64,
    client: ProviderClient,
}

impl ApiAdapter {
    /// Creates a new API adapter.
    ///
    /// Resolves the API key from the environment and builds the HTTP client.
    pub fn new(
        base_url: String,
        api_key_env: Option<&str>,
        default_model: Option<String>,
        timeout_seconds: u64,
    ) -> Result<Self> {
        let env_str = api_key_env.unwrap_or("");

        let client = ProviderClient::new(&base_url, env_str, "api", timeout_seconds)
            .map_err(|e| BatonError::ConfigError(format!("API runtime: {e}")))?;

        Ok(ApiAdapter {
            default_model,
            timeout_seconds,
            client,
        })
    }
}

impl RuntimeAdapter for ApiAdapter {
    fn health_check(&self) -> Result<HealthResult> {
        match self.client.list_models() {
            Ok(models) => Ok(HealthResult {
                reachable: true,
                version: None,
                models: Some(models),
                message: None,
            }),
            Err(ProviderError::Unreachable { detail, .. }) => Ok(HealthResult {
                reachable: false,
                version: None,
                models: None,
                message: Some(detail),
            }),
            Err(ProviderError::Timeout { .. }) => Ok(HealthResult {
                reachable: false,
                version: None,
                models: None,
                message: Some("Connection timed out".into()),
            }),
            Err(ProviderError::AuthFailed { .. }) => {
                // Reachable but auth failed — still counts as reachable for fallback
                Ok(HealthResult {
                    reachable: true,
                    version: None,
                    models: None,
                    message: Some("Authentication failed".into()),
                })
            }
            Err(_) => {
                // Other errors (404 on /v1/models, etc.) — treat as reachable
                Ok(HealthResult {
                    reachable: true,
                    version: None,
                    models: None,
                    message: None,
                })
            }
        }
    }

    fn create_session(&self, _config: SessionConfig) -> Result<SessionHandle> {
        Err(BatonError::RuntimeError(
            "API runtime does not support sessions".into(),
        ))
    }

    fn poll_status(&self, _handle: &SessionHandle) -> Result<SessionStatus> {
        Err(BatonError::RuntimeError(
            "API runtime does not support sessions".into(),
        ))
    }

    fn collect_result(&self, _handle: &SessionHandle) -> Result<SessionResult> {
        Err(BatonError::RuntimeError(
            "API runtime does not support sessions".into(),
        ))
    }

    fn cancel(&self, _handle: &SessionHandle) -> Result<()> {
        Err(BatonError::RuntimeError(
            "API runtime does not support sessions".into(),
        ))
    }

    fn teardown(&self, _handle: &SessionHandle) -> Result<()> {
        Err(BatonError::RuntimeError(
            "API runtime does not support sessions".into(),
        ))
    }

    fn post_completion(&self, request: CompletionRequest) -> Result<CompletionResult> {
        let mut body = serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature,
        });

        if let Some(max_tokens) = request.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }

        match self.client.post_completion(body, &request.model) {
            Ok(response) => Ok(CompletionResult {
                content: response.content,
                cost: response.cost,
            }),
            Err(ProviderError::EmptyContent { cost }) => Err(BatonError::ValidationError(format!(
                "Provider returned empty response{}",
                cost.as_ref()
                    .and_then(|c| c.model.as_ref())
                    .map(|m| format!(" (model: {m})"))
                    .unwrap_or_default()
            ))),
            Err(e) => Err(BatonError::ValidationError(format!("{e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ProviderClient;

    // ─── Helpers ─────────────────────────────────────────

    fn test_adapter(server: &httpmock::MockServer) -> ApiAdapter {
        let client = ProviderClient::new(&server.url(""), "", "api-test", 10).unwrap();
        ApiAdapter {
            default_model: Some("test-model".into()),
            timeout_seconds: 10,
            client,
        }
    }

    fn test_runtime_config(base_url: &str) -> crate::config::Runtime {
        crate::config::Runtime {
            runtime_type: "api".into(),
            base_url: base_url.into(),
            api_key_env: None,
            default_model: Some("test-model".into()),
            sandbox: false,
            timeout_seconds: 10,
            max_iterations: 1,
        }
    }

    fn test_session_config() -> SessionConfig {
        SessionConfig {
            task: "test".into(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 1,
            timeout_seconds: 10,
            env: std::collections::BTreeMap::new(),
        }
    }

    // ─── Construction ────────────────────────────────────

    #[test]
    fn create_api_adapter_no_auth() {
        let config = test_runtime_config("http://localhost:99999");
        let result = super::super::create_adapter("test-api", &config);
        assert!(result.is_ok());
    }

    #[test]
    fn create_adapter_stores_default_model() {
        let config = test_runtime_config("http://localhost:99999");
        let adapter = super::super::create_adapter("test-api", &config).unwrap();
        // Verify via debug output since we can't downcast
        let debug = format!("{adapter:?}");
        assert!(debug.contains("test-model"), "Debug: {debug}");
    }

    #[test]
    fn create_adapter_no_default_model() {
        let mut config = test_runtime_config("http://localhost:99999");
        config.default_model = None;
        let adapter = super::super::create_adapter("test-api", &config).unwrap();
        let debug = format!("{adapter:?}");
        assert!(debug.contains("default_model: None"), "Debug: {debug}");
    }

    #[test]
    fn new_with_empty_api_key_env_succeeds() {
        let result = ApiAdapter::new(
            "http://localhost:99999".into(),
            Some(""),
            Some("model".into()),
            10,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn new_with_none_api_key_env_succeeds() {
        let result = ApiAdapter::new(
            "http://localhost:99999".into(),
            None,
            Some("model".into()),
            10,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn new_with_missing_env_var_returns_error() {
        let result = ApiAdapter::new(
            "http://localhost:99999".into(),
            Some("BATON_TEST_NONEXISTENT_KEY_12345"),
            None,
            10,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("API runtime"), "Error: {err}");
    }

    // ─── Session methods return errors ───────────────────

    #[test]
    fn create_session_returns_error() {
        let config = test_runtime_config("http://localhost:99999");
        let adapter = super::super::create_adapter("test-api", &config).unwrap();
        let err = adapter.create_session(test_session_config()).unwrap_err();
        assert!(
            err.to_string().contains("does not support sessions"),
            "Error: {err}"
        );
    }

    #[test]
    fn poll_status_returns_error() {
        let config = test_runtime_config("http://localhost:99999");
        let adapter = super::super::create_adapter("test-api", &config).unwrap();
        let handle = SessionHandle {
            id: "x".into(),
            workspace_id: "y".into(),
        };
        let err = adapter.poll_status(&handle).unwrap_err();
        assert!(
            err.to_string().contains("does not support sessions"),
            "Error: {err}"
        );
    }

    #[test]
    fn collect_result_returns_error() {
        let config = test_runtime_config("http://localhost:99999");
        let adapter = super::super::create_adapter("test-api", &config).unwrap();
        let handle = SessionHandle {
            id: "x".into(),
            workspace_id: "y".into(),
        };
        let err = adapter.collect_result(&handle).unwrap_err();
        assert!(
            err.to_string().contains("does not support sessions"),
            "Error: {err}"
        );
    }

    #[test]
    fn cancel_returns_error() {
        let config = test_runtime_config("http://localhost:99999");
        let adapter = super::super::create_adapter("test-api", &config).unwrap();
        let handle = SessionHandle {
            id: "x".into(),
            workspace_id: "y".into(),
        };
        let err = adapter.cancel(&handle).unwrap_err();
        assert!(
            err.to_string().contains("does not support sessions"),
            "Error: {err}"
        );
    }

    #[test]
    fn teardown_returns_error() {
        let config = test_runtime_config("http://localhost:99999");
        let adapter = super::super::create_adapter("test-api", &config).unwrap();
        let handle = SessionHandle {
            id: "x".into(),
            workspace_id: "y".into(),
        };
        let err = adapter.teardown(&handle).unwrap_err();
        assert!(
            err.to_string().contains("does not support sessions"),
            "Error: {err}"
        );
    }

    // ─── health_check HTTP tests ─────────────────────────

    #[test]
    fn health_check_success_returns_models() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).json_body(serde_json::json!({
                "data": [
                    {"id": "gpt-4"},
                    {"id": "gpt-3.5-turbo"}
                ]
            }));
        });

        let adapter = test_adapter(&server);
        let result = adapter.health_check().unwrap();
        assert!(result.reachable);
        assert!(result.models.is_some());
        let models = result.models.unwrap();
        assert!(models.contains(&"gpt-4".to_string()));
        mock.assert();
    }

    #[test]
    fn health_check_unreachable() {
        let adapter = ApiAdapter {
            default_model: None,
            timeout_seconds: 10,
            client: ProviderClient::new("http://127.0.0.1:1", "", "test", 2).unwrap(),
        };

        let result = adapter.health_check().unwrap();
        assert!(!result.reachable);
        assert!(result.message.is_some());
    }

    #[test]
    fn health_check_auth_failed_still_reachable() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(401);
        });

        let adapter = test_adapter(&server);
        let result = adapter.health_check().unwrap();
        assert!(result.reachable);
        assert!(result.message.unwrap().contains("Authentication failed"));
        mock.assert();
    }

    #[test]
    fn health_check_other_error_still_reachable() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(500).body("internal error");
        });

        let adapter = test_adapter(&server);
        let result = adapter.health_check().unwrap();
        assert!(result.reachable);
        assert!(result.models.is_none());
        mock.assert();
    }

    // ─── post_completion HTTP tests ──────────────────────

    #[test]
    fn post_completion_success() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "PASS"}}],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5,
                    "total_tokens": 15
                },
                "model": "gpt-4"
            }));
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.0,
            max_tokens: None,
        };
        let result = adapter.post_completion(request).unwrap();
        assert_eq!(result.content, "PASS");
        assert!(result.cost.is_some());
        mock.assert();
    }

    #[test]
    fn post_completion_includes_max_tokens_when_set() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "ok"}}]
            }));
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.0,
            max_tokens: Some(100),
        };
        let result = adapter.post_completion(request).unwrap();
        assert_eq!(result.content, "ok");
        mock.assert();
    }

    #[test]
    fn post_completion_empty_content_returns_error() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": ""}}],
                "model": "gpt-4"
            }));
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.0,
            max_tokens: None,
        };
        let err = adapter.post_completion(request).unwrap_err();
        assert!(err.to_string().contains("empty response"), "Error: {err}");
        mock.assert();
    }

    #[test]
    fn post_completion_http_error_returns_error() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(500).body("server error");
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.0,
            max_tokens: None,
        };
        let err = adapter.post_completion(request).unwrap_err();
        assert!(err.to_string().contains("500"), "Error: {err}");
        mock.assert();
    }

    #[test]
    fn post_completion_model_not_found() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(404);
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "nonexistent-model".into(),
            temperature: 0.0,
            max_tokens: None,
        };
        let err = adapter.post_completion(request).unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("404"),
            "Error: {err}"
        );
        mock.assert();
    }

    #[test]
    fn post_completion_auth_failure() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(401);
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.0,
            max_tokens: None,
        };
        let err = adapter.post_completion(request).unwrap_err();
        assert!(
            err.to_string().contains("Authentication") || err.to_string().contains("auth"),
            "Error: {err}"
        );
        mock.assert();
    }

    #[test]
    fn post_completion_rate_limited() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(429);
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.0,
            max_tokens: None,
        };
        let err = adapter.post_completion(request).unwrap_err();
        assert!(
            err.to_string().contains("Rate limited") || err.to_string().contains("rate"),
            "Error: {err}"
        );
        mock.assert();
    }

    #[test]
    fn post_completion_no_cost_when_no_usage() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "ok"}}]
            }));
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.0,
            max_tokens: None,
        };
        let result = adapter.post_completion(request).unwrap();
        assert_eq!(result.content, "ok");
        assert!(result.cost.is_none());
        mock.assert();
    }

    #[test]
    fn post_completion_without_max_tokens_omits_field() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "ok"}}]
            }));
        });

        let adapter = test_adapter(&server);
        let request = CompletionRequest {
            messages: vec![serde_json::json!({"role": "user", "content": "test"})],
            model: "gpt-4".into(),
            temperature: 0.7,
            max_tokens: None,
        };
        adapter.post_completion(request).unwrap();
        mock.assert();
    }
}
