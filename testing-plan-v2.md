# Testing Plan: Closing UNTESTED Coverage Gaps

*Updated to reflect the `provider.rs` extraction from `exec.rs` and `main.rs`.*

## 1. HTTP Mocking Framework Selection

### Recommendation: `httpmock`

**Why httpmock over the alternatives:**

| Criterion | httpmock | wiremock | mockito | raw TCP (current) |
|---|---|---|---|---|
| Sync API | Native | Async-only | Sync | Sync |
| Parallel tests | Yes | Yes | Yes (recent) | Yes |
| Multiple servers per test | Yes | Yes | No (1 global) | Yes |
| Request matching | Rich | Rich | Basic | Manual parsing |
| Request body inspection | Built-in `.json_body_includes()` | Via custom matchers | Via `.match_body()` | Manual string search |
| Multipart support | Yes | Via custom matchers | No | Would be painful |
| Call count verification | `.assert_hits(n)` | `.expect()` auto-verify | `.assert()` | Manual via join handle |
| Delayed responses | `.delay()` | `ResponseTemplate::set_delay()` | No | Manual `thread::sleep` |

**The critical factor is the sync API.** Baton uses `reqwest::blocking` everywhere — in `provider.rs` (LLM provider API calls), `runtime/openhands.rs` (session management), and `main.rs` (GitHub API for updates). `wiremock` is async-only, and calling `reqwest::blocking` inside a tokio runtime panics. `httpmock` works natively with synchronous code:

```rust
let server = MockServer::start();  // no .await, no async runtime
server.mock(|when, then| {
    when.method(POST).path("/v1/chat/completions");
    then.status(200).json_body(json!({...}));
});
// reqwest::blocking::Client works directly against server.url(...)
```

**Cargo.toml addition:**
```toml
[dev-dependencies]
httpmock = "0.8"
```

No feature flags needed. httpmock bundles its own server — no external process, no tokio dependency in your test binary.

---

## 2. Migration: Existing Raw TCP Tests in `exec.rs`

The current `start_mock_server()` in exec.rs handles exactly one request per server, uses a 4KB buffer (could truncate large requests), requires manual HTTP response formatting, and returns the raw request string for manual parsing. There are ~15 tests using this pattern.

After the provider extraction, these tests still work — they exercise `execute_llm_completion` end-to-end, and `ProviderClient::post_completion` makes a real HTTP call to whatever URL is in the mock server. The raw TCP server doesn't know or care that the request now comes from provider.rs instead of inline exec.rs code.

### Migration strategy: phased, not big-bang

**Phase 1: Add httpmock alongside raw TCP.** Write all new tests with httpmock. Don't touch existing passing tests yet.

**Phase 2: Migrate existing exec.rs LLM tests.** Each migration is mechanical:

Before (raw TCP):
```rust
fn llm_completion_http_429() {
    let (port, handle) = start_mock_server(429, r#"{"error": "rate limited"}"#);
    let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));
    // ... test body ...
    handle.join().unwrap();
}
```

After (httpmock):
```rust
fn llm_completion_http_429() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(429).json_body(json!({"error": "rate limited"}));
    });
    let config = th::config_with_provider(&server.url(""));
    // ... test body ...
    mock.assert();  // verifies the endpoint was actually hit
}
```

**Phase 3: Remove `start_mock_server()` and the raw TCP imports.** Once all callers are migrated.

### Benefits of migration

- Request body assertions become declarative (`when.json_body_includes(...)`) instead of string searches on raw bytes
- No more 4KB buffer truncation risk on large prompt payloads
- Call count verification catches silent test failures where the HTTP call was never made
- Multi-request tests become possible (OpenHands session lifecycle needs this)

---

## 3. Test Plan by Module

### 3a. `provider.rs` — ProviderClient HTTP Tests (NEW)

The provider extraction created a clean seam for testing HTTP transport independently from the exec pipeline. Tests can target `ProviderClient` methods directly against httpmock — no need for validators, artifacts, contexts, or config beyond a `Provider` struct.

**Helper needed: `test_client(server: &MockServer) -> ProviderClient`**
Constructs a client pointed at the mock server's URL with no auth and a short timeout.

```rust
fn test_client(server: &MockServer) -> ProviderClient {
    let provider = Provider {
        api_base: server.url(""),
        api_key_env: "".into(),
        default_model: "test-model".into(),
    };
    ProviderClient::new(&provider, "test", 10).unwrap()
}
```

#### post_completion (SPEC-PV-PC-001 through PC-008)

Most happy-path and error-path behavior is already covered IMPLICITLY via the exec.rs LLM tests that flow through `ProviderClient`. The new tests target gaps that were hard to reach through exec:

| Test | What it covers | Mock setup |
|---|---|---|
| `post_completion_malformed_json` | SPEC-PV-PC-005: non-JSON 200 → MalformedResponse | `POST /v1/chat/completions` → 200 + "not json" |
| `post_completion_body_forwarded` | Verify request body arrives intact (model, messages, temperature) | `when.json_body_includes(...)` → 200 + valid response |
| `post_completion_auth_header_sent` | Verify Bearer header when api_key is set | `when.header("Authorization", ...)` → 200 |
| `post_completion_no_auth_header` | Verify no Authorization header when api_key_env is empty | Negative match on Authorization → 200 |

#### list_models (SPEC-PV-LM-001 through LM-005)

Entirely UNTESTED — no existing test exercises this endpoint at all.

| Test | What it covers | Mock setup |
|---|---|---|
| `list_models_success` | Returns model IDs from data array | `GET /v1/models` → 200 + `{"data": [{"id": "m1"}, {"id": "m2"}]}` |
| `list_models_empty_data` | Empty array returns empty vec | `GET /v1/models` → 200 + `{"data": []}` |
| `list_models_no_data_field` | Missing data field returns empty vec | `GET /v1/models` → 200 + `{}` |
| `list_models_auth_failure` | 401 → AuthFailed | `GET /v1/models` → 401 |
| `list_models_http_error` | 500 → HttpError | `GET /v1/models` → 500 |

#### test_completion (SPEC-PV-TC-001 through TC-003)

Entirely UNTESTED.

| Test | What it covers | Mock setup |
|---|---|---|
| `test_completion_success` | 200 → Ok(true) | `POST /v1/chat/completions` → 200 |
| `test_completion_verifies_body` | Request includes max_tokens: 1 | `when.json_body_includes(json!({"max_tokens": 1}))` → 200 |
| `test_completion_http_error` | 401 → AuthFailed error | `POST /v1/chat/completions` → 401 |

#### send_request error paths (SPEC-PV-SR-001 through SR-002)

| Test | What it covers | Mock setup |
|---|---|---|
| `send_request_timeout` | SPEC-PV-SR-001: timeout → Timeout error with stored seconds | httpmock `.delay(Duration::from_secs(5))`, client with 1s timeout |
| `send_request_unreachable` | SPEC-PV-SR-002: connection refused → Unreachable | Point client at dead port (no server) |

#### Total new tests for provider.rs: ~14

These 14 tests fill 14 UNTESTED assertions from spec/provider.md. Combined with the 16 tests already in provider.rs (construction, error Display, extract_cost), the module reaches 30 tests.

---

### 3b. `runtime/openhands.rs` — HTTP-Level Adapter Tests

This is the highest-value target. Every trait method on `OpenHandsAdapter` is UNTESTED at the HTTP level. The `MockRuntimeAdapter` in test_helpers.rs covers the *exec.rs orchestration* of the trait, but the actual HTTP client code — URL construction, header handling, multipart uploads, JSON parsing, error mapping — has zero coverage.

Each test creates an `OpenHandsAdapter` pointed at a local `MockServer` and verifies the adapter makes the right HTTP calls and handles responses correctly.

**Helper needed: `test_adapter(server: &MockServer) -> OpenHandsAdapter`**
Constructs an adapter pointed at the mock server's URL with no auth, sensible defaults.

#### health_check (SPEC-RT-HC-001 through HC-004)

| Test | What it does | Mock setup |
|---|---|---|
| `health_check_success` | 200 with `{"version": "1.2.3"}` → reachable=true, version=Some | `GET /api/health` → 200 + JSON |
| `health_check_http_error` | 503 → reachable=false, message="HTTP 503" | `GET /api/health` → 503 |
| `health_check_malformed_json` | 200 with non-JSON body → reachable=true, version=None | `GET /api/health` → 200 + "not json" |
| `health_check_connection_refused` | No mock server at all → Err with "Cannot reach" | Point adapter at dead port |

#### create_session (SPEC-RT-CS-001 through CS-011)

This method makes multiple HTTP calls (file uploads + session creation), so the mock server needs multiple routes configured.

| Test | What it does | Mock setup |
|---|---|---|
| `create_session_success_no_files` | Empty files map → skips uploads, creates session | `POST /api/sessions` → 200 + `{"session_id": "s1"}` |
| `create_session_success_with_files` | Uploads files, then creates session | `POST /api/workspaces/*/files` → 200, `POST /api/sessions` → 200 |
| `create_session_file_upload_http_error` | File upload returns 500 → Err with file name | `POST /api/workspaces/*/files` → 500 |
| `create_session_http_error` | Session creation returns 400 → Err with body | `POST /api/sessions` → 400 + error body |
| `create_session_missing_session_id` | 200 but no session_id field → Err | `POST /api/sessions` → 200 + `{}` |
| `create_session_unparseable_json` | 200 but garbage body → Err | `POST /api/sessions` → 200 + "not json" |
| `create_session_body_contents` | Verify the JSON body includes model, task, sandbox, etc. | `POST /api/sessions` with body matching → 200 |

**File upload tests need a real temp file** on disk since the adapter reads from `config.files` paths. Use `tempfile::NamedTempFile`.

#### poll_status (SPEC-RT-PS-001 through PS-005)

| Test | What it does | Mock setup |
|---|---|---|
| `poll_status_running` | Returns Running status | `GET /api/sessions/s1/status` → 200 + `{"status": "running"}` |
| `poll_status_completed` | Returns Completed | Same pattern, "completed" |
| `poll_status_http_error` | 500 → Err with "Failed to poll" | 500 response |
| `poll_status_missing_status_field` | No status in JSON → defaults to Failed | `{}` body |
| `poll_status_unparseable_json` | Non-JSON 200 → Err | "not json" body |

#### collect_result (SPEC-RT-CR-001 through CR-008)

| Test | What it does | Mock setup |
|---|---|---|
| `collect_result_success` | Extracts output, raw_log, status, cost | Full JSON response |
| `collect_result_missing_fields` | Optional fields default to empty strings | Minimal JSON |
| `collect_result_http_error` | 500 → Err | 500 response |
| `collect_result_unparseable_json` | Non-JSON 200 → Err | "not json" body |

#### cancel / teardown (SPEC-RT-CN-001 through TD-004)

| Test | What it does | Mock setup |
|---|---|---|
| `cancel_sends_post` | Verifies POST to `/api/sessions/{id}/cancel` | 200 response, assert hit |
| `cancel_ignores_errors` | 500 → still returns Ok(()) | 500 response |
| `teardown_sends_delete` | Verifies DELETE to `/api/workspaces/{workspace_id}` | 200 response, assert hit |
| `teardown_ignores_errors` | 500 → still returns Ok(()) | 500 response |

#### Total new tests for openhands.rs: ~20

---

### 3c. `exec.rs` — UNTESTED Assertions

With the provider extraction, the exec module's remaining UNTESTED gaps are narrower. HTTP transport, auth, and error classification are now provider.rs's responsibility. What exec still owns: config/provider resolution, prompt resolution, request body construction, verdict parsing, and session orchestration.

| Assertion | Test approach |
|---|---|
| SPEC-EX-LC-003: api-key-env-not-set | Now tested at provider level (SPEC-PV-CL-002). At exec level: set a non-empty `api_key_env` on the test provider, don't set the env var. Assert exec returns Status::Error with the formatted ProviderError. |
| SPEC-EX-LC-005: prompt-file-resolution | Create a temp dir with a .md prompt file. Build a validator with the filename as prompt value. Point config's prompts_dir at the temp dir. |
| SPEC-EX-LC-009: max-tokens-in-request | Now testable at provider level with httpmock `when.json_body_includes(json!({"max_tokens": 100}))`. At exec level, verify the request body is constructed correctly by inspecting the mock server request. |
| SPEC-EX-LC-010: timeout-uses-validator-timeout | Now handled by ProviderClient constructor — test at provider level via SPEC-PV-CL-004. |
| SPEC-EX-LC-012: timeout-error-distinguished | Now tested at provider level (SPEC-PV-SR-001). At exec level, verify the feedback string contains "timed out". |
| SPEC-EX-LC-017: malformed-json-response | Now tested at provider level (SPEC-PV-PC-005). At exec level, verify exec maps MalformedResponse to Status::Error. |
| SPEC-EX-DS-002: poll-loop-sleeps-2-seconds | Not worth testing (timing). Leave as UNTESTED. |
| SPEC-EX-DS-004: poll-error-cancels-and-tears-down | Add `with_poll_error(msg)` to MockRuntimeAdapter. Verify cancel + teardown called. |

**MockRuntimeAdapter enhancement:** Add `with_poll_error(msg: &str)` to test_helpers.rs so that `poll_status` returns `Err` after N polls. This fills SPEC-EX-DS-004.

Several gaps that were previously "exec.rs UNTESTED" are now "provider.rs UNTESTED" — the ownership shifted with the code. Specifically, LC-010 (timeout setting), LC-012 (timeout vs unreachable distinction), and LC-017 (malformed JSON) are now primarily provider-level concerns. Exec-level tests for these would be redundant unless we want to verify the error message formatting specifically.

#### Exec-level tests still worth writing: ~5

The remaining exec-specific tests are:
1. `api_key_env_not_set` — verify ProviderClient::new error is wrapped correctly
2. `prompt_file_resolution` — exec-only concern (provider doesn't know about prompts)
3. `poll_error_cancels_and_tears_down` — exec/session orchestration concern
4. `max_tokens_in_request_body` — verify exec builds the body correctly (optional, could also test at provider level)
5. `prompt_file_not_found` — error path when .md file doesn't exist

---

### 3d. `main.rs` / `tests/cli.rs` — CLI Integration Tests

These test the compiled binary via `assert_cmd`. Grouped by subcommand.

#### cmd_check gaps

| Assertion | Test approach |
|---|---|
| SPEC-MN-CA-002: missing-equals-context | `--context "noequals"` → assert exit 2, stderr contains "Invalid context format" |
| SPEC-MN-CK-022: artifact-error-exits-2 | `--artifact /nonexistent/file` → exit 2 |
| SPEC-MN-CK-031: context-file-error-exits-2 | `--context spec=/nonexistent` → exit 2 |
| SPEC-MN-CK-042: dry-run-shows-run-if | Config with run_if validator, `--dry-run` → stderr contains the expression |
| SPEC-MN-CK-051: suppress-errors | Validator that errors + `--suppress-errors` → exit 0 |
| SPEC-MN-CK-052: suppress-all | Failing validator + `--suppress-all` → exit 0 |
| SPEC-MN-CK-063: history-errors-are-warnings | Point `--config` at a config with an unwritable history_db path. Verify "Warning:" in stderr but verdict still output. |
| SPEC-MN-CK-073: unknown-format | `--format potato` → stderr "Unknown format", stdout is JSON |
| SPEC-MN-CK-090: stdin-temp-cleanup | Pipe stdin, verify no leftover files in .baton/tmp after. |

#### cmd_init gaps

| Assertion | Test approach |
|---|---|
| SPEC-MN-IN-004: creates-prompt-templates | `baton init` → check prompts/spec-compliance.md etc. exist |
| SPEC-MN-IN-005: minimal-skips-prompts | `baton init --minimal` → prompts/ doesn't exist |
| SPEC-MN-IN-006: prompts-only-skips-config | `baton init --prompts-only` → baton.toml doesn't exist, prompts/ does |
| SPEC-MN-IN-007: existing-prompts-not-overwritten | Create prompts/spec-compliance.md with custom content, `baton init --prompts-only`, verify content unchanged |

#### cmd_list gaps

| Assertion | Test approach |
|---|---|
| SPEC-MN-LS-010: gate-not-found | `baton list --gate nonexistent` → exit 1 |

#### cmd_history gaps

| Assertion | Test approach |
|---|---|
| SPEC-MN-HY-003: artifact-hash-path | Run a check, grab the artifact hash from output, query with `--artifact-hash` → verify result |
| SPEC-MN-HY-005: empty-results-message | `baton history --gate nonexistent-gate` → "No verdicts found." |

#### cmd_validate_config gaps

| Assertion | Test approach |
|---|---|
| SPEC-MN-VC-001: parse-error | Write invalid TOML to a file, `--config bad.toml` → exit 1 |
| SPEC-MN-VC-003/004: warnings-and-errors-printed | Config with both warnings and errors → verify both "Warning:" and "Error:" in stderr |
| SPEC-MN-VC-005: errors-exit-1-warnings-exit-0 | Two tests: one config with only warnings (exit 0), one with errors (exit 1) |

#### cmd_version gaps

| Assertion | Test approach |
|---|---|
| SPEC-MN-VR-002: spec-version | `baton version` → stdout contains "spec version: 0.4" |
| SPEC-MN-VR-004: config-not-found | Run `baton version` in empty dir → "config: not found" |

#### cmd_clean gaps

| Assertion | Test approach |
|---|---|
| SPEC-MN-CL-002/003/004: stale-threshold-and-cleanup | Create files with timestamps >1hr old (use `filetime` crate or touch + set mtime). Verify `--dry-run` reports but doesn't delete. Verify normal run deletes. |

#### cmd_check_provider — NOW TESTABLE

With `check_single_provider` rewritten to use `ProviderClient`, the function takes a `Provider` struct whose `api_base` can point at an httpmock server. Integration tests start a mock server in the test process, write a temporary baton.toml with the mock URL, then invoke `baton check-provider` as a subprocess. httpmock binds a real port, so the baton subprocess can reach it.

| Assertion | Test approach |
|---|---|
| SPEC-MN-SP-001: missing-api-key-returns-false | Config with `api_key_env = "NONEXISTENT"`. No mock needed — fails before HTTP. |
| SPEC-MN-SP-003: auth-failure | Mock `/v1/models` → 401. Assert stderr contains "Authentication failed". |
| SPEC-MN-SP-005: model-found-in-list | Mock `/v1/models` → 200 with model in data array. Assert "OK" + exit 0. |
| SPEC-MN-SP-006: model-not-found-in-list | Mock `/v1/models` → 200 with different models. Assert "WARN" + "not found". |
| SPEC-MN-SP-007: fallback-test-completion | Mock `/v1/models` → 404, `/v1/chat/completions` → 200. Assert "OK". |

#### cmd_check_runtime — still requires httpmock-as-subprocess pattern

Same as check-provider: write a baton.toml with a `[runtimes.test]` whose `base_url` points at the mock server.

| Assertion | Test approach |
|---|---|
| SPEC-MN-CR-006: health-check-reachable | Mock `GET /api/health` → 200 + `{"version": "1.0"}`. Assert "OK". |
| SPEC-MN-CR-007: health-check-unreachable | Mock `GET /api/health` → 503. Assert "ERROR" + "not reachable". |
| SPEC-MN-CR-001: no-runtimes-exits-1 | Config with no `[runtimes]` section. No mock needed. |
| SPEC-MN-CR-003: named-runtime-not-found | Config has runtime "alpha" but command says `baton check-runtime beta`. |

#### Subcommands NOT worth testing via integration

`cmd_update`, `cmd_uninstall`, `detect_install_method` — these mutate the system binary, download from GitHub, or detect install paths. Their UNTESTED assertions should stay UNTESTED.

#### Total new CLI integration tests: ~30

---

### 3e. Other Module Gaps (Lower Priority)

| Module | Assertion | Test approach |
|---|---|---|
| config (SPEC-CF-DC-009) | stops-at-filesystem-root | Impractical — leave UNTESTED |
| config (SPEC-CF-PC-020) | provider-api-base-env-vars | Set env var, parse config with `${VAR}` in api_base |
| history (SPEC-HI-QR-011/012, QA-006/007) | SQL prepare/row-read failures | Would require corrupting the schema between init and query — fragile. Leave UNTESTED. |
| history (SPEC-HI-SV-010/011) | Insert failures | Same — leave UNTESTED |
| history (SPEC-HI-ID-001) | Open connection failure | Pass an invalid path (e.g., `/dev/null/impossible`). Check for error. |

---

## 4. Remaining Structural Work for Testability

*Section 4a (Extract `check_single_provider` HTTP logic) from the previous plan is DONE — this is the `provider.rs` extraction.*

### 4a. MockRuntimeAdapter enhancements

Add to `test_helpers.rs`:

```rust
impl MockRuntimeAdapter {
    /// poll_status returns Err after `polls_before_error` successful polls.
    pub fn with_poll_error(mut self, msg: &str) -> Self { ... }
}
```

This fills the gap for SPEC-EX-DS-004 (poll error → cancel + teardown).

### 4b. cmd_update / cmd_uninstall — Leave UNTESTED

These commands modify the system binary, download from GitHub, or run `cargo uninstall`. Testing them in CI risks:
- Mutating the test environment
- Requiring network access to GitHub
- Platform-specific behavior (Unix self-delete vs. Windows rename)

The assertions should remain UNTESTED with a note in the spec file explaining why. These commands are covered by manual QA during releases.

---

## 5. Implementation Order

Priority is based on: (1) risk of real bugs, (2) number of assertions covered, (3) effort.

### Wave 1: httpmock + provider.rs HTTP tests (~14 tests)

1. Add `httpmock = "0.8"` to `[dev-dependencies]`
2. Write `test_client` helper in provider.rs
3. Implement provider-level HTTP tests: `post_completion` error paths, `list_models` full coverage, `test_completion` full coverage, `send_request` timeout/unreachable

**Why first:** These tests are the fastest to write (no pipeline setup needed — just a `Provider` struct and a mock server) and they validate the new extraction. They'll immediately catch any regression from the refactor and fill 14 UNTESTED assertions. They also establish the httpmock patterns that the rest of the plan reuses.

### Wave 2: openhands adapter HTTP tests (~20 tests)

1. Write the `test_adapter` helper in `runtime/openhands.rs`
2. Implement all openhands HTTP tests (health_check, create_session, poll_status, collect_result, cancel, teardown)

**Why second:** Highest-risk untested code. The OpenHands adapter makes real HTTP calls with multipart uploads, JSON parsing, and error mapping — all with zero test coverage. Now that the httpmock patterns are established from Wave 1, these tests follow the same shape.

### Wave 3: exec.rs gaps + MockRuntimeAdapter enhancement (~5 tests)

1. Add `with_poll_error()` to MockRuntimeAdapter
2. Write exec.rs tests for remaining exec-owned gaps (prompt-file-resolution, poll error handling, api-key-env error wrapping)

### Wave 4: CLI integration tests (~30 tests)

1. cmd_check edge cases (context errors, suppress flags, unknown format, stdin cleanup)
2. cmd_init variations (minimal, prompts-only, no-overwrite)
3. cmd_list, cmd_history, cmd_validate_config, cmd_version, cmd_clean gaps
4. cmd_check_provider and cmd_check_runtime via httpmock-as-subprocess

### Wave 5: Migrate existing raw TCP tests to httpmock (~15 tests)

1. Replace `start_mock_server` calls one by one in exec.rs
2. Remove the raw TCP helper and imports
3. Verify no test regressions

---

## 6. Expected Final Coverage

### Assertion counts by module (post-extraction)

| Module | Total assertions | Tested | UNTESTED | IMPLICIT | Permanently UNTESTED |
|---|---|---|---|---|---|
| provider.rs (NEW) | 44 | 16 → 30 | 14 → 0 | 14 | 0 |
| runtime/openhands.rs | ~35 | ~5 → ~25 | ~30 → ~10 | ~5 | ~5 (env var, client build) |
| exec.rs | ~75 | ~60 → ~65 | ~10 → ~5 | ~5 | ~2 (timing) |
| main.rs (CLI) | 111 | 34 → ~64 | 67 → ~37 | 14 | ~25 (update/uninstall/detect) |
| config.rs | ~50 | ~45 | ~3 | ~2 | ~2 (filesystem root, env var in api_base) |
| history.rs | ~35 | ~28 | ~7 | ~2 | ~7 (SQL error injection) |
| types.rs | ~25 | ~23 | ~1 | ~1 | 0 |
| Other (prompt, placeholder, verdict_parser) | ~60 | ~55 | ~3 | ~2 | ~2 |

### Summary

| Category | Before | After |
|---|---|---|
| provider.rs HTTP-level | — (didn't exist) | 30 tested |
| openhands.rs HTTP-level | 0 tested | ~20 tested |
| exec.rs UNTESTED | ~10 gaps | ~2 remaining (timing) |
| main.rs / CLI | 34 tested, 67 UNTESTED | ~64 tested, ~37 UNTESTED |
| Permanently UNTESTED | — | ~40 |
| **Total assertions with tests** | **~265** | **~345** |
| **New tests written** | — | **~70** |

The permanently UNTESTED assertions fall into four categories:
1. **System-mutating** (cmd_update, cmd_uninstall, detect_install_method): ~16 assertions
2. **Requires error injection** (SQL prepare failures, client build failures): ~10 assertions
3. **Impractical** (filesystem root traversal, timing assertions): ~4 assertions
4. **Environment/platform dependent** (env var edge cases in edge-case paths): ~10 assertions

---

## 7. Dependency Summary

**New dev-dependency:**
```toml
[dev-dependencies]
httpmock = "0.8"
```

**Optionally useful (for cmd_clean timestamp tests):**
```toml
[dev-dependencies]
filetime = "0.2"
```

No new runtime dependencies. No async runtime needed. No changes to the production code path.
