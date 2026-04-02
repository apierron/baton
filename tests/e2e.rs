mod common;
use common::*;

use std::fs;

// ─── Blocking / Non-blocking Error Recovery ─────────────────

#[test]
fn e2e_blocking_nonblocking_error_recovery() {
    // v1: pass (blocking), v2: fail (non-blocking), v3: pass (blocking),
    // v4: fail (blocking — stops here), v5: pass (never runs)
    let validators = [
        script_validator_blocking_for("review", "v1", "echo PASS", true),
        script_validator_blocking_for("review", "v2", "exit 1", false),
        script_validator_blocking_for("review", "v3", "echo PASS", true),
        script_validator_blocking_for("review", "v4", "exit 1", true),
        script_validator_blocking_for("review", "v5", "echo PASS", true),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "fail");
    assert_eq!(verdict["failed_at"], "v4");

    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history.len(), 4, "v5 should not run");

    // v1 passed
    assert_eq!(history[0]["name"], "v1");
    assert_eq!(history[0]["status"], "pass");
    // v2 failed but non-blocking, pipeline continued
    assert_eq!(history[1]["name"], "v2");
    assert_eq!(history[1]["status"], "fail");
    // v3 passed after non-blocking failure
    assert_eq!(history[2]["name"], "v3");
    assert_eq!(history[2]["status"], "pass");
    // v4 failed and is blocking — stopped pipeline
    assert_eq!(history[3]["name"], "v4");
    assert_eq!(history[3]["status"], "fail");
    // v5 not present
    assert!(!history.iter().any(|v| v["name"] == "v5"));
}

// ─── run_if Chain ───────────────────────────────────────────

#[test]
fn e2e_run_if_chain_all_pass() {
    let validators = [
        script_validator_blocking_for("review", "v1", "echo PASS", true),
        script_validator_blocking_for("review", "v2", "echo PASS", false),
        script_validator_with_run_if(
            "review",
            "v3",
            "echo PASS",
            "v1.status == pass and v2.status == pass",
        ),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(history[2]["name"], "v3");
    assert_eq!(history[2]["status"], "pass");
}

#[test]
fn e2e_run_if_chain_skips_on_failure() {
    let validators = [
        script_validator_blocking_for("review", "v1", "echo PASS", true),
        script_validator_blocking_for("review", "v2", "exit 1", false),
        script_validator_with_run_if(
            "review",
            "v3",
            "echo PASS",
            "v1.status == pass and v2.status == pass",
        ),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // v2 is non-blocking, so gate still passes even though v2 failed
    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history.len(), 3);
    // v3 should be skipped because run_if evaluates to false
    let v3 = history.iter().find(|v| v["name"] == "v3").unwrap();
    assert_eq!(v3["status"], "skip");
}

// ─── History Round-Trip ─────────────────────────────────────

#[test]
fn e2e_history_round_trip() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    // First check — writes to history
    let output = baton()
        .current_dir(dir.path())
        .args(["check"])
        .output()
        .unwrap();
    assert!(output.status.success());

    // Query history — should have 1 entry
    let output = baton()
        .current_dir(dir.path())
        .args(["history", "--gate", "review"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pass"),
        "History should show pass verdict: {stdout}"
    );

    // Second check
    let output = baton()
        .current_dir(dir.path())
        .args(["check"])
        .output()
        .unwrap();
    assert!(output.status.success());

    // Query with limit 2 — should have 2 entries
    let output = baton()
        .current_dir(dir.path())
        .args(["history", "--gate", "review", "--limit", "2"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Count lines containing "pass" to verify 2 entries
    let pass_count = stdout.lines().filter(|l| l.contains("pass")).count();
    assert!(
        pass_count >= 2,
        "Should have 2 pass entries, got {pass_count}: {stdout}"
    );

    // Filter by fail — should show no verdicts
    let output = baton()
        .current_dir(dir.path())
        .args(["history", "--gate", "review", "--status", "fail"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("No verdicts")
            || combined.contains("no verdicts")
            || combined.contains("0 verdicts"),
        "Should indicate no fail verdicts: {combined}"
    );
}

// ─── Validate → List → Check Workflow ───────────────────────

#[test]
fn e2e_validate_list_check_workflow() {
    let toml = multi_gate_toml(&[
        ("alpha", &script_validator_for("alpha", "lint", "echo PASS")),
        ("beta", &script_validator_for("beta", "test", "echo PASS")),
    ]);
    let dir = setup_project(&toml, "hello");

    // Step 1: doctor (config validation)
    fs::create_dir_all(dir.path().join("prompts")).unwrap();
    let output = baton()
        .current_dir(dir.path())
        .args(["doctor", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "doctor should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Step 2: list all gates
    let output = baton()
        .current_dir(dir.path())
        .args(["list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let list_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(list_output.contains("alpha"), "list should show alpha gate");
    assert!(list_output.contains("beta"), "list should show beta gate");

    // Step 3: list specific gate
    let output = baton()
        .current_dir(dir.path())
        .args(["list", "--gate", "alpha"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let list_output = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        list_output.contains("lint"),
        "list --gate alpha should show lint validator"
    );

    // Step 4: check
    let output = baton()
        .current_dir(dir.path())
        .args(["check", "--no-log"])
        .output()
        .unwrap();
    assert!(output.status.success());
}

// ─── Init → Customize → Check ──────────────────────────────

#[test]
fn e2e_init_customize_check() {
    let dir = tempfile::TempDir::new().unwrap();

    // Step 1: init
    let output = baton()
        .current_dir(dir.path())
        .args(["init"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Step 2: overwrite baton.toml with a working script validator
    let toml = minimal_toml("review", &script_validator("check", "echo PASS"));
    fs::write(dir.path().join("baton.toml"), &toml).unwrap();
    fs::write(dir.path().join("artifact.txt"), "test content").unwrap();

    // Step 3: check
    let output = baton()
        .current_dir(dir.path())
        .args(["check", "--no-log"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Warn Exit Codes & Suppression ──────────────────────────

#[test]
fn e2e_warn_exit_codes_and_suppression() {
    let validators = [
        script_validator_with_warn_codes("review", "linter", "exit 3", &[3]),
        script_validator_for("review", "after-lint", "echo PASS"),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    // Without suppression — warn doesn't block, gate passes
    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    let linter = history.iter().find(|v| v["name"] == "linter").unwrap();
    assert_eq!(linter["status"], "warn");
    let after = history.iter().find(|v| v["name"] == "after-lint").unwrap();
    assert_eq!(after["status"], "pass");
    let warnings = verdict["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w.as_str() == Some("linter")),
        "warnings should contain linter"
    );

    // With --suppress-warnings
    let output = baton()
        .args(["check", "--no-log", "--suppress-warnings"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let suppressed = verdict["suppressed"].as_array().unwrap();
    assert!(
        !suppressed.is_empty(),
        "suppressed array should be non-empty"
    );
}

// ─── Suppress-All Unblocks Pipeline ─────────────────────────

#[test]
fn e2e_suppress_all_unblocks_pipeline() {
    let validators = [
        script_validator_blocking_for("review", "v1", "exit 1", true),
        script_validator_blocking_for("review", "v2", "echo PASS", true),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    // Without suppress-all — v1 blocks, v2 never runs
    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "fail");
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history.len(), 1, "Only v1 should run without suppress-all");

    // With --suppress-all — both run, gate passes
    let output = baton()
        .args(["check", "--no-log", "--suppress-all"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(
        history.len(),
        2,
        "Both validators should run with suppress-all"
    );
}

// ─── Multi-Gate Execution ───────────────────────────────────

#[test]
fn e2e_multi_gate_execution() {
    let toml = multi_gate_toml(&[
        ("alpha", &script_validator_for("alpha", "lint", "echo PASS")),
        ("beta", &script_validator_for("beta", "test", "echo PASS")),
    ]);
    let dir = setup_project(&toml, "hello");

    // Run all gates
    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Multi-gate output prints pretty-printed JSON objects consecutively.
    // Use a streaming deserializer to parse them.
    let stream = serde_json::Deserializer::from_str(&stdout).into_iter::<serde_json::Value>();
    let verdicts: Vec<serde_json::Value> = stream
        .map(|r| r.expect("Failed to parse verdict"))
        .collect();
    assert_eq!(verdicts.len(), 2, "Should have 2 verdict objects");
    let gates: Vec<&str> = verdicts
        .iter()
        .map(|v| v["gate"].as_str().unwrap())
        .collect();
    assert!(gates.contains(&"alpha"));
    assert!(gates.contains(&"beta"));
    for v in &verdicts {
        assert_eq!(v["status"], "pass");
    }

    // With --only alpha
    let output = baton()
        .args(["check", "--no-log", "--only", "alpha"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict = parse_verdict(stdout.trim());
    assert_eq!(verdict["gate"], "alpha");
}

// ─── LLM Validator Tests (httpmock) ─────────────────────────

#[test]
fn e2e_llm_validator_pass() {
    let server = httpmock::MockServer::start();

    // Mock health check
    server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{"id": "test-model"}]
        }));
    });

    // Mock completion
    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/chat/completions");
        then.status(200).json_body(serde_json::json!({
            "choices": [{"message": {"content": "PASS"}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20}
        }));
    });

    let validators = llm_validator("review", "ai-review", "Review this code", "default");
    let runtime = runtime_toml("default", &server.url(""));
    let toml = format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

{runtime}

[gates.review]
{validators}
"#
    );
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "LLM pass test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");

    let history = verdict["history"].as_array().unwrap();
    let ai_result = &history[0];
    assert_eq!(ai_result["name"], "ai-review");
    assert_eq!(ai_result["status"], "pass");
    // Cost metadata should be present
    assert!(ai_result["cost"].is_object(), "cost should be present");
    assert_eq!(ai_result["cost"]["input_tokens"], 100);
    assert_eq!(ai_result["cost"]["output_tokens"], 20);
}

#[test]
fn e2e_llm_validator_fail_with_feedback() {
    let server = httpmock::MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{"id": "test-model"}]
        }));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/chat/completions");
        then.status(200).json_body(serde_json::json!({
            "choices": [{"message": {"content": "FAIL: missing error handling in edge case"}}],
            "usage": {"prompt_tokens": 150, "completion_tokens": 30}
        }));
    });

    let validators = llm_validator("review", "ai-review", "Review this code", "default");
    let runtime = runtime_toml("default", &server.url(""));
    let toml = format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

{runtime}

[gates.review]
{validators}
"#
    );
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "fail");
    assert_eq!(verdict["failed_at"], "ai-review");

    let history = verdict["history"].as_array().unwrap();
    let ai_result = &history[0];
    assert_eq!(ai_result["status"], "fail");
    let feedback = ai_result["feedback"].as_str().unwrap();
    assert!(
        feedback.contains("missing error handling"),
        "Feedback should contain LLM evidence: {feedback}"
    );
}

#[test]
fn e2e_llm_validator_system_prompt() {
    let server = httpmock::MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{"id": "test-model"}]
        }));
    });

    // Mock completion — accept any request and verify it was called
    let system_mock = server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/chat/completions");
        then.status(200).json_body(serde_json::json!({
            "choices": [{"message": {"content": "PASS"}}],
            "usage": {"prompt_tokens": 200, "completion_tokens": 10}
        }));
    });

    let validators = llm_validator_with_system_prompt(
        "review",
        "ai-review",
        "Check this code",
        "You are a strict code reviewer",
        "default",
    );
    let runtime = runtime_toml("default", &server.url(""));
    let toml = format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

{runtime}

[gates.review]
{validators}
"#
    );
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "System prompt test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the mock matched (system + user roles were present in request)
    system_mock.assert();
}

// ─── Full Project Lifecycle ────────────────────────────────

#[test]
fn e2e_full_lifecycle_init_add_doctor_check_history() {
    let dir = tempfile::TempDir::new().unwrap();

    // Step 1: init
    let output = baton()
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Step 2: overwrite with a working config that has a script validator
    let toml = minimal_toml("review", &script_validator_for("review", "lint", "echo PASS"));
    fs::write(dir.path().join("baton.toml"), &toml).unwrap();
    fs::write(dir.path().join("artifact.txt"), "test").unwrap();

    // Step 3: doctor
    fs::create_dir_all(dir.path().join("prompts")).unwrap();
    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Step 4: check (with logging to write history)
    let output = baton()
        .args(["check"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Step 5: history should show the pass
    let output = baton()
        .args(["history", "--gate", "review"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pass"),
        "History should record pass: {stdout}"
    );
}

// ─── Multi-Gate Blocking Independence ──────────────────────

#[test]
fn e2e_multi_gate_first_fails_second_passes() {
    let toml = multi_gate_toml(&[
        (
            "alpha",
            &script_validator_blocking_for("alpha", "blocker", "exit 1", true),
        ),
        (
            "beta",
            &script_validator_for("beta", "checker", "echo PASS"),
        ),
    ]);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Overall exit code should be 1 (at least one gate failed)
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stream = serde_json::Deserializer::from_str(&stdout).into_iter::<serde_json::Value>();
    let verdicts: Vec<serde_json::Value> = stream
        .map(|r| r.expect("Failed to parse verdict"))
        .collect();
    assert_eq!(verdicts.len(), 2, "Should have 2 verdict objects");

    let alpha = verdicts.iter().find(|v| v["gate"] == "alpha").unwrap();
    assert_eq!(alpha["status"], "fail");

    let beta = verdicts.iter().find(|v| v["gate"] == "beta").unwrap();
    assert_eq!(beta["status"], "pass");
}

// ─── Human Validator in Mixed Pipeline ─────────────────────

#[test]
fn e2e_human_in_mixed_pipeline() {
    let validators = [
        script_validator_blocking_for("review", "pre-check", "echo PASS", true),
        human_validator_blocking("review", "manual-review", "Review this code", false),
        script_validator_blocking_for("review", "post-check", "echo PASS", true),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Gate should pass (human is non-blocking)"
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");

    let history = verdict["history"].as_array().unwrap();
    let human = history
        .iter()
        .find(|v| v["name"] == "manual-review")
        .unwrap();
    assert_eq!(human["status"], "fail");
    let feedback = human["feedback"].as_str().unwrap();
    assert!(
        feedback.starts_with("[human-review-requested]"),
        "Human feedback should have prefix: {feedback}"
    );

    let post = history
        .iter()
        .find(|v| v["name"] == "post-check")
        .unwrap();
    assert_eq!(post["status"], "pass");
}

// ─── LLM Freeform Response Produces Warn ───────────────────

#[test]
fn e2e_llm_freeform_always_warns() {
    let server = httpmock::MockServer::start();

    server.mock(|when, then| {
        when.method(httpmock::Method::GET).path("/v1/models");
        then.status(200).json_body(serde_json::json!({
            "data": [{"id": "test-model"}]
        }));
    });

    server.mock(|when, then| {
        when.method(httpmock::Method::POST)
            .path("/v1/chat/completions");
        then.status(200).json_body(serde_json::json!({
            "choices": [{"message": {"content": "This code looks reasonable but could use better error handling."}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 30}
        }));
    });

    let validators =
        llm_validator_freeform("review", "advisory", "Review this code", "default");
    let runtime = runtime_toml("default", &server.url(""));
    let toml = format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

{runtime}

[gates.review]
{validators}
"#
    );
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Freeform LLM should not block (warn status): {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");

    let history = verdict["history"].as_array().unwrap();
    let advisory = history.iter().find(|v| v["name"] == "advisory").unwrap();
    assert_eq!(
        advisory["status"], "warn",
        "Freeform response should produce warn status"
    );
}

// ─── --diff Workflow ───────────────────────────────────────

#[test]
fn e2e_diff_workflow() {
    let toml = minimal_toml("review", &script_validator_for("review", "lint", "echo PASS"));
    let dir = setup_git_project(&toml, &[("src/main.rs", "fn main() {}")]);

    // Modify a file after the initial commit
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { println!(\"hello\"); }",
    )
    .unwrap();

    let output = baton()
        .args(["check", "--no-log", "--diff", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Diff workflow should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Suppressed Verdicts in History ────────────────────────

#[test]
fn e2e_suppressed_verdict_history_records_true_status() {
    let toml = minimal_toml(
        "review",
        &script_validator_blocking_for("review", "failing", "exit 1", true),
    );
    let dir = setup_project(&toml, "hello");

    // Run with --suppress-all (logging enabled to write history)
    let output = baton()
        .args(["check", "--suppress-all"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass with suppress-all: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The verdict JSON should have the true status in history
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let failing = history
        .iter()
        .find(|v| v["name"] == "failing")
        .unwrap();
    assert_eq!(
        failing["status"], "fail",
        "History should record true 'fail' status even when suppressed"
    );

    // Query the history DB to verify the true status is persisted
    let output = baton()
        .args(["history", "--gate", "review"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The verdict-level status is "pass" (suppressed), but the individual validator shows "fail"
    assert!(
        stdout.contains("pass"),
        "Verdict status should be 'pass' (suppressed): {stdout}"
    );
}
