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
    #[allow(dead_code)]
    base_url: String,
    #[allow(dead_code)]
    api_key_env: String,
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
            base_url,
            api_key_env: env_str.to_string(),
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
    #[test]
    fn session_methods_return_error() {
        // ApiAdapter can't be constructed without a valid base_url,
        // but we can test that the trait default is overridden
        // by checking the error message pattern
        let config = crate::config::Runtime {
            runtime_type: "api".into(),
            base_url: "http://localhost:99999".into(),
            api_key_env: None,
            default_model: Some("test".into()),
            sandbox: false,
            timeout_seconds: 10,
            max_iterations: 1,
        };
        let adapter = super::super::create_adapter("test-api", &config).unwrap();

        let session_config = super::super::SessionConfig {
            task: "test".into(),
            files: std::collections::BTreeMap::new(),
            model: "test".into(),
            sandbox: false,
            max_iterations: 1,
            timeout_seconds: 10,
            env: std::collections::BTreeMap::new(),
        };

        let err = adapter.create_session(session_config).unwrap_err();
        assert!(
            err.to_string().contains("does not support sessions"),
            "Error: {err}"
        );
    }

    #[test]
    fn create_api_adapter_no_auth() {
        let config = crate::config::Runtime {
            runtime_type: "api".into(),
            base_url: "http://localhost:99999".into(),
            api_key_env: None,
            default_model: Some("test-model".into()),
            sandbox: false,
            timeout_seconds: 10,
            max_iterations: 1,
        };
        let result = super::super::create_adapter("test-api", &config);
        assert!(result.is_ok());
    }
}
