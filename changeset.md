# Changeset: provider.rs Extraction

## Overview

New file: `src/provider.rs` — HTTP client for OpenAI-compatible LLM providers.
New file: `spec/provider.md` — Spec assertions for the provider module.

This document describes the exact changes needed in existing files to integrate the new module.

---

## 1. src/lib.rs

Add the new module declaration. Insert after the `pub mod prompt;` line:

```rust
pub mod provider;
```

The full module list becomes:

```rust
pub mod config;
pub mod error;
pub mod exec;
pub mod history;
pub mod placeholder;
pub mod prompt;
pub mod provider;
pub mod runtime;
pub mod types;
pub mod verdict_parser;
```

Also update the module doc comment to include provider:

```rust
//! - [`provider`] — HTTP client for OpenAI-compatible LLM provider APIs
```

---

## 2. src/exec.rs

### 2a. Remove `extract_cost` function

Delete the `extract_cost` function and its imports (it now lives in `provider.rs`):

```rust
// DELETE this function:
fn extract_cost(resp_body: &serde_json::Value, model: &str) -> Option<Cost> {
    let usage = resp_body.get("usage")?;
    // ... entire function body ...
}
```

### 2b. Add import

At the top of exec.rs, add:

```rust
use crate::provider::{self, ProviderClient, ProviderError};
```

### 2c. Rewrite `execute_llm_completion`

Replace the entire `execute_llm_completion` function with:

```rust
fn execute_llm_completion(
    validator: &ValidatorConfig,
    artifact: &mut Artifact,
    context: &mut Context,
    prior_results: &BTreeMap<String, ValidatorResult>,
    config: Option<&BatonConfig>,
) -> ValidatorResult {
    let config = match config {
        Some(c) => c,
        None => {
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some(
                    "[baton] LLM validator requires config with provider settings".into(),
                ),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    // Resolve provider
    let provider = match config.providers.get(&validator.provider) {
        Some(p) => p,
        None => {
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some(format!(
                    "[baton] Provider '{}' is not defined in [providers].",
                    validator.provider
                )),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    // Build provider client
    let client = match ProviderClient::new(provider, &validator.provider, validator.timeout_seconds)
    {
        Ok(c) => c,
        Err(e) => {
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some(format!("[baton] {e}")),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    // Resolve prompt
    let prompt_value = match &validator.prompt {
        Some(p) => p.clone(),
        None => {
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some("[baton] LLM validator missing prompt".into()),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    let prompt_body = if is_file_reference(&prompt_value) {
        match resolve_prompt_value(
            &prompt_value,
            &config.defaults.prompts_dir,
            &config.config_dir,
        ) {
            Ok(template) => template.body,
            Err(e) => {
                return ValidatorResult {
                    name: validator.name.clone(),
                    status: Status::Error,
                    feedback: Some(format!("[baton] {e}")),
                    duration_ms: 0,
                    cost: None,
                };
            }
        }
    } else {
        prompt_value
    };

    // Resolve placeholders in prompt
    let mut warnings = ResolutionWarnings::new();
    let rendered_prompt = resolve_placeholders(
        &prompt_body,
        artifact,
        context,
        prior_results,
        &mut warnings,
    );

    // Build model name
    let model = validator
        .model
        .clone()
        .unwrap_or_else(|| provider.default_model.clone());

    // Build messages
    let mut messages = Vec::new();

    if let Some(ref sys) = validator.system_prompt {
        let rendered_sys =
            resolve_placeholders(sys, artifact, context, prior_results, &mut warnings);
        messages.push(serde_json::json!({
            "role": "system",
            "content": rendered_sys,
        }));
    }

    messages.push(serde_json::json!({
        "role": "user",
        "content": rendered_prompt,
    }));

    // Build request body
    let mut request_body = serde_json::json!({
        "model": model,
        "messages": messages,
        "temperature": validator.temperature,
    });

    if let Some(max_tokens) = validator.max_tokens {
        request_body["max_tokens"] = serde_json::json!(max_tokens);
    }

    // Send completion via provider client
    match client.post_completion(request_body, &model) {
        Ok(response) => {
            // Parse verdict from content
            match validator.response_format {
                ResponseFormat::Verdict => {
                    let parsed = parse_verdict(&response.content);
                    ValidatorResult {
                        name: validator.name.clone(),
                        status: parsed.status,
                        feedback: parsed.evidence,
                        duration_ms: 0,
                        cost: response.cost,
                    }
                }
                ResponseFormat::Freeform => ValidatorResult {
                    name: validator.name.clone(),
                    status: Status::Warn,
                    feedback: Some(response.content),
                    duration_ms: 0,
                    cost: response.cost,
                },
            }
        }
        Err(ProviderError::EmptyContent { cost }) => ValidatorResult {
            name: validator.name.clone(),
            status: Status::Error,
            feedback: Some("[baton] Provider returned empty or malformed response.".into()),
            duration_ms: 0,
            cost,
        },
        Err(e) => ValidatorResult {
            name: validator.name.clone(),
            status: Status::Error,
            feedback: Some(format!("[baton] {e}")),
            duration_ms: 0,
            cost: None,
        },
    }
}
```

### 2d. Update `extract_cost` test references

In the `#[cfg(test)] mod tests` block, the `extract_cost` tests need to call `provider::extract_cost` instead. Replace:

```rust
// Old:
use ... (existing imports)

// Add to test imports:
use crate::provider::extract_cost;
```

The test function bodies stay the same — they call `extract_cost(...)` which now resolves to the re-imported function.

### 2e. Remove TCP imports from LLM section

The `use std::io::{Read, Write};` and `use std::net::TcpListener;` imports and the `start_mock_server` function remain for now (they're used by existing tests). They'll be migrated in the httpmock wave later.

---

## 3. src/main.rs

### 3a. Add import

Add to the use block at the top:

```rust
use baton::provider::{ProviderClient, ProviderError};
```

### 3b. Rewrite `check_single_provider`

Replace the entire function with:

```rust
fn check_single_provider(name: &str, provider: &baton::config::Provider) -> bool {
    // 1. Build provider client
    let client = match ProviderClient::new(provider, name, 10) {
        Ok(c) => c,
        Err(ProviderError::ApiKeyNotSet { env_var, .. }) => {
            eprintln!("  ERROR: API key env var '{env_var}' is not set");
            return false;
        }
        Err(e) => {
            eprintln!("  ERROR: {e}");
            return false;
        }
    };

    // 2. Try /v1/models endpoint
    match client.list_models() {
        Ok(models) => {
            if models.iter().any(|m| m == &provider.default_model) {
                eprintln!(
                    "  OK: Provider '{name}': reachable, model '{}' available",
                    provider.default_model
                );
                return true;
            } else if models.is_empty() {
                // Model list came back empty — fall through to test completion
            } else {
                eprintln!(
                    "  WARN: Provider '{name}': reachable, but model '{}' not found",
                    provider.default_model
                );
                let display: Vec<&str> = models.iter().take(10).map(|s| s.as_str()).collect();
                eprintln!("  Available models: {}", display.join(", "));
                return true; // reachable, just model not found
            }
        }
        Err(ProviderError::AuthFailed { api_key_env, .. }) => {
            eprintln!(
                "  ERROR: Authentication failed for provider '{name}'. Check {api_key_env}."
            );
            return false;
        }
        Err(ProviderError::Timeout { .. }) => {
            eprintln!(
                "  ERROR: Provider '{name}': connection timed out to {}",
                provider.api_base
            );
            return false;
        }
        Err(ProviderError::Unreachable { api_base, detail, .. }) => {
            eprintln!("  ERROR: Cannot reach {api_base}: {detail}");
            return false;
        }
        Err(_) => {
            // /v1/models not available — fall through to test completion
        }
    }

    // 3. Fallback: minimal test completion
    eprintln!("  WARN: Model list not available. Attempting test completion...");
    match client.test_completion(&provider.default_model) {
        Ok(true) => {
            eprintln!(
                "  OK: Provider '{name}': reachable, model '{}' responds",
                provider.default_model
            );
            true
        }
        Err(e) => {
            eprintln!("  ERROR: Provider '{name}': {e}");
            false
        }
        Ok(false) => unreachable!("test_completion returns Ok(true) or Err"),
    }
}
```

### 3c. Remove unused imports

After the rewrite, `check_single_provider` no longer directly uses `reqwest`. Remove these if they become unused:
- `reqwest::blocking::Client` (check if `cmd_update` still needs it — it does, so `reqwest` stays as a dependency, but you may be able to remove specific imports from the `check_single_provider` scope)

The `reqwest` imports for `cmd_update` remain unchanged.

### 3d. Remove the `ValidatorTypeStr` trait

This trait and its impl block are unrelated to the provider extraction and stay unchanged.

---

## 4. docs/ARCHITECTURE.md

### 4a. Update dependency diagram

Replace the ASCII diagram with:

```text
          ┌───────────┐
          │  main.rs  │  CLI entry point (clap)
          └─────┬─────┘
                │ uses
    ┌───────┬───┼───────┐
    │       │   │       │
    ▼       ▼   ▼       ▼
  exec   config history runtime
    │       │           │
    ├───────┤           │
    │       │     ┌─────┘
    ▼       ▼     │
placeholder prompt│
    │             │
    ▼             ▼
  types ◄──── verdict_parser
    │
    ▼
  error
```

becomes:

```text
          ┌───────────┐
          │  main.rs  │  CLI entry point (clap)
          └─────┬─────┘
                │ uses
    ┌───────┬───┼───────┬──────────┐
    │       │   │       │          │
    ▼       ▼   ▼       ▼          ▼
  exec   config history runtime  provider
    │       │                      │
    ├───────┤──────────────────────┘
    │       │     
    ▼       ▼     
placeholder prompt
    │             
    ▼             
  types ◄──── verdict_parser
    │
    ▼
  error
```

### 4b. Update dependency table

Add row for `provider`:

```
| `provider` | `config` (for `Provider` struct), `types` (for `Cost`) |
```

Update `exec` row:

```
| `exec` | `config`, `types`, `placeholder`, `runtime`, `provider`, `error` |
```

Update `main.rs` row:

```
| `main.rs` | `config`, `exec`, `history`, `runtime`, `provider`, `types` |
```

### 4c. Update LLM Validators section

Replace:

> **Completion** (`exec.rs: execute_llm_completion`) — Sends a single HTTP POST to an OpenAI-compatible `/v1/chat/completions` endpoint. The prompt template is resolved with placeholders, the response is parsed by `verdict_parser` for PASS/FAIL/WARN keywords. Token counts and cost are tracked in `ValidatorResult.cost`.

With:

> **Completion** (`exec.rs: execute_llm_completion`) — Resolves the prompt template and placeholders, then delegates the HTTP call to `provider::ProviderClient::post_completion()`. The response content is parsed by `verdict_parser` for PASS/FAIL/WARN keywords. Token counts and cost are extracted by the provider client and tracked in `ValidatorResult.cost`.

### 4d. Add Provider Client section

After the "Runtime Adapters" section, add:

```markdown
## Provider Client

The `provider` module provides `ProviderClient`, a shared HTTP client for OpenAI-compatible LLM APIs. It handles API key resolution, Bearer auth, and structured error classification (auth failures, model-not-found, rate limiting, timeouts). Both `exec::execute_llm_completion` and the CLI's `check-provider` command use it.

Unlike `RuntimeAdapter` (a trait for pluggable backends), `ProviderClient` is a concrete struct — all supported LLM providers use the OpenAI-compatible API format. If a non-OpenAI-compatible provider is added, the client can be extended or a trait can be extracted at that point.
```

---

## 5. AGENTS.md

### 5a. Update Architecture section

In the module dependency layers block, add `provider` to the main.rs line and add a new line:

```text
main.rs → exec, config, history, runtime, provider, types
exec → config, types, placeholder, runtime, provider, error
provider → config, types
```

### 5b. Update Spec Files table

Add to the spec files list:

```
Files: `types.md`, `config.md`, `prompt.md`, `placeholder.md`, `verdict_parser.md`, `exec.md`, `history.md`, `runtime.md`, `provider.md`, `main.md`
```

---

## 6. spec/exec.md

### 6a. Update execute_llm_completion section header

Add a note that HTTP transport is now delegated:

> Resolves provider and prompt, builds the request body, delegates the HTTP call to `provider::ProviderClient::post_completion()`, and maps the response or error to a `ValidatorResult`.

### 6b. Update affected assertions

SPEC-EX-LC-001 through LC-002 — unchanged (config/provider resolution still in exec.rs).

SPEC-EX-LC-003 — update description:
```
SPEC-EX-LC-003: api-key-env-not-set-errors
  If ProviderClient::new returns ApiKeyNotSet, returns Status::Error with the formatted error.
  test: UNTESTED (would require env var manipulation during test)
```

SPEC-EX-LC-011 through LC-016 — update to reference provider module:
```
  HTTP error classification is performed by ProviderClient::classify_http_error.
  The exec module maps ProviderError variants to "[baton]" prefixed feedback strings.
```

### 6c. Remove extract_cost section from exec.md

The `extract_cost` assertions move to `spec/provider.md`. Remove the extract_cost section from spec/exec.md and add a note:

```
Note: extract_cost has been moved to the provider module. See spec/provider.md SPEC-PV-EC-*.
```

---

## 7. spec/main.md

### 7a. Update check_single_provider section

Replace the intro paragraph:

> Tests connectivity to a single LLM provider. Uses `ProviderClient` from the provider module for all HTTP interactions.

Update assertions SPEC-MN-SP-001 through SP-007 to reference ProviderClient:

```
SPEC-MN-SP-001: missing-api-key-returns-false
  If ProviderClient::new returns ApiKeyNotSet, prints the env var error and returns false.
  test: UNTESTED

SPEC-MN-SP-002: empty-api-key-env-skips-key-check
  Handled by ProviderClient::new — empty api_key_env means no auth.
  test: UNTESTED

SPEC-MN-SP-003: models-endpoint-auth-failure
  If list_models returns AuthFailed, prints "Authentication failed" and returns false.
  test: UNTESTED

SPEC-MN-SP-004: models-endpoint-timeout
  If list_models returns Timeout, prints "connection timed out" and returns false.
  test: UNTESTED

SPEC-MN-SP-005: model-found-in-list
  If list_models succeeds and the default model is in the list, prints "OK" and returns true.
  test: UNTESTED

SPEC-MN-SP-006: model-not-found-in-list
  If list_models succeeds but the model is absent, prints "WARN" with available models. Returns true.
  test: UNTESTED

SPEC-MN-SP-007: fallback-test-completion
  If list_models returns a non-auth/non-connectivity error, falls through to test_completion.
  test: UNTESTED
```

---

## Test Migration Notes

The `extract_cost` tests in exec.rs can either:
1. **Stay in exec.rs** with `use crate::provider::extract_cost;` — simplest, avoids touching test structure
2. **Move to provider.rs** — cleaner ownership, but changes test counts

Recommendation: **option 2** (move to provider.rs) since the function now lives there. The four `extract_cost` tests in exec.rs are replaced by the identical four in provider.rs. The exec.rs test count drops by 4, provider.rs gains 16 (4 extract_cost + 12 existing).

All existing LLM completion tests in exec.rs continue to work unchanged — they exercise `execute_llm_completion` end-to-end through the raw TCP mock, which still works because `ProviderClient::post_completion` makes a real HTTP call to whatever URL is in `provider.api_base`.
