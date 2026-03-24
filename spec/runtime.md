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

These are plain data types with public fields. They carry the information needed to create, monitor, and collect results from runtime sessions and completions.

SPEC-RT-TY-001: session-config-fields
  `SessionConfig` carries: task description, named input files (name-to-content mapping), model identifier, sandbox flag, max iteration count, timeout in seconds, and environment variables.
  test: runtime::tests::session_config_construction

SPEC-RT-TY-002: session-handle-fields
  `SessionHandle` carries: session ID and workspace ID. These are opaque identifiers returned by create_session and passed to all subsequent session operations.
  test: runtime::tests::session_handle_construction

SPEC-RT-TY-003: session-status-variants
  `SessionStatus` has exactly five states: Running, Completed, Failed, TimedOut, Cancelled. Each runtime backend maps its own status vocabulary to these five canonical states.
  test: IMPLICIT via map_status tests

SPEC-RT-TY-004: session-result-fields
  `SessionResult` carries: final status, output text, raw log, and optional cost metadata.
  test: runtime::tests::session_result_construction

SPEC-RT-TY-005: health-result-fields
  `HealthResult` carries: reachability flag, optional server version, optional model list, and optional diagnostic message.
  test: runtime::tests::health_result_construction

SPEC-RT-TY-010: completion-request-fields
  `CompletionRequest` carries: message list, model identifier, temperature, and optional max token limit.
  test: UNTESTED

SPEC-RT-TY-011: completion-result-fields
  `CompletionResult` carries: response content text and optional cost metadata.
  test: UNTESTED

---

## post_completion (trait default)

Default method on `RuntimeAdapter` that returns an error indicating the runtime doesn't support one-shot completions. Overridden by API, OpenHands, and OpenCode adapters.

SPEC-RT-PC-001: default-returns-runtime-error
  The default implementation returns an error indicating the runtime does not support one-shot completions. Runtimes that support completions override this method.
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
  test: runtime::api::tests::create_api_adapter_no_auth

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
  test: runtime::session_common::tests::map_status_running

SPEC-RT-MS-002: completed-variants
  The strings `"completed"`, `"finished"`, and `"done"` (case-insensitive) map to `SessionStatus::Completed`.
  test: runtime::session_common::tests::map_status_completed

SPEC-RT-MS-003: failed-variants
  The strings `"failed"` and `"error"` (case-insensitive) map to `SessionStatus::Failed`.
  test: runtime::session_common::tests::map_status_failed

SPEC-RT-MS-004: timed-out-variants
  The strings `"timed_out"` and `"timeout"` (case-insensitive) map to `SessionStatus::TimedOut`.
  test: runtime::session_common::tests::map_status_timed_out

SPEC-RT-MS-005: cancelled-variants
  The strings `"cancelled"`, `"canceled"`, and `"stopped"` (case-insensitive) map to `SessionStatus::Cancelled`. Both British and American spellings are accepted.
  test: runtime::session_common::tests::map_status_cancelled

SPEC-RT-MS-006: unknown-defaults-to-failed
  Any string not matching a known variant maps to `SessionStatus::Failed`. This is a conservative default: an unknown status is treated as a failure rather than silently succeeding.
  test: runtime::session_common::tests::map_status_unknown_defaults_to_failed

SPEC-RT-MS-007: case-insensitive-matching
  Status matching uses `.to_lowercase()` before comparison. `"RUNNING"`, `"Running"`, and `"running"` all map to `SessionStatus::Running`.
  test: runtime::session_common::tests::map_status_case_insensitive

---

## extract_cost_from_openhands

Extracts cost metadata from the OpenHands result response body. Returns `None` if no meaningful cost data is present.

The function uses a two-tier absence check: first, is the `"metrics"` key present at all? Second, are there any token counts? A metrics object with only a model name but no token counts is treated as "no cost data" because cost without token counts is not actionable.

SPEC-RT-EC-001: full-metrics-extracted
  When the response body contains a `"metrics"` object with `"input_tokens"` (i64), `"output_tokens"` (i64), `"model"` (string), and `"cost"` (f64), all four are extracted into the returned `Cost` struct.
  test: runtime::session_common::tests::extract_cost_with_metrics

SPEC-RT-EC-002: no-metrics-key-returns-none
  When the response body has no `"metrics"` key, returns `None`.
  test: runtime::session_common::tests::extract_cost_no_metrics

SPEC-RT-EC-003: empty-metrics-returns-none
  When the `"metrics"` object exists but contains neither `"input_tokens"` nor `"output_tokens"`, returns `None`. A metrics object with only `"model"` or `"cost"` but no token counts is not considered meaningful cost data.
  test: runtime::session_common::tests::extract_cost_empty_metrics

SPEC-RT-EC-004: partial-metrics-returns-some
  When the `"metrics"` object contains at least one of `"input_tokens"` or `"output_tokens"`, returns `Some(Cost)` with the present fields populated and absent fields as `None`.
  test: runtime::session_common::tests::extract_cost_partial_metrics

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
  test: runtime::session_common::tests::new_valid_env_var_is_resolved
  test: runtime::session_common::tests::new_missing_env_var_returns_config_error

SPEC-RT-OC-002: api-key-env-none-means-no-auth
  When `api_key_env` is `None`, no API key is resolved. The adapter operates without authentication. Auth headers will be empty.
  test: runtime::tests::create_opencode_adapter_no_auth

SPEC-RT-OC-003: api-key-env-empty-string-means-no-auth
  When `api_key_env` is `Some("")` (empty string), it is treated the same as `None`. No API key is resolved.
  test: runtime::session_common::tests::new_empty_env_var_name_treated_as_none

SPEC-RT-OC-004: trailing-slash-stripped-from-base-url
  If `base_url` ends with `/`, the trailing slash is removed before storing. This prevents double-slashes in constructed URLs.
  test: runtime::session_common::tests::new_strips_trailing_slash

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
  test: runtime::session_common::tests::adapter_with_api_key_has_auth_header

SPEC-RT-OC-AH-002: empty-headers-when-no-api-key
  When `self.api_key` is `None`, the returned `HeaderMap` is empty.
  test: runtime::session_common::tests::adapter_without_api_key_has_no_auth_in_debug

---

## OpenCode health_check

Probes the OpenCode runtime server's health endpoint. Same endpoint pattern and behavior as OpenHands.

SPEC-RT-OC-HC-001: success-returns-reachable-with-version
  On a successful (2xx) response from `GET /api/health`, returns `Ok(HealthResult)` with `reachable=true` and `version` extracted from JSON body.
  test: runtime::session_common::tests::http_health_check_success

SPEC-RT-OC-HC-002: http-error-returns-not-reachable
  On a non-2xx HTTP response, returns `Ok(HealthResult)` with `reachable=false` and `message=Some("HTTP {status}")`.
  test: runtime::session_common::tests::http_health_check_http_error

SPEC-RT-OC-HC-003: connection-error-returns-validation-error
  If the HTTP request fails at the network level, returns `Err(ValidationError)` with "Cannot reach runtime at" and the base URL.
  test: runtime::session_common::tests::http_health_check_connection_refused

SPEC-RT-OC-HC-004: malformed-json-treated-as-empty
  If the 2xx response body is not valid JSON, `version` will be `None`. No error is raised.
  test: runtime::session_common::tests::http_health_check_malformed_json

---

## OpenCode create_session

Creates a new agent session on the OpenCode runtime. Same two-phase approach as OpenHands: upload files, then create session.

SPEC-RT-OC-CS-001: workspace-id-is-uuid
  A new UUID v4 is generated for each session's workspace.
  test: IMPLICIT via http_create_session_success_no_files (workspace_id is generated)

SPEC-RT-OC-CS-002: files-uploaded-via-multipart
  For each entry in `config.files`, the file is uploaded via `POST /api/workspaces/{workspace_id}/files` as a multipart form.
  test: runtime::session_common::tests::http_create_session_success_with_files

SPEC-RT-OC-CS-003: file-upload-http-error-returns-validation-error
  If a file upload returns a non-2xx status, returns `Err(ValidationError)` with the file name and HTTP status.
  test: runtime::session_common::tests::http_create_session_file_upload_http_error

SPEC-RT-OC-CS-004: session-creation-posts-json-body
  After files are uploaded, `POST /api/sessions` is called with workspace_id, task, model, sandbox, max_iterations, and timeout.
  test: runtime::session_common::tests::http_create_session_body_contents

SPEC-RT-OC-CS-005: session-creation-http-error-includes-body
  If session creation POST returns non-2xx, returns `Err(ValidationError)` with HTTP status and response body.
  test: runtime::session_common::tests::http_create_session_http_error

SPEC-RT-OC-CS-006: missing-session-id-returns-validation-error
  If the response is valid JSON but missing `"session_id"`, returns `Err(ValidationError)`.
  test: runtime::session_common::tests::http_create_session_missing_session_id

SPEC-RT-OC-CS-007: successful-creation-returns-handle
  On success, returns `Ok(SessionHandle)` with `id` from `"session_id"` and `workspace_id` from the generated UUID.
  test: runtime::session_common::tests::http_create_session_success_no_files

SPEC-RT-OC-CS-008: unparseable-json-response-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)`.
  test: runtime::session_common::tests::http_create_session_unparseable_json

---

## OpenCode poll_status

Polls the current lifecycle state of a running OpenCode session.

SPEC-RT-OC-PS-001: polls-via-get-request
  Sends `GET /api/sessions/{session_id}/status` with auth headers. Maps status via `map_opencode_status`.
  test: runtime::session_common::tests::http_poll_status_running

SPEC-RT-OC-PS-002: http-error-returns-validation-error
  If the response is non-2xx, returns `Err(ValidationError)`.
  test: runtime::session_common::tests::http_poll_status_http_error

SPEC-RT-OC-PS-003: missing-status-field-defaults-to-unknown
  If the response JSON has no `"status"` field, defaults to `"unknown"` which maps to `SessionStatus::Failed`.
  test: runtime::session_common::tests::http_poll_status_missing_status_field

SPEC-RT-OC-PS-004: unparseable-json-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)`.
  test: runtime::session_common::tests::http_poll_status_unparseable_json

---

## OpenCode collect_result

Collects the final output from a completed OpenCode session.

SPEC-RT-OC-CR-001: collects-via-get-request
  Sends `GET /api/sessions/{session_id}/result` with auth headers.
  test: runtime::session_common::tests::http_collect_result_success

SPEC-RT-OC-CR-002: extracts-final-message-as-output
  The `output` field is populated from `"final_message"`. If absent, defaults to empty string.
  test: runtime::session_common::tests::http_collect_result_missing_fields

SPEC-RT-OC-CR-003: extracts-full-log-as-raw-log
  The `raw_log` field is populated from `"full_log"`. If absent, defaults to empty string.
  test: runtime::session_common::tests::http_collect_result_missing_fields

SPEC-RT-OC-CR-004: cost-extracted-via-extract-cost-from-opencode
  The `cost` field is populated by `extract_cost_from_opencode`.
  test: IMPLICIT via extract_cost tests

SPEC-RT-OC-CR-005: http-error-returns-validation-error
  If the response is non-2xx, returns `Err(ValidationError)`.
  test: runtime::session_common::tests::http_collect_result_http_error

SPEC-RT-OC-CR-006: unparseable-json-returns-validation-error
  If the 2xx response body is not valid JSON, returns `Err(ValidationError)`.
  test: runtime::session_common::tests::http_collect_result_unparseable_json

---

## OpenCode cancel

Cancels a running OpenCode session. Idempotent.

SPEC-RT-OC-CN-001: sends-delete-to-session-endpoint
  Sends `DELETE /api/sessions/{session_id}` with auth headers.
  test: runtime::session_common::tests::http_cancel_sends_delete

SPEC-RT-OC-CN-002: always-returns-ok
  Regardless of whether the DELETE succeeds or fails, always returns `Ok(())`.
  test: runtime::session_common::tests::http_cancel_ignores_errors

---

## OpenCode teardown

Cleans up OpenCode workspace resources. Idempotent.

SPEC-RT-OC-TD-001: sends-delete-to-workspace-endpoint
  Sends `DELETE /api/workspaces/{workspace_id}` with auth headers.
  test: runtime::session_common::tests::http_teardown_sends_delete

SPEC-RT-OC-TD-002: always-returns-ok
  Regardless of whether the DELETE succeeds or fails, always returns `Ok(())`.
  test: runtime::session_common::tests::http_teardown_ignores_errors

---

## map_opencode_status

Maps OpenCode-specific status strings to baton's canonical `SessionStatus` enum. Case-insensitive. Same mapping as `map_openhands_status`.

SPEC-RT-OC-MS-001: running-variants
  `"running"`, `"pending"`, `"started"` (case-insensitive) map to `SessionStatus::Running`.
  test: runtime::session_common::tests::map_status_running

SPEC-RT-OC-MS-002: completed-variants
  `"completed"`, `"finished"`, `"done"` (case-insensitive) map to `SessionStatus::Completed`.
  test: runtime::session_common::tests::map_status_completed

SPEC-RT-OC-MS-003: failed-variants
  `"failed"` and `"error"` (case-insensitive) map to `SessionStatus::Failed`.
  test: runtime::session_common::tests::map_status_failed

SPEC-RT-OC-MS-004: timed-out-variants
  `"timed_out"` and `"timeout"` (case-insensitive) map to `SessionStatus::TimedOut`.
  test: runtime::session_common::tests::map_status_timed_out

SPEC-RT-OC-MS-005: cancelled-variants
  `"cancelled"`, `"canceled"`, `"stopped"` (case-insensitive) map to `SessionStatus::Cancelled`.
  test: runtime::session_common::tests::map_status_cancelled

SPEC-RT-OC-MS-006: unknown-defaults-to-failed
  Any unrecognized string maps to `SessionStatus::Failed`.
  test: runtime::session_common::tests::map_status_unknown_defaults_to_failed

SPEC-RT-OC-MS-007: case-insensitive-matching
  Status matching uses `.to_lowercase()` before comparison.
  test: runtime::session_common::tests::map_status_case_insensitive

---

## extract_cost_from_opencode

Extracts cost metadata from the OpenCode result response body. Same logic as `extract_cost_from_openhands`.

SPEC-RT-OC-EC-001: full-metrics-extracted
  When the response body contains a `"metrics"` object with `"input_tokens"`, `"output_tokens"`, `"model"`, and `"cost"`, all four are extracted into the `Cost` struct.
  test: runtime::session_common::tests::extract_cost_with_metrics

SPEC-RT-OC-EC-002: no-metrics-key-returns-none
  When the response body has no `"metrics"` key, returns `None`.
  test: runtime::session_common::tests::extract_cost_no_metrics

SPEC-RT-OC-EC-003: empty-metrics-returns-none
  When `"metrics"` exists but contains no token counts, returns `None`.
  test: runtime::session_common::tests::extract_cost_empty_metrics

SPEC-RT-OC-EC-004: partial-metrics-returns-some
  When at least one of `"input_tokens"` or `"output_tokens"` is present, returns `Some(Cost)`.
  test: runtime::session_common::tests::extract_cost_partial_metrics

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
  test: runtime::api::tests::create_api_adapter_no_auth
  test: runtime::api::tests::create_adapter_stores_default_model
  test: runtime::api::tests::create_adapter_no_default_model

SPEC-RT-API-002: api-key-resolved-from-env
  When `api_key_env` is `Some(name)` and non-empty, reads the env var. If not set, returns error.
  test: runtime::api::tests::new_with_missing_env_var_returns_error

SPEC-RT-API-003: no-api-key-env-means-no-auth
  When `api_key_env` is `None` or empty, no API key is set.
  test: runtime::api::tests::new_with_empty_api_key_env_succeeds
  test: runtime::api::tests::new_with_none_api_key_env_succeeds

### ApiAdapter::health_check

SPEC-RT-API-010: health-check-via-models-endpoint
  Sends `GET {base_url}/v1/models`. On success, returns `reachable=true` with model list. On HTTP error, returns `reachable=false`. On connection error, returns `Err`.
  test: runtime::api::tests::health_check_success_returns_models
  test: runtime::api::tests::health_check_unreachable
  test: runtime::api::tests::health_check_auth_failed_still_reachable
  test: runtime::api::tests::health_check_other_error_still_reachable

### ApiAdapter::post_completion

SPEC-RT-API-020: posts-to-chat-completions
  Sends `POST {base_url}/v1/chat/completions` with messages, model, temperature, max_tokens. Parses `choices[0].message.content` and usage for cost.
  test: runtime::api::tests::post_completion_success
  test: runtime::api::tests::post_completion_includes_max_tokens_when_set
  test: runtime::api::tests::post_completion_without_max_tokens_omits_field
  test: runtime::api::tests::post_completion_no_cost_when_no_usage

SPEC-RT-API-021: delegates-to-provider-client
  Uses `ProviderClient` internally for HTTP construction, response parsing, and error classification.
  test: runtime::api::tests::post_completion_empty_content_returns_error
  test: runtime::api::tests::post_completion_http_error_returns_error
  test: runtime::api::tests::post_completion_model_not_found
  test: runtime::api::tests::post_completion_auth_failure
  test: runtime::api::tests::post_completion_rate_limited

### ApiAdapter session methods

SPEC-RT-API-030: session-methods-return-error
  `create_session`, `poll_status`, `collect_result`, `cancel`, and `teardown` all return `Err(RuntimeError("API runtime does not support sessions"))`.
  test: runtime::api::tests::create_session_returns_error
  test: runtime::api::tests::poll_status_returns_error
  test: runtime::api::tests::collect_result_returns_error
  test: runtime::api::tests::cancel_returns_error
  test: runtime::api::tests::teardown_returns_error

---

## ClaudeCodeAdapter

Subprocess-based runtime adapter for Claude Code (Anthropic's CLI tool). Unlike OpenHands/OpenCode which use HTTP APIs, this adapter spawns `claude` as a child process. Supports both query mode (one-shot completions via `post_completion`) and session mode (background subprocess lifecycle).

Internal state: tracks running child processes via `Mutex<HashMap<String, ChildState>>` because trait methods take `&self`. Each session has a workspace directory (temp dir with input files), an optional `Child` process handle, and collected stdout/stderr.

### ClaudeCodeAdapter::new

Constructs the adapter from connection parameters. The `base_url` config field is repurposed as the path to the `claude` binary (defaults to `"claude"`).

SPEC-RT-CC-001: claude-path-from-base-url
  The `base_url` config value is stored as the path to the `claude` binary. If `base_url` is `"claude"` (the default), the system PATH is used to locate the binary at runtime.
  test: runtime::claude_code::tests::new_stores_claude_path

SPEC-RT-CC-002: api-key-env-resolved-from-environment
  When `api_key_env` is `Some(name)` and `name` is non-empty, the constructor reads the environment variable. If set, the value is stored for passing to subprocess environments. If not set, returns `Err(ConfigError)` with a message containing the variable name and "is not set".
  test: runtime::claude_code::tests::new_missing_env_var_returns_config_error
  test: runtime::claude_code::tests::new_valid_env_var_is_resolved

SPEC-RT-CC-003: api-key-env-none-means-no-auth
  When `api_key_env` is `None`, no API key is resolved. The subprocess inherits the parent process's environment (which may already have `ANTHROPIC_API_KEY` set).
  test: runtime::claude_code::tests::new_no_api_key_env

SPEC-RT-CC-004: api-key-env-empty-string-means-no-auth
  When `api_key_env` is `Some("")` (empty string), it is treated the same as `None`.
  test: runtime::claude_code::tests::new_empty_api_key_env

SPEC-RT-CC-005: max-iterations-stored-as-max-turns
  The `max_iterations` config value is stored as `max_turns` for use with Claude Code's `--max-turns` flag.
  test: runtime::claude_code::tests::new_stores_config_fields

SPEC-RT-CC-006: sessions-map-initialized-empty
  The internal sessions map is initialized as an empty `HashMap` wrapped in a `Mutex`.
  test: IMPLICIT via constructor tests

---

### ClaudeCodeAdapter::health_check

Verifies that the `claude` binary is available and functional by running `claude --version`.

SPEC-RT-CC-HC-001: runs-claude-version
  Spawns `{claude_path} --version` as a subprocess. On success (exit code 0), returns `Ok(HealthResult)` with `reachable=true` and `version` extracted from stdout.
  test: runtime::claude_code::tests::health_check_success

SPEC-RT-CC-HC-002: binary-not-found-returns-not-reachable
  If the binary cannot be found (spawn fails with `NotFound`), returns `Ok(HealthResult)` with `reachable=false` and a diagnostic message.
  test: runtime::claude_code::tests::health_check_binary_not_found

SPEC-RT-CC-HC-003: non-zero-exit-returns-not-reachable
  If the binary exits with a non-zero status, returns `Ok(HealthResult)` with `reachable=false` and a message containing the exit code.
  test: runtime::claude_code::tests::health_check_non_zero_exit

---

### ClaudeCodeAdapter::create_session

Creates a new agent session by spawning `claude -p` as a background subprocess.

SPEC-RT-CC-CS-001: workspace-dir-created-as-temp-dir
  A new temporary directory is created for the session's workspace. Input files from `config.files` are copied into this directory.
  test: runtime::claude_code::tests::create_session_creates_workspace

SPEC-RT-CC-CS-002: files-copied-to-workspace
  For each entry in `config.files`, the file at the path (value) is read from disk and written to the workspace directory using the map key as the filename.
  test: runtime::claude_code::tests::create_session_copies_files

SPEC-RT-CC-CS-003: file-read-error-returns-validation-error
  If reading a source file fails, returns `Err(ValidationError)` with a message containing the file name and path.
  test: runtime::claude_code::tests::create_session_file_read_error

SPEC-RT-CC-CS-004: subprocess-spawned-with-print-mode
  Spawns `{claude_path} -p "{task}" --output-format json` with the workspace directory as the current working directory. If `default_model` is set, adds `--model {model}`. If `max_turns > 0`, adds `--max-turns {max_turns}`.
  test: UNTESTED (subprocess args verified indirectly via poll_status and collect_result tests)

SPEC-RT-CC-CS-005: api-key-passed-via-env
  If an API key was resolved during construction, it is passed to the subprocess via the `ANTHROPIC_API_KEY` environment variable.
  test: UNTESTED

SPEC-RT-CC-CS-006: handle-returned-with-session-id
  On success, returns `Ok(SessionHandle)` with `id` set to a generated UUID and `workspace_id` set to the workspace directory path.
  test: runtime::claude_code::tests::create_session_returns_handle

SPEC-RT-CC-CS-007: spawn-failure-returns-runtime-error
  If spawning the subprocess fails, returns `Err(RuntimeError)` with "Failed to spawn claude" and the error details.
  test: runtime::claude_code::tests::create_session_spawn_failure

SPEC-RT-CC-CS-008: child-stored-in-sessions-map
  The spawned `Child` process handle is stored in the internal sessions map keyed by the session ID.
  test: IMPLICIT via poll_status and collect_result tests

---

### ClaudeCodeAdapter::poll_status

Checks whether the background `claude` subprocess has exited.

SPEC-RT-CC-PS-001: running-when-child-not-exited
  If `child.try_wait()` returns `Ok(None)`, the process is still running. Returns `Ok(SessionStatus::Running)`.
  test: runtime::claude_code::tests::poll_status_running

SPEC-RT-CC-PS-002: completed-on-zero-exit
  If `child.try_wait()` returns `Ok(Some(status))` with `status.success()`, returns `Ok(SessionStatus::Completed)`.
  test: runtime::claude_code::tests::poll_status_completed

SPEC-RT-CC-PS-003: failed-on-non-zero-exit
  If `child.try_wait()` returns `Ok(Some(status))` with a non-zero exit code, returns `Ok(SessionStatus::Failed)`.
  test: runtime::claude_code::tests::poll_status_failed

SPEC-RT-CC-PS-004: unknown-session-returns-error
  If the session ID is not found in the sessions map, returns `Err(RuntimeError)` with "Unknown session".
  test: runtime::claude_code::tests::poll_status_unknown_session

SPEC-RT-CC-PS-005: try-wait-error-returns-runtime-error
  If `child.try_wait()` returns `Err`, returns `Err(RuntimeError)` with the error details.
  test: UNTESTED

---

### ClaudeCodeAdapter::collect_result

Waits for the subprocess to finish (if still running) and collects its output.

SPEC-RT-CC-CR-001: waits-for-child-to-complete
  If the child process has not yet exited, calls `child.wait()` to block until completion.
  test: IMPLICIT via collect_result tests

SPEC-RT-CC-CR-002: stdout-parsed-as-json
  The child's stdout is read and parsed as JSON. The `result` field becomes the `output`. The full stdout becomes `raw_log`.
  test: runtime::claude_code::tests::collect_result_parses_json

SPEC-RT-CC-CR-003: cost-extracted-from-json
  Cost data is extracted from the JSON output's `usage` and `cost_usd` fields. `usage.input_tokens` → `Cost.input_tokens`, `usage.output_tokens` → `Cost.output_tokens`, `cost_usd` → `Cost.estimated_usd`.
  test: runtime::claude_code::tests::collect_result_extracts_cost

SPEC-RT-CC-CR-004: non-json-stdout-used-as-raw-output
  If stdout is not valid JSON, the raw text is used as `output` and `raw_log`. Cost is `None`.
  test: runtime::claude_code::tests::collect_result_non_json_output

SPEC-RT-CC-CR-005: unknown-session-returns-error
  If the session ID is not found in the sessions map, returns `Err(RuntimeError)` with "Unknown session".
  test: runtime::claude_code::tests::collect_result_unknown_session

SPEC-RT-CC-CR-006: status-from-exit-code
  The `SessionResult.status` is `Completed` for exit code 0, `Failed` for non-zero exit codes.
  test: runtime::claude_code::tests::collect_result_failed_status

---

### ClaudeCodeAdapter::cancel

Kills the running `claude` subprocess. Idempotent.

SPEC-RT-CC-CN-001: kills-child-process
  If the session exists and has a running child process, calls `child.kill()`. Always returns `Ok(())` regardless of whether the kill succeeds.
  test: runtime::claude_code::tests::cancel_kills_process

SPEC-RT-CC-CN-002: unknown-session-returns-ok
  If the session ID is not found in the sessions map, returns `Ok(())` silently. This is idempotent behavior for cleanup paths.
  test: runtime::claude_code::tests::cancel_unknown_session_ok

---

### ClaudeCodeAdapter::teardown

Cleans up the workspace directory and removes the session from the internal map. Idempotent.

SPEC-RT-CC-TD-001: removes-workspace-directory
  If the session exists, removes the workspace directory (identified by `handle.workspace_id`). Ignores errors from directory removal.
  test: runtime::claude_code::tests::teardown_removes_workspace

SPEC-RT-CC-TD-002: removes-session-from-map
  Removes the session entry from the internal sessions map.
  test: runtime::claude_code::tests::teardown_removes_from_map

SPEC-RT-CC-TD-003: unknown-session-returns-ok
  If the session ID is not found, returns `Ok(())` silently.
  test: runtime::claude_code::tests::teardown_unknown_session_ok

---

### ClaudeCodeAdapter::post_completion

Runs a one-shot completion by spawning `claude -p` synchronously and parsing the JSON output.

SPEC-RT-CC-PC-001: spawns-claude-with-prompt
  Constructs a prompt from the `messages` array in the `CompletionRequest` (concatenates message contents). Spawns `{claude_path} -p "{prompt}" --output-format json`. If `request.model` is non-empty, adds `--model {model}`.
  test: runtime::claude_code::tests::post_completion_success

SPEC-RT-CC-PC-002: parses-json-output-for-content
  Parses stdout as JSON. Extracts `result` field as `CompletionResult.content`.
  test: runtime::claude_code::tests::post_completion_parses_content

SPEC-RT-CC-PC-003: extracts-cost-from-output
  Extracts `usage.input_tokens`, `usage.output_tokens`, and `cost_usd` from the JSON output into `CompletionResult.cost`.
  test: runtime::claude_code::tests::post_completion_extracts_cost

SPEC-RT-CC-PC-004: empty-content-returns-error
  If the `result` field is empty or missing, returns `Err(ValidationError)` with "empty response".
  test: runtime::claude_code::tests::post_completion_empty_content_error

SPEC-RT-CC-PC-005: non-zero-exit-returns-error
  If the subprocess exits with a non-zero code, returns `Err(RuntimeError)` with stderr content.
  test: runtime::claude_code::tests::post_completion_non_zero_exit

SPEC-RT-CC-PC-006: spawn-failure-returns-error
  If spawning the subprocess fails, returns `Err(RuntimeError)` with "Failed to spawn claude".
  test: runtime::claude_code::tests::post_completion_spawn_failure

---

### create_adapter for claude-code

SPEC-RT-CA-007: claude-code-type-creates-adapter
  When `runtime_config.runtime_type` is `"claude-code"`, `create_adapter` constructs a `ClaudeCodeAdapter` via `ClaudeCodeAdapter::new`, passing through base_url, api_key_env, default_model, timeout_seconds, and max_iterations. Returns `Ok(Box<dyn RuntimeAdapter>)`.
  test: runtime::tests::create_claude_code_adapter

### parse_claude_output (internal helper)

SPEC-RT-CC-PO-001: parses-result-field
  Given valid JSON with a `"result"` string field, returns the result text as content.
  test: runtime::claude_code::tests::parse_output_result_field

SPEC-RT-CC-PO-002: parses-cost-fields
  Extracts `cost_usd` (f64) → `estimated_usd`, `usage.input_tokens` (i64) → `input_tokens`, `usage.output_tokens` (i64) → `output_tokens` from the JSON.
  test: runtime::claude_code::tests::parse_output_cost_fields

SPEC-RT-CC-PO-003: missing-result-returns-empty
  If the `"result"` field is absent, returns empty string as content.
  test: runtime::claude_code::tests::parse_output_missing_result

SPEC-RT-CC-PO-004: missing-usage-returns-no-cost
  If `"usage"` and `"cost_usd"` are absent, cost is `None`.
  test: runtime::claude_code::tests::parse_output_no_cost
