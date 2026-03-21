# module: provider

> **v0.6 note:** `provider.rs` is now an internal utility used by the API adapter (`src/runtime/api.rs`), not called directly from `exec.rs` or `main.rs`. The runtime layer mediates all provider access.

HTTP client abstraction for OpenAI-compatible LLM provider APIs. Centralizes API key resolution, auth header construction, HTTP error classification, and response parsing. Used by the API runtime adapter (`runtime::api`) and `main` (provider health checks via `check-provider`).

## Public types

| Type | Purpose |
|---|---|
| `ProviderClient` | HTTP client wrapping reqwest with provider-specific auth, URL construction, and error classification |
| `ProviderError` | Structured error enum for all provider HTTP interaction failures |
| `CompletionResponse` | Parsed response from a chat completion call: content + optional cost |

## Public functions

| Function | Purpose |
|---|---|
| `ProviderClient::new` | Construct client from `Provider` config, resolving API key from env |
| `ProviderClient::post_completion` | Send chat completion request, parse response |
| `ProviderClient::list_models` | GET /v1/models, return model ID list |
| `ProviderClient::test_completion` | Minimal completion as connectivity test |
| `ProviderClient::provider_name` | Accessor for error messages |
| `ProviderClient::api_base` | Accessor for error messages |
| `ProviderClient::api_key_env` | Accessor for error messages |
| `extract_cost` | Extract token usage from OpenAI-compatible response JSON |

## Internal functions

| Function | Called by |
|---|---|
| `ProviderClient::apply_auth` | `post_completion`, `list_models`, `test_completion` |
| `ProviderClient::send_request` | `post_completion`, `list_models`, `test_completion` |
| `ProviderClient::classify_http_error` | `post_completion`, `list_models`, `test_completion` |

## Design notes

`ProviderClient` is a thin wrapper over `reqwest::blocking::Client` that owns the provider's auth credentials and base URL. It is not a trait — there is currently no need for mock implementations because the client talks to a real HTTP endpoint (which can be a local mock server in tests). If a non-HTTP provider backend is added in the future, a trait can be extracted at that point.

The `ProviderError` enum is designed so that callers can either use `Display` for a formatted error message or `match` on variants for custom formatting. The `Display` implementation uses `[baton]`-style messages suitable for validator feedback. The CLI (`main.rs`) matches on variants for its own output format.

`extract_cost` is a standalone public function (not a method) because it parses a standard OpenAI response structure that may also be useful outside the client context.

The timeout is stored on the client so that `ProviderError::Timeout` can report the configured value. This avoids the caller needing to re-supply it for error messages.

---

## ProviderError

Structured error type. Each variant carries the context needed for user-facing messages.

SPEC-PV-PE-001: display-api-key-not-set
  Display output includes the provider name and the env var name.
  test: provider::tests::error_display_api_key_not_set

SPEC-PV-PE-002: display-auth-failed
  Display output includes "Authentication failed", the provider name, and the api_key_env name.
  test: provider::tests::error_display_auth_failed

SPEC-PV-PE-003: display-model-not-found
  Display output includes the model name and "not found".
  test: provider::tests::error_display_model_not_found

SPEC-PV-PE-004: display-timeout
  Display output includes "timed out" and the timeout duration in seconds.
  test: provider::tests::error_display_timeout

SPEC-PV-PE-005: display-unreachable
  Display output includes "Cannot reach", the api_base URL, and the error detail.
  test: provider::tests::error_display_unreachable

SPEC-PV-PE-006: display-rate-limited
  Display output includes "Rate limited" and the provider name.
  test: provider::tests::error_display_rate_limited

SPEC-PV-PE-007: display-http-error
  Display output includes the HTTP status code and response body text.
  test: provider::tests::error_display_http_error

SPEC-PV-PE-008: display-empty-content
  Display output includes "empty or malformed".
  test: provider::tests::error_display_empty_content

SPEC-PV-PE-009: display-malformed-response
  Display output includes "empty or malformed response" and the parse error detail.
  test: UNTESTED

SPEC-PV-PE-010: display-client-build-failed
  Display output includes "Failed to create HTTP client" and the error detail.
  test: UNTESTED

SPEC-PV-PE-011: implements-std-error
  `ProviderError` implements `std::error::Error` for compatibility with `?` and error chains.
  test: IMPLICIT (used via Display in exec.rs and main.rs)

---

## ProviderClient::new

Constructs an HTTP client from a `Provider` config struct. Resolves the API key and builds the reqwest client with the specified timeout.

SPEC-PV-CL-001: empty-api-key-env-means-no-auth
  When `provider.api_key_env` is empty, `api_key` is `None`. No Authorization header will be sent.
  test: provider::tests::new_with_empty_api_key_env

SPEC-PV-CL-002: missing-env-var-returns-api-key-not-set
  When `provider.api_key_env` is non-empty but the environment variable is not set, returns `Err(ProviderError::ApiKeyNotSet)` with the provider name and env var name.
  test: provider::tests::new_with_missing_env_var

SPEC-PV-CL-003: valid-env-var-resolved
  When the environment variable is set, its value is stored as the API key.
  test: provider::tests::new_with_valid_env_var

SPEC-PV-CL-004: timeout-stored
  The `timeout_seconds` parameter is stored on the client for use in `ProviderError::Timeout` reporting and passed to the reqwest client builder.
  test: provider::tests::new_stores_timeout

SPEC-PV-CL-005: client-build-failure-returns-error
  If `reqwest::blocking::Client::builder().build()` fails, returns `Err(ProviderError::ClientBuildFailed)`.
  test: UNTESTED (reqwest builder rarely fails)

---

## ProviderClient::apply_auth

Private helper that conditionally adds the `Authorization: Bearer {key}` header.

SPEC-PV-AA-001: adds-bearer-when-key-present
  When `self.api_key` is `Some(key)`, the returned request builder has an Authorization header with value `"Bearer {key}"`.
  test: IMPLICIT via post_completion/list_models tests that verify auth behavior

SPEC-PV-AA-002: passthrough-when-no-key
  When `self.api_key` is `None`, the request builder is returned unchanged.
  test: IMPLICIT via new_with_empty_api_key_env (no auth set, requests succeed against mock)

---

## ProviderClient::send_request

Private helper that sends a request and maps transport errors.

SPEC-PV-SR-001: timeout-mapped-to-timeout-error
  If the request fails with `e.is_timeout() == true`, returns `ProviderError::Timeout` with the client's stored `timeout_seconds`.
  test: UNTESTED (requires delayed mock server response)

SPEC-PV-SR-002: connection-error-mapped-to-unreachable
  If the request fails with a non-timeout error (connection refused, DNS failure), returns `ProviderError::Unreachable` with the api_base and error detail.
  test: UNTESTED (tested indirectly via exec::tests::llm_completion_unreachable_provider)

---

## ProviderClient::classify_http_error

Private helper that maps HTTP status codes to structured errors.

SPEC-PV-CH-001: 401-403-maps-to-auth-failed
  HTTP 401 or 403 returns `ProviderError::AuthFailed` with provider name and api_key_env.
  test: IMPLICIT via exec::tests::llm_completion_http_401

SPEC-PV-CH-002: 404-maps-to-model-not-found
  HTTP 404 returns `ProviderError::ModelNotFound` with the model name and provider name.
  test: IMPLICIT via exec::tests::llm_completion_http_404

SPEC-PV-CH-003: 429-maps-to-rate-limited
  HTTP 429 returns `ProviderError::RateLimited` with the provider name.
  test: IMPLICIT via exec::tests::llm_completion_http_429

SPEC-PV-CH-004: other-status-maps-to-http-error
  Any other non-success status returns `ProviderError::HttpError` with the status code and response body text.
  test: IMPLICIT via exec::tests::llm_completion_http_500, llm_completion_http_503

---

## ProviderClient::post_completion

Sends a POST to `/v1/chat/completions` with the given request body.

SPEC-PV-PC-001: posts-to-completions-endpoint
  The request is sent to `{api_base}/v1/chat/completions` as a JSON POST.
  test: IMPLICIT via exec::tests::llm_completion_pass_verdict (verifies request path)

SPEC-PV-PC-002: auth-header-applied
  The Authorization header is applied via `apply_auth` before sending.
  test: IMPLICIT via exec tests with api_key_env

SPEC-PV-PC-003: transport-errors-propagated
  Connection and timeout errors from `send_request` are returned directly.
  test: IMPLICIT via exec::tests::llm_completion_unreachable_provider

SPEC-PV-PC-004: http-errors-classified
  Non-success status codes are classified via `classify_http_error` after reading the response body.
  test: IMPLICIT via exec::tests::llm_completion_http_401, _404, _429, _500

SPEC-PV-PC-005: json-parse-failure-returns-malformed
  If the 2xx response body cannot be parsed as JSON, returns `ProviderError::MalformedResponse`.
  test: UNTESTED (requires mock returning non-JSON with 200)

SPEC-PV-PC-006: content-extracted-from-choices
  Content is extracted from `choices[0].message.content`. If any part of the path is missing, content is treated as empty.
  test: IMPLICIT via exec::tests::llm_completion_pass_verdict

SPEC-PV-PC-007: empty-content-returns-error-with-cost
  If content is empty (missing or empty string), returns `ProviderError::EmptyContent` with the cost still extracted from the `usage` field.
  test: IMPLICIT via exec::tests::llm_completion_empty_response, llm_completion_missing_choices_key

SPEC-PV-PC-008: cost-extracted-from-usage
  Token usage is extracted via `extract_cost()` from the response body. If no `usage` field, cost is `None`.
  test: IMPLICIT via exec::tests::llm_completion_cost_tracking, llm_completion_no_usage_in_response

---

## ProviderClient::list_models

Sends a GET to `/v1/models` and parses the response.

SPEC-PV-LM-001: gets-models-endpoint
  The request is sent to `{api_base}/v1/models` as a GET.
  test: UNTESTED (requires HTTP mock)

SPEC-PV-LM-002: auth-header-applied
  The Authorization header is applied via `apply_auth`.
  test: UNTESTED

SPEC-PV-LM-003: parses-model-ids
  Model IDs are extracted from `data[].id` in the response JSON. Non-string IDs are skipped.
  test: UNTESTED

SPEC-PV-LM-004: empty-or-missing-data-returns-empty-vec
  If the response has no `data` field, or `data` is empty, returns `Ok(vec![])`.
  test: UNTESTED

SPEC-PV-LM-005: http-errors-classified
  Non-success status codes are classified via `classify_http_error`.
  test: UNTESTED

---

## ProviderClient::test_completion

Sends a minimal completion request to verify model availability.

SPEC-PV-TC-001: posts-with-max-tokens-1
  The request body includes `max_tokens: 1` and a single "ping" message to minimize cost.
  test: UNTESTED (requires HTTP mock)

SPEC-PV-TC-002: success-returns-true
  A 2xx response returns `Ok(true)`.
  test: UNTESTED

SPEC-PV-TC-003: http-errors-classified
  Non-success status codes are classified via `classify_http_error`.
  test: UNTESTED

---

## extract_cost

Standalone function that extracts token usage from an OpenAI-compatible response body.

SPEC-PV-EC-001: full-usage-extracted
  When `usage.prompt_tokens` and `usage.completion_tokens` are both present, returns `Some(Cost)` with both token counts and the provided model name. `estimated_usd` is always `None`.
  test: provider::tests::extract_cost_full_usage

SPEC-PV-EC-002: no-usage-returns-none
  When the `usage` field is absent from the response body, returns `None`.
  test: provider::tests::extract_cost_no_usage

SPEC-PV-EC-003: empty-usage-returns-none
  When the `usage` field exists but has no token count fields, returns `None`.
  test: provider::tests::extract_cost_empty_usage

SPEC-PV-EC-004: partial-usage-extracted
  When only one of prompt_tokens/completion_tokens is present, the missing field is `None` in the returned Cost. The cost is still `Some(...)`.
  test: provider::tests::extract_cost_partial_usage
