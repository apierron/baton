//! HTTP client for OpenAI-compatible LLM provider APIs.
//!
//! Centralizes API key resolution, auth header construction, HTTP error
//! classification, and response parsing for `/v1/chat/completions` and
//! `/v1/models` endpoints. Used by both the exec module (LLM validators)
//! and the CLI (provider health checks).

use crate::types::Cost;

// ─── Error type ──────────────────────────────────────────

/// Structured errors from provider HTTP interactions.
///
/// Each variant carries enough context for callers to produce
/// user-facing error messages without re-inspecting the provider config.
#[derive(Debug)]
pub enum ProviderError {
    /// The environment variable named by `api_key_env` is not set.
    ApiKeyNotSet { provider: String, env_var: String },
    /// The reqwest client could not be constructed.
    ClientBuildFailed(String),
    /// The provider server is unreachable (connection refused, DNS failure, etc.).
    Unreachable {
        provider: String,
        api_base: String,
        detail: String,
    },
    /// The HTTP request timed out.
    Timeout {
        provider: String,
        timeout_seconds: u64,
    },
    /// HTTP 401 or 403 — authentication failed.
    AuthFailed {
        provider: String,
        api_key_env: String,
    },
    /// HTTP 404 — model not found on the provider.
    ModelNotFound { model: String, provider: String },
    /// HTTP 429 — rate limited.
    RateLimited { provider: String },
    /// Other non-success HTTP status.
    HttpError { status: u16, body: String },
    /// Response body was not valid JSON or missing expected structure.
    MalformedResponse(String),
    /// Completion response had empty content in choices[0].message.content.
    EmptyContent {
        /// Cost is still extractable from a response with empty content.
        cost: Option<Cost>,
    },
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKeyNotSet { provider, env_var } => {
                write!(
                    f,
                    "Authentication failed for provider '{provider}'. \
                     Env var '{env_var}' is not set."
                )
            }
            Self::ClientBuildFailed(detail) => {
                write!(f, "Failed to create HTTP client: {detail}")
            }
            Self::Unreachable {
                provider,
                api_base,
                detail,
            } => {
                write!(
                    f,
                    "Cannot reach provider '{provider}' at {api_base}: {detail}"
                )
            }
            Self::Timeout {
                timeout_seconds, ..
            } => {
                write!(f, "Validator timed out after {timeout_seconds} seconds")
            }
            Self::AuthFailed {
                provider,
                api_key_env,
            } => {
                write!(
                    f,
                    "Authentication failed for provider '{provider}'. Check {api_key_env}."
                )
            }
            Self::ModelNotFound { model, provider } => {
                write!(f, "Model '{model}' not found on provider '{provider}'.")
            }
            Self::RateLimited { provider } => {
                write!(f, "Rate limited by provider '{provider}'.")
            }
            Self::HttpError { status, body } => {
                write!(f, "Provider returned HTTP {status}: {body}")
            }
            Self::MalformedResponse(detail) => {
                write!(f, "Provider returned empty or malformed response: {detail}")
            }
            Self::EmptyContent { .. } => {
                write!(f, "Provider returned empty or malformed response.")
            }
        }
    }
}

impl std::error::Error for ProviderError {}

// ─── Response types ──────────────────────────────────────

/// Parsed response from a `/v1/chat/completions` call.
#[derive(Debug)]
pub struct CompletionResponse {
    /// The text content from `choices[0].message.content`.
    pub content: String,
    /// Token usage and model info, if the response included a `usage` block.
    pub cost: Option<Cost>,
}

// ─── ProviderClient ──────────────────────────────────────

/// HTTP client for an OpenAI-compatible LLM provider.
///
/// Wraps `reqwest::blocking::Client` with provider-specific auth, URL
/// construction, and error classification. Constructed once and reused
/// for multiple requests against the same provider.
#[derive(Debug)]
pub struct ProviderClient {
    client: reqwest::blocking::Client,
    api_base: String,
    api_key: Option<String>,
    provider_name: String,
    api_key_env: String,
    timeout_seconds: u64,
}

impl ProviderClient {
    /// Creates a new client for the given provider/runtime.
    ///
    /// Resolves the API key from the environment if `api_key_env` is non-empty.
    /// Returns `Err(ProviderError::ApiKeyNotSet)` if the env var is required but missing.
    pub fn new(
        api_base: &str,
        api_key_env: &str,
        provider_name: &str,
        timeout_seconds: u64,
    ) -> Result<Self, ProviderError> {
        let api_key = if api_key_env.is_empty() {
            None
        } else {
            match std::env::var(api_key_env) {
                Ok(key) => Some(key),
                Err(_) => {
                    return Err(ProviderError::ApiKeyNotSet {
                        provider: provider_name.into(),
                        env_var: api_key_env.into(),
                    });
                }
            }
        };

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .build()
            .map_err(|e| ProviderError::ClientBuildFailed(e.to_string()))?;

        Ok(Self {
            client,
            api_base: api_base.into(),
            api_key,
            provider_name: provider_name.into(),
            api_key_env: api_key_env.into(),
            timeout_seconds,
        })
    }

    /// Returns the provider name (for error messages and logging).
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Returns the base URL.
    pub fn api_base(&self) -> &str {
        &self.api_base
    }

    /// Returns the `api_key_env` field name (for error messages).
    pub fn api_key_env(&self) -> &str {
        &self.api_key_env
    }

    /// Sends a chat completion request and parses the response.
    ///
    /// The caller is responsible for constructing the request body (model,
    /// messages, temperature, max_tokens, etc.). This method handles:
    /// - Auth header injection
    /// - HTTP error classification
    /// - JSON response parsing
    /// - Content extraction from `choices[0].message.content`
    /// - Cost extraction from `usage`
    pub fn post_completion(
        &self,
        body: serde_json::Value,
        model: &str,
    ) -> Result<CompletionResponse, ProviderError> {
        let url = format!("{}/v1/chat/completions", self.api_base);

        let mut req = self.client.post(&url).json(&body);
        req = self.apply_auth(req);

        let response = self.send_request(req)?;
        let status_code = response.status();

        if !status_code.is_success() {
            let body_text = response.text().unwrap_or_default();
            return Err(self.classify_http_error(status_code.as_u16(), body_text, model));
        }

        let resp_body: serde_json::Value = response
            .json()
            .map_err(|e| ProviderError::MalformedResponse(e.to_string()))?;

        let content = resp_body
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        let cost = extract_cost(&resp_body, model);

        if content.is_empty() {
            return Err(ProviderError::EmptyContent { cost });
        }

        Ok(CompletionResponse {
            content: content.to_string(),
            cost,
        })
    }

    /// Lists available models via `GET /v1/models`.
    ///
    /// Returns the model ID strings from the `data` array. If the endpoint
    /// is not available or returns a non-success status, returns the
    /// appropriate `ProviderError`.
    pub fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let url = format!("{}/v1/models", self.api_base);

        let mut req = self.client.get(&url);
        req = self.apply_auth(req);

        let response = self.send_request(req)?;
        let status_code = response.status();

        if !status_code.is_success() {
            let body_text = response.text().unwrap_or_default();
            return Err(self.classify_http_error(status_code.as_u16(), body_text, ""));
        }

        let body: serde_json::Value = response.json().unwrap_or_default();
        let models = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }

    /// Sends a minimal chat completion as a connectivity test.
    ///
    /// Uses `max_tokens: 1` to minimize cost. Returns `Ok(true)` if the
    /// provider responds successfully, or a `ProviderError` on failure.
    pub fn test_completion(&self, model: &str) -> Result<bool, ProviderError> {
        let url = format!("{}/v1/chat/completions", self.api_base);
        let body = serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
        });

        let mut req = self.client.post(&url).json(&body);
        req = self.apply_auth(req);

        let response = self.send_request(req)?;

        if response.status().is_success() {
            Ok(true)
        } else {
            let status = response.status().as_u16();
            let body_text = response.text().unwrap_or_default();
            Err(self.classify_http_error(status, body_text, model))
        }
    }

    // ─── Private helpers ─────────────────────────────

    /// Adds the Authorization header if an API key is present.
    fn apply_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        match self.api_key {
            Some(ref key) => req.header("Authorization", format!("Bearer {key}")),
            None => req,
        }
    }

    /// Sends a request, mapping connection and timeout errors to `ProviderError`.
    fn send_request(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::Response, ProviderError> {
        req.send().map_err(|e| {
            if e.is_timeout() {
                ProviderError::Timeout {
                    provider: self.provider_name.clone(),
                    timeout_seconds: self.timeout_seconds,
                }
            } else {
                ProviderError::Unreachable {
                    provider: self.provider_name.clone(),
                    api_base: self.api_base.clone(),
                    detail: e.to_string(),
                }
            }
        })
    }

    /// Maps an HTTP error status code to a structured `ProviderError`.
    fn classify_http_error(&self, status: u16, body_text: String, model: &str) -> ProviderError {
        match status {
            401 | 403 => ProviderError::AuthFailed {
                provider: self.provider_name.clone(),
                api_key_env: self.api_key_env.clone(),
            },
            404 => ProviderError::ModelNotFound {
                model: model.into(),
                provider: self.provider_name.clone(),
            },
            429 => ProviderError::RateLimited {
                provider: self.provider_name.clone(),
            },
            _ => ProviderError::HttpError {
                status,
                body: body_text,
            },
        }
    }
}

// ─── Cost extraction ─────────────────────────────────────

/// Extracts token usage and cost metadata from an OpenAI-compatible response body.
///
/// Returns `None` if the `usage` field is missing or has no token counts.
pub fn extract_cost(resp_body: &serde_json::Value, model: &str) -> Option<Cost> {
    let usage = resp_body.get("usage")?;
    let input = usage.get("prompt_tokens").and_then(|v| v.as_i64());
    let output = usage.get("completion_tokens").and_then(|v| v.as_i64());

    if input.is_none() && output.is_none() {
        return None;
    }

    Some(Cost {
        input_tokens: input,
        output_tokens: output,
        model: Some(model.to_string()),
        estimated_usd: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── extract_cost ────────────────────────────────

    #[test]
    fn extract_cost_full_usage() {
        let body = serde_json::json!({
            "usage": {
                "prompt_tokens": 1500,
                "completion_tokens": 300,
            }
        });
        let cost = extract_cost(&body, "claude-haiku").unwrap();
        assert_eq!(cost.input_tokens, Some(1500));
        assert_eq!(cost.output_tokens, Some(300));
        assert_eq!(cost.model, Some("claude-haiku".into()));
        assert!(cost.estimated_usd.is_none());
    }

    #[test]
    fn extract_cost_no_usage() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "PASS"}}]
        });
        assert!(extract_cost(&body, "model").is_none());
    }

    #[test]
    fn extract_cost_empty_usage() {
        let body = serde_json::json!({
            "usage": {}
        });
        assert!(extract_cost(&body, "model").is_none());
    }

    #[test]
    fn extract_cost_partial_usage() {
        let body = serde_json::json!({
            "usage": {
                "prompt_tokens": 100
            }
        });
        let cost = extract_cost(&body, "model").unwrap();
        assert_eq!(cost.input_tokens, Some(100));
        assert_eq!(cost.output_tokens, None);
    }

    #[test]
    fn extract_cost_only_completion_tokens() {
        let body = serde_json::json!({
            "usage": {
                "completion_tokens": 42
            }
        });
        let cost = extract_cost(&body, "model").unwrap();
        assert_eq!(cost.input_tokens, None);
        assert_eq!(cost.output_tokens, Some(42));
        assert_eq!(cost.model, Some("model".into()));
    }

    #[test]
    fn extract_cost_both_null_returns_none() {
        let body = serde_json::json!({
            "usage": {
                "prompt_tokens": null,
                "completion_tokens": null
            }
        });
        assert!(extract_cost(&body, "model").is_none());
    }

    #[test]
    fn extract_cost_usage_is_non_object() {
        let body = serde_json::json!({
            "usage": "not-an-object"
        });
        assert!(extract_cost(&body, "model").is_none());
    }

    // ─── ProviderError Display ───────────────────────

    #[test]
    fn error_display_api_key_not_set() {
        let e = ProviderError::ApiKeyNotSet {
            provider: "default".into(),
            env_var: "MY_KEY".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("default"));
        assert!(msg.contains("MY_KEY"));
    }

    #[test]
    fn error_display_auth_failed() {
        let e = ProviderError::AuthFailed {
            provider: "openai".into(),
            api_key_env: "OPENAI_KEY".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("Authentication failed"));
        assert!(msg.contains("openai"));
        assert!(msg.contains("OPENAI_KEY"));
    }

    #[test]
    fn error_display_model_not_found() {
        let e = ProviderError::ModelNotFound {
            model: "gpt-5".into(),
            provider: "openai".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("gpt-5"));
        assert!(msg.contains("not found"));
    }

    #[test]
    fn error_display_timeout() {
        let e = ProviderError::Timeout {
            provider: "default".into(),
            timeout_seconds: 30,
        };
        let msg = e.to_string();
        assert!(msg.contains("timed out"));
        assert!(msg.contains("30"));
    }

    #[test]
    fn error_display_unreachable() {
        let e = ProviderError::Unreachable {
            provider: "local".into(),
            api_base: "http://localhost:8080".into(),
            detail: "connection refused".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("Cannot reach"));
        assert!(msg.contains("localhost:8080"));
    }

    #[test]
    fn error_display_rate_limited() {
        let e = ProviderError::RateLimited {
            provider: "openai".into(),
        };
        assert!(e.to_string().contains("Rate limited"));
    }

    #[test]
    fn error_display_http_error() {
        let e = ProviderError::HttpError {
            status: 503,
            body: "service unavailable".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("503"));
        assert!(msg.contains("service unavailable"));
    }

    #[test]
    fn error_display_empty_content() {
        let e = ProviderError::EmptyContent { cost: None };
        assert!(e.to_string().contains("empty or malformed"));
    }

    // ─── ProviderClient construction ─────────────────

    #[test]
    fn new_with_empty_api_key_env() {
        let client = ProviderClient::new("http://localhost:8080", "", "test-provider", 30).unwrap();
        assert_eq!(client.provider_name(), "test-provider");
        assert_eq!(client.api_base(), "http://localhost:8080");
        assert!(client.api_key.is_none());
    }

    #[test]
    fn new_with_missing_env_var() {
        let result = ProviderClient::new(
            "http://localhost",
            "BATON_TEST_NONEXISTENT_PROVIDER_KEY_XYZ",
            "myp",
            30,
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::ApiKeyNotSet { provider, env_var } => {
                assert_eq!(provider, "myp");
                assert_eq!(env_var, "BATON_TEST_NONEXISTENT_PROVIDER_KEY_XYZ");
            }
            other => panic!("Expected ApiKeyNotSet, got: {other:?}"),
        }
    }

    #[test]
    fn new_with_valid_env_var() {
        std::env::set_var("BATON_TEST_PROVIDER_CLIENT_KEY", "secret-123");
        let client = ProviderClient::new(
            "http://localhost",
            "BATON_TEST_PROVIDER_CLIENT_KEY",
            "test",
            30,
        )
        .unwrap();
        std::env::remove_var("BATON_TEST_PROVIDER_CLIENT_KEY");
        assert_eq!(client.api_key, Some("secret-123".into()));
    }

    #[test]
    fn new_stores_timeout() {
        let client = ProviderClient::new("http://localhost", "", "p", 42).unwrap();
        assert_eq!(client.timeout_seconds, 42);
    }

    // ─── HTTP tests (httpmock) ──────────────────────────

    /// Creates a ProviderClient pointed at the mock server with no auth.
    fn test_client(server: &httpmock::MockServer) -> ProviderClient {
        ProviderClient::new(&server.url(""), "", "test", 10).unwrap()
    }

    /// Creates a ProviderClient with a pre-set API key (bypasses env lookup).
    fn test_client_with_key(server: &httpmock::MockServer, key: &str) -> ProviderClient {
        ProviderClient {
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap(),
            api_base: server.url(""),
            api_key: Some(key.into()),
            provider_name: "test".into(),
            api_key_env: "TEST_KEY".into(),
            timeout_seconds: 10,
        }
    }

    fn valid_completion_response() -> serde_json::Value {
        serde_json::json!({
            "choices": [{"message": {"content": "PASS — looks good"}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20}
        })
    }

    // ─── post_completion HTTP tests ─────────────────────

    #[test]
    fn post_completion_malformed_json() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).body("not json at all");
        });

        let client = test_client(&server);
        let body = serde_json::json!({"model": "m", "messages": []});
        let result = client.post_completion(body, "m");

        assert!(result.is_err());
        match result.unwrap_err() {
            ProviderError::MalformedResponse(_) => {}
            other => panic!("Expected MalformedResponse, got: {other:?}"),
        }
        mock.assert();
    }

    #[test]
    fn post_completion_body_forwarded() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .json_body_includes(r#"{"model": "gpt-4o"}"#)
                .json_body_includes(r#"{"temperature": 0.5}"#);
            then.status(200).json_body(valid_completion_response());
        });

        let client = test_client(&server);
        let body = serde_json::json!({
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "hello"}],
            "temperature": 0.5
        });
        let result = client.post_completion(body, "gpt-4o").unwrap();
        assert!(result.content.contains("PASS"));
        mock.assert();
    }

    #[test]
    fn post_completion_auth_header_sent() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .header("Authorization", "Bearer sk-test-key");
            then.status(200).json_body(valid_completion_response());
        });

        let client = test_client_with_key(&server, "sk-test-key");
        let body = serde_json::json!({"model": "m", "messages": []});
        let result = client.post_completion(body, "m").unwrap();
        assert!(!result.content.is_empty());
        mock.assert();
    }

    #[test]
    fn post_completion_no_auth_header_when_no_key() {
        let server = httpmock::MockServer::start();
        // Mock that accepts any request (no auth constraint)
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(valid_completion_response());
        });
        // Mock that would match if Authorization header IS present
        let auth_mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .header_exists("Authorization");
            then.status(200).json_body(valid_completion_response());
        });

        let client = test_client(&server);
        let body = serde_json::json!({"model": "m", "messages": []});
        client.post_completion(body, "m").unwrap();
        mock.assert();
        auth_mock.assert_calls(0);
    }

    #[test]
    fn post_completion_401_returns_auth_failed() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(401).body("unauthorized");
        });

        let client = test_client(&server);
        let body = serde_json::json!({"model": "m", "messages": []});
        match client.post_completion(body, "m").unwrap_err() {
            ProviderError::AuthFailed { .. } => {}
            other => panic!("Expected AuthFailed, got: {other:?}"),
        }
        mock.assert();
    }

    #[test]
    fn post_completion_429_returns_rate_limited() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(429).body("too many requests");
        });

        let client = test_client(&server);
        let body = serde_json::json!({"model": "m", "messages": []});
        match client.post_completion(body, "m").unwrap_err() {
            ProviderError::RateLimited { .. } => {}
            other => panic!("Expected RateLimited, got: {other:?}"),
        }
        mock.assert();
    }

    #[test]
    fn post_completion_404_returns_model_not_found() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(404).body("not found");
        });

        let client = test_client(&server);
        let body = serde_json::json!({"model": "gpt-5", "messages": []});
        match client.post_completion(body, "gpt-5").unwrap_err() {
            ProviderError::ModelNotFound { model, .. } => {
                assert_eq!(model, "gpt-5");
            }
            other => panic!("Expected ModelNotFound, got: {other:?}"),
        }
        mock.assert();
    }

    #[test]
    fn post_completion_500_returns_http_error() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(500).body("internal server error");
        });

        let client = test_client(&server);
        let body = serde_json::json!({"model": "m", "messages": []});
        match client.post_completion(body, "m").unwrap_err() {
            ProviderError::HttpError { status, body } => {
                assert_eq!(status, 500);
                assert!(body.contains("internal server error"));
            }
            other => panic!("Expected HttpError, got: {other:?}"),
        }
        mock.assert();
    }

    #[test]
    fn post_completion_empty_content_returns_empty_content_error() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": ""}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 0}
            }));
        });

        let client = test_client(&server);
        let body = serde_json::json!({"model": "m", "messages": []});
        match client.post_completion(body, "m").unwrap_err() {
            ProviderError::EmptyContent { cost } => {
                assert!(cost.is_some());
            }
            other => panic!("Expected EmptyContent, got: {other:?}"),
        }
        mock.assert();
    }

    // ─── list_models HTTP tests ─────────────────────────

    #[test]
    fn list_models_success() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).json_body(serde_json::json!({
                "data": [{"id": "gpt-4o"}, {"id": "gpt-3.5-turbo"}]
            }));
        });

        let client = test_client(&server);
        let models = client.list_models().unwrap();
        assert_eq!(models, vec!["gpt-4o", "gpt-3.5-turbo"]);
        mock.assert();
    }

    #[test]
    fn list_models_empty_data() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).json_body(serde_json::json!({"data": []}));
        });

        let client = test_client(&server);
        let models = client.list_models().unwrap();
        assert!(models.is_empty());
        mock.assert();
    }

    #[test]
    fn list_models_no_data_field() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200).json_body(serde_json::json!({}));
        });

        let client = test_client(&server);
        let models = client.list_models().unwrap();
        assert!(models.is_empty());
        mock.assert();
    }

    #[test]
    fn list_models_auth_failure() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(401).body("unauthorized");
        });

        let client = test_client(&server);
        match client.list_models().unwrap_err() {
            ProviderError::AuthFailed { .. } => {}
            other => panic!("Expected AuthFailed, got: {other:?}"),
        }
        mock.assert();
    }

    #[test]
    fn list_models_http_error() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(500).body("server error");
        });

        let client = test_client(&server);
        match client.list_models().unwrap_err() {
            ProviderError::HttpError { status, .. } => assert_eq!(status, 500),
            other => panic!("Expected HttpError, got: {other:?}"),
        }
        mock.assert();
    }

    // ─── test_completion HTTP tests ─────────────────────

    #[test]
    fn test_completion_success() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(valid_completion_response());
        });

        let client = test_client(&server);
        let result = client.test_completion("test-model").unwrap();
        assert!(result);
        mock.assert();
    }

    #[test]
    fn test_completion_verifies_body() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .json_body_includes(r#"{"max_tokens": 1}"#)
                .json_body_includes(r#"{"model": "gpt-4o"}"#);
            then.status(200).json_body(valid_completion_response());
        });

        let client = test_client(&server);
        client.test_completion("gpt-4o").unwrap();
        mock.assert();
    }

    #[test]
    fn test_completion_http_error() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(401).body("unauthorized");
        });

        let client = test_client(&server);
        match client.test_completion("m").unwrap_err() {
            ProviderError::AuthFailed { .. } => {}
            other => panic!("Expected AuthFailed, got: {other:?}"),
        }
        mock.assert();
    }

    // ─── send_request error paths ───────────────────────

    #[test]
    fn send_request_timeout() {
        let server = httpmock::MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200)
                .delay(std::time::Duration::from_secs(5))
                .json_body(valid_completion_response());
        });

        // Client with 1-second timeout
        let client = ProviderClient::new(&server.url(""), "", "test", 1).unwrap();

        let body = serde_json::json!({"model": "m", "messages": []});
        match client.post_completion(body, "m").unwrap_err() {
            ProviderError::Timeout {
                timeout_seconds, ..
            } => {
                assert_eq!(timeout_seconds, 1);
            }
            other => panic!("Expected Timeout, got: {other:?}"),
        }
    }

    #[test]
    fn send_request_unreachable() {
        // Point at a port with no server
        let client = ProviderClient::new("http://127.0.0.1:1", "", "test", 5).unwrap();

        let body = serde_json::json!({"model": "m", "messages": []});
        match client.post_completion(body, "m").unwrap_err() {
            ProviderError::Unreachable {
                provider, api_base, ..
            } => {
                assert_eq!(provider, "test");
                assert!(api_base.contains("127.0.0.1"));
            }
            other => panic!("Expected Unreachable, got: {other:?}"),
        }
    }
}
