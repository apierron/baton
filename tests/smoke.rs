//! Integration smoke tests for runtime adapters.
//!
//! These tests call real LLM runtimes and are `#[ignore]`d by default.
//! Run them explicitly with:
//!
//!   cargo test --test smoke -- --ignored --nocapture
//!
//! Configure the runtime via environment variables:
//!
//! | Variable                | Default        | Description                    |
//! |-------------------------|----------------|--------------------------------|
//! | `BATON_SMOKE_RUNTIME`   | `claude-code`  | Runtime type                   |
//! | `BATON_SMOKE_BASE_URL`  | `claude`       | Binary path or API URL         |
//! | `BATON_SMOKE_MODEL`     | `sonnet`       | Model name                     |
//! | `BATON_SMOKE_API_KEY_ENV` | *(empty)*    | Env var name holding API key   |
//! | `BATON_SMOKE_TIMEOUT`   | `60`           | Timeout in seconds             |

mod common;

use common::{baton, parse_verdict, setup_project};

// ─── Helpers ─────────────────────────────────────────────

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

/// Generates `[runtimes.smoke]` TOML from environment variables.
fn smoke_runtime_toml() -> String {
    let runtime_type = env_or("BATON_SMOKE_RUNTIME", "claude-code");
    let base_url = env_or("BATON_SMOKE_BASE_URL", "claude");
    let model = env_or("BATON_SMOKE_MODEL", "sonnet");
    let api_key_env = env_or("BATON_SMOKE_API_KEY_ENV", "");
    let timeout = env_or("BATON_SMOKE_TIMEOUT", "60");

    let mut toml = format!(
        r#"[runtimes.smoke]
type = "{runtime_type}"
base_url = "{base_url}"
default_model = "{model}"
timeout_seconds = {timeout}
max_iterations = 5
"#
    );

    if !api_key_env.is_empty() {
        toml.push_str(&format!("api_key_env = \"{api_key_env}\"\n"));
    }

    toml
}

/// Generates a smoke LLM validator TOML block.
fn smoke_llm_validator(name: &str, prompt: &str) -> String {
    let model = env_or("BATON_SMOKE_MODEL", "sonnet");
    format!(
        r#"[[gates.smoke.validators]]
name = "{name}"
type = "llm"
prompt = "{prompt}"
runtime = ["smoke"]
model = "{model}"
"#
    )
}

/// Builds a complete baton.toml for smoke tests.
fn smoke_config(validators: &str) -> String {
    let timeout = env_or("BATON_SMOKE_TIMEOUT", "60");
    let runtime = smoke_runtime_toml();
    format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = {timeout}
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

{runtime}

[gates.smoke]
{validators}
"#
    )
}

// ─── Smoke tests ─────────────────────────────────────────

#[test]
#[ignore]
fn smoke_llm_query_pass() {
    let validators = smoke_llm_validator(
        "pass-check",
        "You are a validator. Respond with exactly the word PASS and nothing else.",
    );
    let toml = smoke_config(&validators);
    let dir = setup_project(&toml, "hello world");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "smoke_llm_query_pass failed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    let verdict = parse_verdict(&stdout);
    assert_eq!(
        verdict["status"], "pass",
        "Expected pass verdict.\nFull verdict: {verdict:#}"
    );

    // Verify cost metadata is present (proves response was parsed correctly)
    let history = verdict["history"]
        .as_array()
        .expect("history should be array");
    assert!(!history.is_empty(), "history should not be empty");
    let result = &history[0];
    assert_eq!(result["name"], "pass-check");
    assert_eq!(result["status"], "pass");
    assert!(
        result["cost"].is_object(),
        "cost metadata should be present: {result:#}"
    );
}

#[test]
#[ignore]
fn smoke_llm_query_fail() {
    let validators = smoke_llm_validator(
        "fail-check",
        "You are a validator. Respond with exactly: FAIL: smoke test failure detected",
    );
    let toml = smoke_config(&validators);
    let dir = setup_project(&toml, "hello world");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // baton exits with code 1 on fail verdict
    assert!(
        !output.status.success(),
        "smoke_llm_query_fail should have failed.\nstdout: {stdout}\nstderr: {stderr}"
    );

    let verdict = parse_verdict(&stdout);
    assert_eq!(
        verdict["status"], "fail",
        "Expected fail verdict.\nFull verdict: {verdict:#}"
    );

    let history = verdict["history"]
        .as_array()
        .expect("history should be array");
    assert!(!history.is_empty(), "history should not be empty");
    let result = &history[0];
    assert_eq!(result["name"], "fail-check");
    assert_eq!(result["status"], "fail");
    // Feedback should contain the failure reason
    let feedback = result["feedback"].as_str().unwrap_or("");
    assert!(
        !feedback.is_empty(),
        "feedback should be present for a FAIL verdict: {result:#}"
    );
}
