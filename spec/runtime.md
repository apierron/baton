# module: runtime

Runtime adapter abstraction for agent-based validators and API backends. Defines the `RuntimeAdapter` trait, session lifecycle types, and one-shot completion types. Supports OpenHands, OpenCode, and API as runtime backends.

## Public types

| Type | Purpose |
|---|---|
| `SessionConfig` | Config for creating agent session (task, files, model, sandbox, max_iterations, timeout_seconds, env) |
| `SessionHandle` | Opaque handle to a running session (id, workspace_id) |
| `SessionStatus` | Lifecycle enum: Running, Completed, Failed, TimedOut, Cancelled |
| `SessionResult` | Collected output: status, output, raw_log, cost |
| `HealthResult` | Health probe result: reachable, version, models, message |
| `CompletionRequest` | Request for one-shot completion: messages, model, temperature, max_tokens |
| `CompletionResult` | Result of one-shot completion: content, cost |
| `RuntimeAdapter` | Trait: health_check, create_session, poll_status, collect_result, cancel, teardown, post_completion |

## Public functions

| Function | Purpose |
|---|---|
| `create_adapter` | Factory: runtime config to `Box<dyn RuntimeAdapter>` |
| `post_completion` | Trait default: returns error for runtimes that don't support one-shot completions |

## Internal functions

| Function | Called by |
|---|---|
| `OpenHandsAdapter::new` | `create_adapter` |
| `OpenHandsAdapter::auth_headers` | All trait methods (OpenHands) |
| `map_openhands_status` | `poll_status`, `collect_result` (OpenHands) |
| `extract_cost_from_openhands` | `collect_result` (OpenHands) |
| `OpenCodeAdapter::new` | `create_adapter` |
| `OpenCodeAdapter::auth_headers` | All trait methods (OpenCode) |
| `map_opencode_status` | `poll_status`, `collect_result` (OpenCode) |
| `extract_cost_from_opencode` | `collect_result` (OpenCode) |
| `ApiAdapter::new` | `create_adapter` |

## Design notes

The `RuntimeAdapter` trait is object-safe (`Send + Sync + Debug`) so adapters can be stored as `Box<dyn RuntimeAdapter>`. This allows the exec module to work with any runtime backend without knowing the concrete type. The trait methods use `&self` (not `&mut self`) because all state lives server-side; the adapter is just an HTTP client.

`cancel` and `teardown` are intentionally idempotent (always return `Ok(())`) because they are called during cleanup paths where the session or workspace may already be gone. Propagating errors from cleanup would mask the real failure.

The `SessionStatus` enum is baton's canonical representation. Each runtime backend maps its own status vocabulary to these five states via a helper function, keeping backend-specific strings out of the core types.

---

## Public type construction

These are plain data types with public fields. They have no invariants beyond Rust's type system, so the spec assertions here simply document their shape and derivations.

SPEC-RT-TY-001: session-config-fields
  `SessionConfig` has fields: task (String), files (BTreeMap<String, String>), model (String), sandbox (bool), max_iterations (u32), timeout_seconds (u64), env (BTreeMap<String, String>). It derives Debug and Clone.
  test: runtime::tests::session_config_construction

SPEC-RT-TY-002: session-handle-fields
  `SessionHandle` has fields: id (String), workspace_id (String). It derives Debug and Clone.
  test: runtime::tests::session_handle_construction

SPEC-RT-TY-003: session-status-variants
  `SessionStatus` has exactly five variants: Running, Completed, Failed, TimedOut, Cancelled. It derives Debug, Clone, PartialEq, Eq.
  test: IMPLICIT via map_status tests

SPEC-RT-TY-004: session-result-fields
  `SessionResult` has fields: status (SessionStatus), output (String), raw_log (String), cost (Option<Cost>). It derives Debug and Clone.
  test: runtime::tests::session_result_construction

SPEC-RT-TY-005: health-result-fields
  `HealthResult` has fields: reachable (bool), version (Option<String>), models (Option<Vec<String>>), message (Option<String>). It derives Debug and Clone.
  test: runtime::tests::health_result_construction

SPEC-RT-TY-010: completion-request-fields
  `CompletionRequest` has fields: messages (Vec<serde_json::Value>), model (String), temperature (f64), max_tokens (Option<u32>). It derives Debug and Clone.
  test: UNTESTED

SPEC-RT-TY-011: completion-result-fields
  `CompletionResult` has fields: content (String), cost (Option<Cost>). It derives Debug and Clone.
  test: UNTESTED

---

## post_completion (trait default)

Default method on `RuntimeAdapter` that returns an error indicating the runtime doesn't support one-shot completions. Overridden by API, OpenHands, and OpenCode adapters.

SPEC-RT-PC-001: default-returns-runtime-error
  The default implementation returns `Err(BatonError::RuntimeError("This runtime does not support one-shot completions."))`.
  test: UNTESTED

---

## create_adapter

Factory function that maps a runtime config to a concrete `RuntimeAdapter` implementation. Takes both the runtime name (for error messages) and the `Runtime` config struct.

SPEC-RT-CA-001: openhands-type-creates-adapter
  When `runtime_config.runtime_type` is `"openhands"`, `create_adapter` constructs an `OpenHandsAdapter` via `OpenHandsAdapter::new`, passing through base_url, api_key_env, default_model, sandbox, timeout_seconds, and max_iterations. Returns `Ok(Box<dyn RuntimeAdapter>)`.
  test: runtime::tests::create_openhands_adapter_no_auth

SPEC-RT-CA-002: unknown-type-returns-config-error
  When `runtime_config.runtime_type` is anything other than `"openhands"` or `"opencode"`, `create_adapter` returns `Err(ConfigError)` with a message containing "Unknown runtime type" and the offending type string.
  test: runtime::tests::create_adapter_unknown_type

SPEC-RT-CA-003: openhands-new-error-propagated
  If `OpenHandsAdapter::new` returns an error (e.g., missing API key env var), `create_adapter` propagates that error unchanged.
  test: UNTESTED

SPEC-RT-CA-004: opencode-type-creates-adapter
  When `runtime_config.runtime_type` is `"opencode"`, `create_adapter` constructs an `OpenCodeAdapter` via `OpenCodeAdapter::new`, passing through base_url, api_key_env, default_model, sandbox, timeout_seconds, and max_iterations. Returns `Ok(Box<dyn RuntimeAdapter>)`.
  test: runtime::tests::create_opencode_adapter_no_auth

SPEC-RT-CA-005: opencode-new-error-propagated
  If `OpenCodeAdapter::new` returns an error (e.g., missing API key env var), `create_adapter` propagates that error unchanged.
  test: UNTESTED

SPEC-RT-CA-006: api-type-creates-api-adapter
  When `runtime_config.runtime_type` is `"api"`, `create_adapter` constructs an `ApiAdapter`. Returns `Ok(Box<dyn RuntimeAdapter>)`.
  test: UNTESTED

---

## OpenHandsAdapter::new

Constructs the HTTP client adapter. Resolves the API key from the environment, normalizes the base URL, and builds the reqwest client.

The client timeout is set to `timeout_seconds + 30` to give a buffer beyond the server-side session timeout. This prevents the HTTP client from timing out before the server does, which would mask the server's timeout response with a less informative connection error.

SPEC-RT-OH-001: api-key-env-resolved-from-environment
  When `api_key_env` is `Some(name)` and `name` is non-empty, the constructor reads the environment variable named by `name`. If the variable is set, its value is stored as the API key. If the variable is not set, returns `Err(ConfigError)` with a message containing the variable name and "is not set".
  test: UNTESTED (env var set case requires setting env var in test)
  test: UNTESTED (env var unset case)

SPEC-RT-OH-002: api-key-env-none-means-no-auth
  When `api_key_env` is `None`, no API key is resolved. The adapter operates without authentication. Auth headers will be empty.
  test: runtime::tests::create_openhands_adapter_no_auth

SPEC-RT-OH-003: api-key-env-empty-string-means-no-auth
  When `api_key_env` is `Some("")` (empty string), it is treated the same as `None`. No API key is resolved. This handles the case where a config file has `api_key_env = ""`.
  test: runtime::tests::create_openhands_adapter_empty_auth

SPEC-RT-OH-004: trailing-slash-stripped-from-base-url
  If `base_url` ends with `/`, the trailing slash is removed before storing. This prevents double-slashes in constructed URLs (e.g., `http://host//api/health`).
  test: IMPLICIT via create_openhands_adapter_empty_auth (passes url with trailing slash)

SPEC-RT-OH-005: client-timeout-is-session-timeout-plus-buffer
  The reqwest client is built with a timeout of `timeout_seconds + 30` seconds. This ensures the HTTP client outlives the server-side session timeout.
  test: UNTESTED

SPEC-RT-OH-006: client-build-failure-returns-validation-error
  If the reqwest client builder fails (e.g., invalid TLS config), returns `Err(ValidationError)` with "Failed to create HTTP client".
  test: UNTESTED

---

## OpenHandsAdapter::auth_headers

Private helper that constructs HTTP headers for authentication.

SPEC-RT-AH-001: bearer-token-added-when-api-key-present
  When `self.api_key` is `Some(key)`, the returned `HeaderMap` contains an `Authorization` header with value `"Bearer {key}"`. If the key contains characters that are invalid for a header value, the header is silently omitted.
  test: UNTESTED

SPEC-RT-AH-002: empty-headers-when-no-api-key
  When `self.api_key` is `None`, the returned `HeaderMap` is empty.
  test: UNTESTED

---

## health_check

Probes the runtime server's health endpoint. Used for pre-flight validation before attempting to create sessions.

The method distinguishes between three outcomes: success (server is up and responding), HTTP error (server is reachable but returned a non-2xx status), and connection error (server is unreachable). The first two return `Ok(HealthResult)` with different `reachable` values; the third returns `Err`.

SPEC-RT-HC-001: success-returns-reachable-with-version
  On a successful (2xx) response from `GET /api/health`, returns `Ok(HealthResult)` with `reachable=true`, `version` extracted from the JSON body's `"version"` field (if present), `models=None`, and `message=None`.
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-HC-002: http-error-returns-not-reachable
  On a non-2xx HTTP response, returns `Ok(HealthResult)` with `reachable=false`, `version=None`, `models=None`, and `message=Some("HTTP {status}")`. Note: this is `Ok`, not `Err`, because the server was reachable at the network level.
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-HC-003: connection-error-returns-validation-error
  If the HTTP request fails at the network level (connection refused, DNS failure, timeout), returns `Err(ValidationError)` with a message containing "Cannot reach runtime at" and the base URL.
  test: UNTESTED (requires network failure simulation)

SPEC-RT-HC-004: malformed-json-treated-as-empty
  If the 2xx response body is not valid JSON, `response.json()` falls back to `serde_json::Value::default()` (null), and `version` will be `None`. No error is raised.
  test: UNTESTED

---

## create_session

Creates a new agent session: generates a workspace, uploads files, and starts the session. This is the most complex trait method with multiple failure points.

The method uses a two-phase approach: first upload all files to a workspace, then create the session referencing that workspace. If any file upload fails, the session is never created and the workspace is orphaned (caller should teardown on error).

SPEC-RT-CS-001: workspace-id-is-uuid
  A new UUID v4 is generated for each session's workspace. This ensures workspace isolation even when sessions run concurrently.
  test: UNTESTED

SPEC-RT-CS-002: files-uploaded-via-multipart
  For each entry in `config.files`, the file at the path (value) is read from disk and uploaded via `POST /api/workspaces/{workspace_id}/files` as a multipart form with the file name set to the map key.
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-CS-003: file-read-error-returns-validation-error
  If reading a file from disk fails (e.g., file not found, permission denied), returns `Err(ValidationError)` with a message containing the file name and path.
  test: UNTESTED

SPEC-RT-CS-004: file-upload-http-error-returns-validation-error
  If the file upload HTTP request returns a non-2xx status, returns `Err(ValidationError)` with a message containing the file name and the HTTP status code.
  test: UNTESTED

SPEC-RT-CS-005: file-upload-connection-error-returns-validation-error
  If the file upload HTTP request fails at the network level, returns `Err(ValidationError)` with a message containing "Failed to upload file" and the file name.
  test: UNTESTED

SPEC-RT-CS-006: session-creation-posts-json-body
  After files are uploaded, `POST /api/sessions` is called with a JSON body containing: workspace_id, task, model, sandbox, max_iterations, and timeout (mapped from timeout_seconds).
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-CS-007: session-creation-http-error-includes-body
  If the session creation POST returns a non-2xx status, returns `Err(ValidationError)` with a message containing both the HTTP status code and the response body text. Including the body helps diagnose server-side validation errors.
  test: UNTESTED

SPEC-RT-CS-008: session-creation-connection-error
  If the session creation POST fails at the network level, returns `Err(ValidationError)` with "Failed to create session on runtime".
  test: UNTESTED

SPEC-RT-CS-009: missing-session-id-returns-validation-error
  If the session creation response is valid JSON but does not contain a `"session_id"` string field, returns `Err(ValidationError)` with "Session creation response missing 'session_id'".
  test: UNTESTED

SPEC-RT-CS-010: successful-creation-returns-handle
  On success, returns `Ok(SessionHandle)` with `id` from the response's `"session_id"` field and `workspace_id` from the generated UUID.
  test: UNTESTED

SPEC-RT-CS-011: unparseable-json-response-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)` with "Failed to parse session creation response".
  test: UNTESTED

---

## poll_status

Polls the current lifecycle state of a running session.

SPEC-RT-PS-001: polls-via-get-request
  Sends `GET /api/sessions/{session_id}/status` with auth headers. Parses the response body's `"status"` field and maps it via `map_openhands_status`.
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-PS-002: http-error-returns-validation-error
  If the response is non-2xx, returns `Err(ValidationError)` with "Failed to poll session status: HTTP {status}".
  test: UNTESTED

SPEC-RT-PS-003: connection-error-returns-validation-error
  If the HTTP request fails at the network level, returns `Err(ValidationError)` with "Failed to poll session status".
  test: UNTESTED

SPEC-RT-PS-004: missing-status-field-defaults-to-unknown
  If the response JSON has no `"status"` field, the status string defaults to `"unknown"`, which `map_openhands_status` maps to `SessionStatus::Failed`.
  test: UNTESTED

SPEC-RT-PS-005: unparseable-json-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)` with "Failed to parse status response".
  test: UNTESTED

---

## collect_result

Collects the final output from a completed session. Extracts the agent's response, full execution log, and cost metrics.

SPEC-RT-CR-001: collects-via-get-request
  Sends `GET /api/sessions/{session_id}/result` with auth headers.
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-CR-002: extracts-final-message-as-output
  The `output` field is populated from the response body's `"final_message"` string. If absent, defaults to empty string.
  test: UNTESTED

SPEC-RT-CR-003: extracts-full-log-as-raw-log
  The `raw_log` field is populated from the response body's `"full_log"` string. If absent, defaults to empty string.
  test: UNTESTED

SPEC-RT-CR-004: status-mapped-via-map-openhands-status
  The `status` field is mapped from the response body's `"status"` string via `map_openhands_status`. If the field is absent, defaults to `"failed"`, which maps to `SessionStatus::Failed`. This default differs from `poll_status` (which defaults to `"unknown"`), but both map to `Failed`.
  test: UNTESTED

SPEC-RT-CR-005: cost-extracted-via-extract-cost-from-openhands
  The `cost` field is populated by passing the entire response body to `extract_cost_from_openhands`.
  test: IMPLICIT via extract_cost tests

SPEC-RT-CR-006: http-error-returns-validation-error
  If the response is non-2xx, returns `Err(ValidationError)` with "Failed to collect session result: HTTP {status}".
  test: UNTESTED

SPEC-RT-CR-007: connection-error-returns-validation-error
  If the HTTP request fails at the network level, returns `Err(ValidationError)` with "Failed to collect session result".
  test: UNTESTED

SPEC-RT-CR-008: unparseable-json-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)` with "Failed to parse result response".
  test: UNTESTED

---

## cancel

Cancels a running session. Designed to be idempotent for safe use in cleanup paths.

SPEC-RT-CN-001: sends-delete-to-session-endpoint
  Sends `DELETE /api/sessions/{session_id}` with auth headers.
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-CN-002: always-returns-ok
  Regardless of whether the DELETE request succeeds, fails, or errors at the network level, `cancel` always returns `Ok(())`. The response is discarded. This is intentional: cancel is called during error cleanup where the session may already be gone.
  test: UNTESTED

---

## teardown

Cleans up workspace resources. Designed to be idempotent for safe use in cleanup paths.

SPEC-RT-TD-001: sends-delete-to-workspace-endpoint
  Sends `DELETE /api/workspaces/{workspace_id}` with auth headers. Note: uses `handle.workspace_id`, not `handle.id`.
  test: UNTESTED (requires HTTP mock server)

SPEC-RT-TD-002: always-returns-ok
  Regardless of whether the DELETE request succeeds, fails, or errors at the network level, `teardown` always returns `Ok(())`. The response is discarded. Same rationale as `cancel`.
  test: UNTESTED

---

## map_openhands_status

Maps OpenHands-specific status strings to baton's canonical `SessionStatus` enum. Case-insensitive.

The function accepts multiple synonyms for each state because OpenHands has used different status strings across versions, and we want forward compatibility with reasonable variations.

SPEC-RT-MS-001: running-variants
  The strings `"running"`, `"pending"`, and `"started"` (case-insensitive) map to `SessionStatus::Running`.
  test: runtime::openhands::tests::map_status_running

SPEC-RT-MS-002: completed-variants
  The strings `"completed"`, `"finished"`, and `"done"` (case-insensitive) map to `SessionStatus::Completed`.
  test: runtime::openhands::tests::map_status_completed

SPEC-RT-MS-003: failed-variants
  The strings `"failed"` and `"error"` (case-insensitive) map to `SessionStatus::Failed`.
  test: runtime::openhands::tests::map_status_failed

SPEC-RT-MS-004: timed-out-variants
  The strings `"timed_out"` and `"timeout"` (case-insensitive) map to `SessionStatus::TimedOut`.
  test: runtime::openhands::tests::map_status_timed_out

SPEC-RT-MS-005: cancelled-variants
  The strings `"cancelled"`, `"canceled"`, and `"stopped"` (case-insensitive) map to `SessionStatus::Cancelled`. Both British and American spellings are accepted.
  test: runtime::openhands::tests::map_status_cancelled

SPEC-RT-MS-006: unknown-defaults-to-failed
  Any string not matching a known variant maps to `SessionStatus::Failed`. This is a conservative default: an unknown status is treated as a failure rather than silently succeeding.
  test: runtime::openhands::tests::map_status_unknown_defaults_to_failed

SPEC-RT-MS-007: case-insensitive-matching
  Status matching uses `.to_lowercase()` before comparison. `"RUNNING"`, `"Running"`, and `"running"` all map to `SessionStatus::Running`.
  test: runtime::openhands::tests::map_status_case_insensitive

---

## extract_cost_from_openhands

Extracts cost metadata from the OpenHands result response body. Returns `None` if no meaningful cost data is present.

The function uses a two-tier absence check: first, is the `"metrics"` key present at all? Second, are there any token counts? A metrics object with only a model name but no token counts is treated as "no cost data" because cost without token counts is not actionable.

SPEC-RT-EC-001: full-metrics-extracted
  When the response body contains a `"metrics"` object with `"input_tokens"` (i64), `"output_tokens"` (i64), `"model"` (string), and `"cost"` (f64), all four are extracted into the returned `Cost` struct.
  test: runtime::openhands::tests::extract_cost_with_metrics

SPEC-RT-EC-002: no-metrics-key-returns-none
  When the response body has no `"metrics"` key, returns `None`.
  test: runtime::openhands::tests::extract_cost_no_metrics

SPEC-RT-EC-003: empty-metrics-returns-none
  When the `"metrics"` object exists but contains neither `"input_tokens"` nor `"output_tokens"`, returns `None`. A metrics object with only `"model"` or `"cost"` but no token counts is not considered meaningful cost data.
  test: runtime::openhands::tests::extract_cost_empty_metrics

SPEC-RT-EC-004: partial-metrics-returns-some
  When the `"metrics"` object contains at least one of `"input_tokens"` or `"output_tokens"`, returns `Some(Cost)` with the present fields populated and absent fields as `None`.
  test: runtime::openhands::tests::extract_cost_partial_metrics

SPEC-RT-EC-005: cost-field-mapped-to-estimated-usd
  The OpenHands `"cost"` field (f64) in metrics maps to `Cost::estimated_usd`. This naming difference reflects that OpenHands reports actual cost while baton treats it as an estimate.
  test: IMPLICIT via extract_cost_with_metrics

---

## OpenHandsAdapter::post_completion

SPEC-RT-OH-PC-001: posts-to-chat-completions
  Sends `POST {base_url}/v1/chat/completions` with the same OpenAI-compatible format. Parses content and cost from usage.
  test: UNTESTED

---

## OpenCodeAdapter::new

Constructs the HTTP client adapter for the OpenCode runtime. Follows the same pattern as `OpenHandsAdapter::new`: resolves the API key from the environment, normalizes the base URL, and builds the reqwest client with a timeout buffer.

SPEC-RT-OC-001: api-key-env-resolved-from-environment
  When `api_key_env` is `Some(name)` and `name` is non-empty, the constructor reads the environment variable named by `name`. If the variable is set, its value is stored as the API key. If the variable is not set, returns `Err(ConfigError)` with a message containing the variable name and "is not set".
  test: runtime::opencode::tests::new_valid_env_var_is_resolved
  test: runtime::opencode::tests::new_missing_env_var_returns_config_error

SPEC-RT-OC-002: api-key-env-none-means-no-auth
  When `api_key_env` is `None`, no API key is resolved. The adapter operates without authentication. Auth headers will be empty.
  test: runtime::tests::create_opencode_adapter_no_auth

SPEC-RT-OC-003: api-key-env-empty-string-means-no-auth
  When `api_key_env` is `Some("")` (empty string), it is treated the same as `None`. No API key is resolved.
  test: runtime::opencode::tests::new_empty_env_var_name_treated_as_none

SPEC-RT-OC-004: trailing-slash-stripped-from-base-url
  If `base_url` ends with `/`, the trailing slash is removed before storing. This prevents double-slashes in constructed URLs.
  test: runtime::opencode::tests::new_strips_trailing_slash

SPEC-RT-OC-005: client-timeout-is-session-timeout-plus-buffer
  The reqwest client is built with a timeout of `timeout_seconds + 30` seconds. This ensures the HTTP client outlives the server-side session timeout.
  test: UNTESTED

SPEC-RT-OC-006: client-build-failure-returns-validation-error
  If the reqwest client builder fails (e.g., invalid TLS config), returns `Err(ValidationError)` with "Failed to create HTTP client".
  test: UNTESTED

---

## OpenCodeAdapter::auth_headers

Private helper that constructs HTTP headers for authentication. Same behavior as `OpenHandsAdapter::auth_headers`.

SPEC-RT-OC-AH-001: bearer-token-added-when-api-key-present
  When `self.api_key` is `Some(key)`, the returned `HeaderMap` contains an `Authorization` header with value `"Bearer {key}"`.
  test: runtime::opencode::tests::adapter_with_api_key_has_auth_header

SPEC-RT-OC-AH-002: empty-headers-when-no-api-key
  When `self.api_key` is `None`, the returned `HeaderMap` is empty.
  test: runtime::opencode::tests::adapter_without_api_key_has_no_auth_in_debug

---

## OpenCode health_check

Probes the OpenCode runtime server's health endpoint. Same endpoint pattern and behavior as OpenHands.

SPEC-RT-OC-HC-001: success-returns-reachable-with-version
  On a successful (2xx) response from `GET /api/health`, returns `Ok(HealthResult)` with `reachable=true` and `version` extracted from JSON body.
  test: runtime::opencode::tests::http_health_check_success

SPEC-RT-OC-HC-002: http-error-returns-not-reachable
  On a non-2xx HTTP response, returns `Ok(HealthResult)` with `reachable=false` and `message=Some("HTTP {status}")`.
  test: runtime::opencode::tests::http_health_check_http_error

SPEC-RT-OC-HC-003: connection-error-returns-validation-error
  If the HTTP request fails at the network level, returns `Err(ValidationError)` with "Cannot reach runtime at" and the base URL.
  test: runtime::opencode::tests::http_health_check_connection_refused

SPEC-RT-OC-HC-004: malformed-json-treated-as-empty
  If the 2xx response body is not valid JSON, `version` will be `None`. No error is raised.
  test: runtime::opencode::tests::http_health_check_malformed_json

---

## OpenCode create_session

Creates a new agent session on the OpenCode runtime. Same two-phase approach as OpenHands: upload files, then create session.

SPEC-RT-OC-CS-001: workspace-id-is-uuid
  A new UUID v4 is generated for each session's workspace.
  test: IMPLICIT via http_create_session_success_no_files (workspace_id is generated)

SPEC-RT-OC-CS-002: files-uploaded-via-multipart
  For each entry in `config.files`, the file is uploaded via `POST /api/workspaces/{workspace_id}/files` as a multipart form.
  test: runtime::opencode::tests::http_create_session_success_with_files

SPEC-RT-OC-CS-003: file-upload-http-error-returns-validation-error
  If a file upload returns a non-2xx status, returns `Err(ValidationError)` with the file name and HTTP status.
  test: runtime::opencode::tests::http_create_session_file_upload_http_error

SPEC-RT-OC-CS-004: session-creation-posts-json-body
  After files are uploaded, `POST /api/sessions` is called with workspace_id, task, model, sandbox, max_iterations, and timeout.
  test: runtime::opencode::tests::http_create_session_body_contents

SPEC-RT-OC-CS-005: session-creation-http-error-includes-body
  If session creation POST returns non-2xx, returns `Err(ValidationError)` with HTTP status and response body.
  test: runtime::opencode::tests::http_create_session_http_error

SPEC-RT-OC-CS-006: missing-session-id-returns-validation-error
  If the response is valid JSON but missing `"session_id"`, returns `Err(ValidationError)`.
  test: runtime::opencode::tests::http_create_session_missing_session_id

SPEC-RT-OC-CS-007: successful-creation-returns-handle
  On success, returns `Ok(SessionHandle)` with `id` from `"session_id"` and `workspace_id` from the generated UUID.
  test: runtime::opencode::tests::http_create_session_success_no_files

SPEC-RT-OC-CS-008: unparseable-json-response-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)`.
  test: runtime::opencode::tests::http_create_session_unparseable_json

---

## OpenCode poll_status

Polls the current lifecycle state of a running OpenCode session.

SPEC-RT-OC-PS-001: polls-via-get-request
  Sends `GET /api/sessions/{session_id}/status` with auth headers. Maps status via `map_opencode_status`.
  test: runtime::opencode::tests::http_poll_status_running

SPEC-RT-OC-PS-002: http-error-returns-validation-error
  If the response is non-2xx, returns `Err(ValidationError)`.
  test: runtime::opencode::tests::http_poll_status_http_error

SPEC-RT-OC-PS-003: missing-status-field-defaults-to-unknown
  If the response JSON has no `"status"` field, defaults to `"unknown"` which maps to `SessionStatus::Failed`.
  test: runtime::opencode::tests::http_poll_status_missing_status_field

SPEC-RT-OC-PS-004: unparseable-json-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)`.
  test: runtime::opencode::tests::http_poll_status_unparseable_json

---

## OpenCode collect_result

Collects the final output from a completed OpenCode session.

SPEC-RT-OC-CR-001: collects-via-get-request
  Sends `GET /api/sessions/{session_id}/result` with auth headers.
  test: runtime::opencode::tests::http_collect_result_success

SPEC-RT-OC-CR-002: extracts-final-message-as-output
  The `output` field is populated from `"final_message"`. If absent, defaults to empty string.
  test: runtime::opencode::tests::http_collect_result_missing_fields

SPEC-RT-OC-CR-003: extracts-full-log-as-raw-log
  The `raw_log` field is populated from `"full_log"`. If absent, defaults to empty string.
  test: runtime::opencode::tests::http_collect_result_missing_fields

SPEC-RT-OC-CR-004: cost-extracted-via-extract-cost-from-opencode
  The `cost` field is populated by `extract_cost_from_opencode`.
  test: IMPLICIT via extract_cost tests

SPEC-RT-OC-CR-005: http-error-returns-validation-error
  If the response is non-2xx, returns `Err(ValidationError)`.
  test: runtime::opencode::tests::http_collect_result_http_error

SPEC-RT-OC-CR-006: unparseable-json-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)`.
  test: runtime::opencode::tests::http_collect_result_unparseable_json

---

## OpenCode cancel

Cancels a running OpenCode session. Idempotent.

SPEC-RT-OC-CN-001: sends-delete-to-session-endpoint
  Sends `DELETE /api/sessions/{session_id}` with auth headers.
  test: runtime::opencode::tests::http_cancel_sends_delete

SPEC-RT-OC-CN-002: always-returns-ok
  Regardless of whether the DELETE succeeds or fails, always returns `Ok(())`.
  test: runtime::opencode::tests::http_cancel_ignores_errors

---

## OpenCode teardown

Cleans up OpenCode workspace resources. Idempotent.

SPEC-RT-OC-TD-001: sends-delete-to-workspace-endpoint
  Sends `DELETE /api/workspaces/{workspace_id}` with auth headers.
  test: runtime::opencode::tests::http_teardown_sends_delete

SPEC-RT-OC-TD-002: always-returns-ok
  Regardless of whether the DELETE succeeds or fails, always returns `Ok(())`.
  test: runtime::opencode::tests::http_teardown_ignores_errors

---

## map_opencode_status

Maps OpenCode-specific status strings to baton's canonical `SessionStatus` enum. Case-insensitive. Same mapping as `map_openhands_status`.

SPEC-RT-OC-MS-001: running-variants
  `"running"`, `"pending"`, `"started"` (case-insensitive) map to `SessionStatus::Running`.
  test: runtime::opencode::tests::map_status_running

SPEC-RT-OC-MS-002: completed-variants
  `"completed"`, `"finished"`, `"done"` (case-insensitive) map to `SessionStatus::Completed`.
  test: runtime::opencode::tests::map_status_completed

SPEC-RT-OC-MS-003: failed-variants
  `"failed"` and `"error"` (case-insensitive) map to `SessionStatus::Failed`.
  test: runtime::opencode::tests::map_status_failed

SPEC-RT-OC-MS-004: timed-out-variants
  `"timed_out"` and `"timeout"` (case-insensitive) map to `SessionStatus::TimedOut`.
  test: runtime::opencode::tests::map_status_timed_out

SPEC-RT-OC-MS-005: cancelled-variants
  `"cancelled"`, `"canceled"`, `"stopped"` (case-insensitive) map to `SessionStatus::Cancelled`.
  test: runtime::opencode::tests::map_status_cancelled

SPEC-RT-OC-MS-006: unknown-defaults-to-failed
  Any unrecognized string maps to `SessionStatus::Failed`.
  test: runtime::opencode::tests::map_status_unknown_defaults_to_failed

SPEC-RT-OC-MS-007: case-insensitive-matching
  Status matching uses `.to_lowercase()` before comparison.
  test: runtime::opencode::tests::map_status_case_insensitive

---

## extract_cost_from_opencode

Extracts cost metadata from the OpenCode result response body. Same logic as `extract_cost_from_openhands`.

SPEC-RT-OC-EC-001: full-metrics-extracted
  When the response body contains a `"metrics"` object with `"input_tokens"`, `"output_tokens"`, `"model"`, and `"cost"`, all four are extracted into the `Cost` struct.
  test: runtime::opencode::tests::extract_cost_with_metrics

SPEC-RT-OC-EC-002: no-metrics-key-returns-none
  When the response body has no `"metrics"` key, returns `None`.
  test: runtime::opencode::tests::extract_cost_no_metrics

SPEC-RT-OC-EC-003: empty-metrics-returns-none
  When `"metrics"` exists but contains no token counts, returns `None`.
  test: runtime::opencode::tests::extract_cost_empty_metrics

SPEC-RT-OC-EC-004: partial-metrics-returns-some
  When at least one of `"input_tokens"` or `"output_tokens"` is present, returns `Some(Cost)`.
  test: runtime::opencode::tests::extract_cost_partial_metrics

SPEC-RT-OC-EC-005: cost-field-mapped-to-estimated-usd
  The `"cost"` field (f64) in metrics maps to `Cost::estimated_usd`.
  test: IMPLICIT via extract_cost_with_metrics

---

## OpenCodeAdapter::post_completion

SPEC-RT-OC-PC-001: posts-to-chat-completions
  Sends `POST {base_url}/v1/chat/completions` with the same OpenAI-compatible format. Parses content and cost from usage.
  test: UNTESTED

---

## ApiAdapter

API runtime adapter that wraps `ProviderClient` for OpenAI-compatible LLM APIs. Handles one-shot completions but does not support agent sessions.

### ApiAdapter::new

SPEC-RT-API-001: constructs-from-runtime-config
  Creates an `ApiAdapter` from base_url, api_key_env, default_model, and timeout_seconds. Resolves API key from environment. Strips trailing slash from base_url.
  test: UNTESTED

SPEC-RT-API-002: api-key-resolved-from-env
  When `api_key_env` is `Some(name)` and non-empty, reads the env var. If not set, returns error.
  test: UNTESTED

SPEC-RT-API-003: no-api-key-env-means-no-auth
  When `api_key_env` is `None` or empty, no API key is set.
  test: UNTESTED

### ApiAdapter::health_check

SPEC-RT-API-010: health-check-via-models-endpoint
  Sends `GET {base_url}/v1/models`. On success, returns `reachable=true` with model list. On HTTP error, returns `reachable=false`. On connection error, returns `Err`.
  test: UNTESTED

### ApiAdapter::post_completion

SPEC-RT-API-020: posts-to-chat-completions
  Sends `POST {base_url}/v1/chat/completions` with messages, model, temperature, max_tokens. Parses `choices[0].message.content` and usage for cost.
  test: UNTESTED

SPEC-RT-API-021: delegates-to-provider-client
  Uses `ProviderClient` internally for HTTP construction, response parsing, and error classification.
  test: UNTESTED

### ApiAdapter session methods

SPEC-RT-API-030: session-methods-return-error
  `create_session`, `poll_status`, `collect_result`, `cancel`, and `teardown` all return `Err(RuntimeError("API runtime does not support sessions"))`.
  test: UNTESTED
