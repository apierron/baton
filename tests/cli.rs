mod common;
use common::*;

use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

// ─── Pass / Fail ──────────────────────────────────────────

#[test]
fn check_pass() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn check_pass_json_output() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    assert_eq!(verdict["gate"], "review");
    assert!(verdict["history"].as_array().unwrap().len() == 1);
    assert_eq!(verdict["history"][0]["name"], "lint");
    assert_eq!(verdict["history"][0]["status"], "pass");
}

#[test]
#[cfg(not(windows))]
fn check_fail_exit_code_1() {
    let toml = minimal_toml("review", &script_validator("lint", "echo FAIL; exit 1"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "fail");
    assert_eq!(verdict["failed_at"], "lint");
}

#[test]
#[cfg(windows)]
fn check_fail_exit_code_1() {
    let toml = minimal_toml("review", &script_validator("lint", "echo FAIL & exit /b 1"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "fail");
    assert_eq!(verdict["failed_at"], "lint");
}

#[test]
fn nonzero_exit_is_fail_not_error() {
    // Any non-zero, non-warn exit code produces Status::Fail at the validator level
    let toml = minimal_toml("review", &script_validator("lint", "exit 2"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "fail");
    assert_eq!(verdict["history"][0]["status"], "fail");
}

#[test]
fn check_multiple_validators_all_pass() {
    let validators = [
        script_validator("v1", "echo PASS"),
        script_validator("v2", "echo PASS"),
        script_validator("v3", "echo PASS"),
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
    assert_eq!(verdict["history"].as_array().unwrap().len(), 3);
}

// ─── Blocking Stops Pipeline ─────────────────────────────

#[test]
fn blocking_validator_stops_pipeline() {
    let validators = [
        script_validator_blocking("first", "echo PASS", true),
        script_validator_blocking("blocker", "exit 1", true),
        script_validator_blocking("after", "echo PASS", true),
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
    assert_eq!(verdict["failed_at"], "blocker");
    // "after" is not in history — early return skips remaining validators
    let history = verdict["history"].as_array().unwrap();
    assert!(
        !history.iter().any(|v| v["name"] == "after"),
        "blocking failure should prevent remaining validators from appearing in history"
    );
    assert_eq!(history.len(), 2); // first + blocker
}

#[test]
fn non_blocking_failure_does_not_stop_pipeline() {
    let validators = [
        script_validator_blocking("non-blocker", "exit 1", false),
        script_validator_blocking("after", "echo PASS", true),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    // Non-blocking fail doesn't block — gate passes because no blocking validator failed
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    let after = history.iter().find(|v| v["name"] == "after").unwrap();
    assert_eq!(after["status"], "pass");
    // But the non-blocking validator itself failed
    let nb = history.iter().find(|v| v["name"] == "non-blocker").unwrap();
    assert_eq!(nb["status"], "fail");
}

// (all_flag_runs_past_blocking_failure removed — --all flag no longer exists)

// ─── Dry Run ──────────────────────────────────────────────

#[test]
fn dry_run_lists_validators_and_exits_zero() {
    let validators = [
        script_validator("v1", "echo PASS"),
        script_validator("v2", "exit 1"),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check", "--dry-run"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    // Dry run output goes to stderr
    let output = baton()
        .args(["check", "--dry-run"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("v1"));
    assert!(stderr.contains("v2"));
    assert!(stderr.contains("Gate 'review'"));
}

#[test]
fn dry_run_shows_skip_reasons() {
    let validators = [
        script_validator("v1", "echo PASS"),
        script_validator("v2", "echo PASS"),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--only", "review v1", "--dry-run"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("v2"));
    assert!(stderr.contains("--only"));
}

// ─── --only / --skip / --tags Filtering ──────────────────

#[test]
fn only_runs_specified_validators() {
    let validators = [
        script_validator("v1", "echo PASS"),
        script_validator("v2", "echo PASS"),
        script_validator("v3", "echo PASS"),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--only", "review v1 v3", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let v2 = history.iter().find(|v| v["name"] == "v2").unwrap();
    assert_eq!(v2["status"], "skip");
    let v1 = history.iter().find(|v| v["name"] == "v1").unwrap();
    assert_eq!(v1["status"], "pass");
}

#[test]
fn skip_excludes_validators() {
    let validators = [
        script_validator("v1", "echo PASS"),
        script_validator("v2", "echo PASS"),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--skip", "v1"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let v1 = history.iter().find(|v| v["name"] == "v1").unwrap();
    assert_eq!(v1["status"], "skip");
    let v2 = history.iter().find(|v| v["name"] == "v2").unwrap();
    assert_eq!(v2["status"], "pass");
}

// (only_invalid_validator_exits_2 removed — --only with unknown name no longer errors,
// it just means no validators match and the gate passes with all skipped)

// ─── Missing Config ──────────────────────────────────────

#[test]
fn missing_config_exits_2() {
    let dir = TempDir::new().unwrap();
    // Put a .git so discover_config stops searching
    fs::create_dir(dir.path().join(".git")).unwrap();

    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Error"));
}

#[test]
fn explicit_missing_config_exits_2() {
    let dir = TempDir::new().unwrap();

    baton()
        .args(["check", "--config", "nonexistent.toml"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not found"));
}

// ─── Nonexistent Gate ─────────────────────────────────────

#[test]
fn nonexistent_gate_runs_all_gates() {
    // --only with a name that doesn't match any gate runs all gates
    // (gate-level filtering only kicks in when the name matches a gate)
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--only", "nope", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // "nope" doesn't match any gate name, so all gates run.
    // But inside each gate, --only filters validators: "nope" doesn't match "lint",
    // so "lint" is skipped. Gate passes with all validators skipped.
    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history[0]["status"], "skip");
}

// ─── Output Formats ──────────────────────────────────────

#[test]
fn format_json_on_stdout() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "json"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_ok());
}

#[test]
fn format_human_on_stderr() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "human"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    // Human format goes to stderr, not stdout
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("VERDICT: PASS"));
}

#[test]
fn format_summary_on_stderr() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "summary"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.trim().is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("PASS"));
}

#[test]
fn format_summary_fail_includes_validator_name() {
    let toml = minimal_toml("review", &script_validator("lint", "echo FAIL && exit 1"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "summary"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("FAIL"));
    assert!(stderr.contains("lint"));
}

#[test]
fn unknown_format_falls_back_to_json() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "bogus"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(serde_json::from_str::<serde_json::Value>(&stdout).is_ok());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown format"));
}

// ─── --no-log ─────────────────────────────────────────────

#[test]
fn no_log_skips_db_write() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .assert()
        .success();

    let db_path = dir.path().join(".baton/history.db");
    assert!(
        !db_path.exists(),
        "history.db should not be created with --no-log"
    );
}

#[test]
fn without_no_log_creates_db() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .success();

    let db_path = dir.path().join(".baton/history.db");
    assert!(
        db_path.exists(),
        "history.db should be created without --no-log"
    );
}

// ─── --suppress-* Flags ──────────────────────────────────

#[test]
fn suppress_warnings_treats_warn_as_pass() {
    // Use warn_exit_codes to produce a warn status
    let toml_str = r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[gates.review]
[[gates.review.validators]]
name = "linter"
type = "script"
command = "exit 3"
warn_exit_codes = [3]
"#;
    let dir = setup_project(toml_str, "hello");

    // Without suppress: should still pass (warn doesn't fail the gate by default)
    let output = baton()
        .args(["check", "--no-log", "--suppress-warnings"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let suppressed = verdict["suppressed"].as_array().unwrap();
    assert!(!suppressed.is_empty());
}

#[test]
fn suppress_errors_listed_in_verdict() {
    // Script validators produce Fail (not Error) for non-zero exits,
    // but --suppress-errors still appears in the suppressed list
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--suppress-errors"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let suppressed = verdict["suppressed"].as_array().unwrap();
    assert!(suppressed.iter().any(|s| s == "error"));
}

#[test]
fn suppress_all_treats_everything_as_pass() {
    let validators = [
        script_validator_blocking("fail-validator", "exit 1", true),
        script_validator_blocking("pass-validator", "echo PASS", true),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--suppress-all"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── (Context and artifact hash tests removed in v2 migration) ──

// ─── Explicit Config Path ────────────────────────────────

#[test]
fn explicit_config_path() {
    let dir = TempDir::new().unwrap();
    let config_dir = dir.path().join("custom");
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(config_dir.join(".baton/tmp")).unwrap();
    fs::create_dir_all(config_dir.join(".baton/logs")).unwrap();

    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(config_dir.join("custom.toml"), &toml).unwrap();
    fs::write(dir.path().join("artifact.txt"), "hello").unwrap();

    baton()
        .args([
            "check",
            "--no-log",
            "--config",
            config_dir.join("custom.toml").to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .assert()
        .success();
}

// ─── Multiple Gates ──────────────────────────────────────

#[test]
fn multiple_gates_selects_correct_one() {
    let toml = format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[gates.pass_gate]
{pass_v}

[gates.fail_gate]
{fail_v}
"#,
        pass_v = script_validator_for("pass_gate", "ok", "echo PASS"),
        fail_v = script_validator_for("fail_gate", "bad", "exit 1"),
    );
    let dir = setup_project(&toml, "hello");

    // --only filters both gates and validators; include validator names too
    baton()
        .args(["check", "--only", "pass_gate ok", "--no-log"])
        .current_dir(dir.path())
        .assert()
        .success();

    baton()
        .args(["check", "--only", "fail_gate bad", "--no-log"])
        .current_dir(dir.path())
        .assert()
        .code(1);
}

// ─── Validator Feedback in Output ────────────────────────

#[test]
fn validator_feedback_captured() {
    let toml = minimal_toml(
        "review",
        &script_validator("lint", "echo 'FAIL: missing semicolons' && exit 1"),
    );
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let feedback = verdict["history"][0]["feedback"].as_str().unwrap();
    assert!(feedback.contains("missing semicolons"));
}

// ─── Skip with Unknown Validator ─────────────────────────

#[test]
fn skip_unknown_validator_still_succeeds() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--skip", "nonexistent"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Unknown skip name is silently ignored; validators run normally
    assert!(output.status.success());
}

// ─── (Artifact env var tests removed in v2 migration) ──

// ─── Duration Tracked ────────────────────────────────────

#[test]
fn duration_tracked_in_verdict() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert!(verdict["duration_ms"].as_i64().unwrap() >= 0);
    assert!(verdict["history"][0]["duration_ms"].as_i64().unwrap() >= 0);
}

// ─── Invalid Config ──────────────────────────────────────

#[test]
fn invalid_toml_exits_2() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("baton.toml"), "this is not valid toml [[[").unwrap();
    fs::write(dir.path().join("artifact.txt"), "hello").unwrap();

    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Error"));
}

#[test]
fn missing_version_exits_2() {
    let dir = TempDir::new().unwrap();
    let toml = r#"
[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"
"#;
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::write(dir.path().join("artifact.txt"), "hello").unwrap();

    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .code(2);
}

// ─── Nonexistent Artifact ────────────────────────────────

#[test]
fn nonexistent_file_exits_2() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check", "--no-log", "does_not_exist.txt"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Error"));
}

// ─── Timestamp Present ───────────────────────────────────

#[test]
fn timestamp_present_in_verdict() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let ts = verdict["timestamp"].as_str().unwrap();
    assert!(!ts.is_empty());
    // Should be ISO 8601 format
    assert!(ts.contains("T"));
}

// ─── History Query After Check ───────────────────────────

#[test]
fn history_records_after_check() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    // Run a check with logging
    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Query history
    let output = baton()
        .args(["history", "--gate", "review"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("review"));
    assert!(stdout.contains("pass"));
}

// ─── Human Format Shows Failure Details ──────────────────

#[test]
fn human_format_shows_failure_feedback() {
    let toml = minimal_toml(
        "review",
        &script_validator("lint", "echo 'FAIL: bad style' && exit 1"),
    );
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "human"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("VERDICT: FAIL"));
    assert!(stderr.contains("lint"));
}

// ─── Skip with Unknown Name ──────────────────────────────

#[test]
fn skip_unknown_name_silently_ignored() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    // --skip with unknown name is silently ignored
    let output = baton()
        .args(["check", "--no-log", "--skip", "bogus"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Empty Gate (No Validators) ──────────────────────────

#[test]
fn all_validators_skipped_still_passes() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--skip", "lint"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history[0]["status"], "skip");
}

// ─── Init Command ────────────────────────────────────────

#[test]
fn init_creates_baton_toml_and_baton_dir() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(dir.path().join("baton.toml").exists());
    assert!(dir.path().join(".baton/logs").exists());
    assert!(dir.path().join(".baton/tmp").exists());
    // Default (non-minimal) also creates prompts/
    assert!(dir.path().join("prompts").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("initialized"));
}

#[test]
fn init_minimal_skips_prompts_dir() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .args(["init", "--minimal"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(dir.path().join("baton.toml").exists());
    assert!(dir.path().join(".baton/logs").exists());
    assert!(dir.path().join(".baton/tmp").exists());
    assert!(
        !dir.path().join("prompts").exists(),
        "prompts/ should not be created with --minimal"
    );
}

#[test]
fn init_when_baton_toml_already_exists_returns_error() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("baton.toml"), "existing content").unwrap();

    let output = baton()
        .arg("init")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already exists"));
    // Original content should not be overwritten
    let content = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert_eq!(content, "existing content");
}

#[test]
fn init_prompts_only_creates_only_prompts() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .args(["init", "--prompts-only"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(
        !dir.path().join("baton.toml").exists(),
        "baton.toml should not be created with --prompts-only"
    );
    assert!(dir.path().join("prompts").exists());
    // Starter templates should exist
    assert!(dir.path().join("prompts/spec-compliance.md").exists());
    assert!(dir.path().join("prompts/adversarial-review.md").exists());
    assert!(dir.path().join("prompts/doc-completeness.md").exists());
}

// ─── Version Command ─────────────────────────────────────

#[test]
fn version_outputs_crate_version() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .arg("version")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("baton"));
    assert!(stdout.contains("spec version: 0.5"));
}

#[test]
fn version_shows_config_not_found_when_no_config() {
    let dir = TempDir::new().unwrap();
    // Put a .git so discover_config stops searching
    fs::create_dir(dir.path().join(".git")).unwrap();

    let output = baton()
        .arg("version")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config: not found"));
}

#[test]
fn version_shows_config_found_when_config_exists() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .arg("version")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config:"));
    assert!(stdout.contains("(found)"));
}

// ─── Validate-Config Command ─────────────────────────────

#[test]
fn validate_config_valid_exits_0() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .arg("validate-config")
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Config OK"));
}

#[test]
fn validate_config_invalid_exits_1() {
    let dir = TempDir::new().unwrap();
    // Config with a validator missing a command field
    let bad_toml = r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
"#;
    fs::write(dir.path().join("baton.toml"), bad_toml).unwrap();

    let output = baton()
        .arg("validate-config")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error"));
}

#[test]
fn validate_config_nonexistent_file_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    let output = baton()
        .args(["validate-config", "--config", "nonexistent.toml"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
}

// ─── List Command ────────────────────────────────────────

#[test]
fn list_all_gates() {
    let toml = format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[gates.alpha]
{alpha_v}

[gates.beta]
{beta_v}
"#,
        alpha_v = script_validator_for("alpha", "lint", "echo PASS"),
        beta_v = script_validator_for("beta", "test", "echo PASS"),
    );
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .arg("list")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("alpha"));
    assert!(stdout.contains("beta"));
    assert!(stdout.contains("Available gates"));
}

#[test]
fn list_validators_for_specific_gate() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["list", "--gate", "review"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Gate: review"));
    assert!(stdout.contains("lint"));
    assert!(stdout.contains("script"));
}

#[test]
fn list_nonexistent_gate_exits_1() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["list", "--gate", "nonexistent"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));
}

// ─── History Command (Empty) ─────────────────────────────

#[test]
fn history_empty_shows_no_verdicts() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["history", "--gate", "review"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No verdicts found"));
}

#[test]
fn history_with_status_filter() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    // Run a passing check
    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Query for fail status - should find nothing
    let output = baton()
        .args(["history", "--gate", "review", "--status", "fail"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No verdicts found"));

    // Query for pass status - should find the result
    let output = baton()
        .args(["history", "--gate", "review", "--status", "pass"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pass"));
    assert!(stdout.contains("review"));
}

// ─── Clean Command ───────────────────────────────────────

#[test]
fn clean_with_no_stale_files() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .arg("clean")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No stale files"));
}

#[test]
fn clean_dry_run() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["clean", "--dry-run"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    // With no stale files, dry run should also report nothing
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No stale files") || stderr.contains("would be removed"),
        "Expected clean output, got: {stderr}"
    );
}

// ─── Check with --verbose flag ───────────────────────────

#[test]
fn check_verbose_flag_accepted() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check", "--no-log", "--verbose"])
        .current_dir(dir.path())
        .assert()
        .success();
}

// ─── Check with --timeout override ───────────────────────

#[test]
fn check_timeout_override_accepted() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check", "--no-log", "--timeout", "60"])
        .current_dir(dir.path())
        .assert()
        .success();
}

// ─── List with --config flag ─────────────────────────────

#[test]
fn list_with_explicit_config() {
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("custom.toml"), &toml).unwrap();

    let output = baton()
        .args([
            "list",
            "--config",
            dir.path().join("custom.toml").to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("review"));
}

// ─── Validate-Config with explicit --config ──────────────

#[test]
fn validate_config_with_explicit_config() {
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("custom.toml"), &toml).unwrap();

    baton()
        .args([
            "validate-config",
            "--config",
            dir.path().join("custom.toml").to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("Config OK"));
}

// ─── History with --limit ────────────────────────────────

#[test]
fn history_respects_limit() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    // Run two checks
    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .success();
    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Query with limit 1
    let output = baton()
        .args(["history", "--gate", "review", "--limit", "1"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should have exactly one result line (containing "pass")
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "Expected 1 line with --limit 1, got: {stdout}"
    );
}

// ─── Init creates valid config ───────────────────────────

#[test]
fn init_creates_valid_parseable_config() {
    let dir = TempDir::new().unwrap();

    baton()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    // The generated config should pass validate-config
    baton()
        .arg("validate-config")
        .current_dir(dir.path())
        .assert()
        .success();
}

// ─── Clean with --config flag ────────────────────────────

#[test]
fn clean_with_explicit_config() {
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("custom.toml"), &toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();

    let output = baton()
        .args([
            "clean",
            "--config",
            dir.path().join("custom.toml").to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
}

// ─── Version with --config flag ──────────────────────────

#[test]
fn version_with_explicit_config() {
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("custom.toml"), &toml).unwrap();

    let output = baton()
        .args([
            "version",
            "--config",
            dir.path().join("custom.toml").to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("(found)"));
}

// ─── History without --gate (all gates) ──────────────────

#[test]
fn history_without_gate_filter() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    // Run a check
    baton()
        .args(["check"])
        .current_dir(dir.path())
        .assert()
        .success();

    // Query without gate filter
    let output = baton()
        .arg("history")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("review"));
    assert!(stdout.contains("pass"));
}

#[test]
#[cfg(not(windows))]
fn dry_run_shows_run_if_expression() {
    let toml = r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[gates.review]

[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"

[[gates.review.validators]]
name = "typecheck"
type = "script"
command = "echo PASS"
run_if = "lint.status == pass"
"#;
    let dir = setup_project(toml, "hello");

    let output = baton()
        .args(["check", "--dry-run", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("lint.status == pass"),
        "dry-run should show run_if expression, got: {stderr}"
    );
}

#[test]
fn unknown_format_falls_back_to_json_on_stdout() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "potato"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Should warn about unknown format but still produce JSON on stdout
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown format") || stderr.contains("unknown format"),
        "Should warn about unknown format: {stderr}"
    );
    // stdout should be valid JSON
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

#[test]
#[cfg(not(windows))]
fn suppress_all_flag() {
    let toml = minimal_toml("review", &script_validator("lint", "echo FAIL; exit 1"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--suppress-all"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "suppress-all should exit 0");
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── cmd_init gaps ───────────────────────────────────────

#[test]
fn init_creates_prompt_templates() {
    let dir = TempDir::new().unwrap();

    baton()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(dir.path().join("prompts/spec-compliance.md").exists());
    assert!(dir.path().join("prompts/adversarial-review.md").exists());
    assert!(dir.path().join("prompts/doc-completeness.md").exists());
}

#[test]
fn init_minimal_skips_prompts() {
    let dir = TempDir::new().unwrap();

    baton()
        .args(["init", "--minimal"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(dir.path().join("baton.toml").exists());
    assert!(!dir.path().join("prompts").exists());
}

#[test]
fn init_prompts_only_skips_config() {
    let dir = TempDir::new().unwrap();

    baton()
        .args(["init", "--prompts-only"])
        .current_dir(dir.path())
        .assert()
        .success();

    assert!(!dir.path().join("baton.toml").exists());
    assert!(dir.path().join("prompts").exists());
    assert!(dir.path().join("prompts/spec-compliance.md").exists());
}

#[test]
fn init_existing_prompts_not_overwritten() {
    let dir = TempDir::new().unwrap();
    let prompts_dir = dir.path().join("prompts");
    fs::create_dir_all(&prompts_dir).unwrap();

    let custom_content = "my custom prompt content — do not overwrite";
    fs::write(prompts_dir.join("spec-compliance.md"), custom_content).unwrap();

    baton()
        .args(["init", "--prompts-only"])
        .current_dir(dir.path())
        .assert()
        .success();

    let after = fs::read_to_string(prompts_dir.join("spec-compliance.md")).unwrap();
    assert_eq!(
        after, custom_content,
        "Existing prompt should not be overwritten"
    );
}

// ─── cmd_list gaps ───────────────────────────────────────

#[test]
fn list_gate_not_found_exits_1() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["list", "--gate", "nonexistent"])
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not found"));
}

// ─── cmd_history gaps ────────────────────────────────────

#[test]
fn history_empty_results_message() {
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();

    baton()
        .args(["history", "--gate", "nonexistent-gate"])
        .current_dir(dir.path())
        .assert()
        .stdout(predicate::str::contains("No verdicts found"));
}

// ─── cmd_validate_config gaps ────────────────────────────

#[test]
fn validate_config_parse_error_exits_1() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("bad.toml"), "this is not valid toml {{{}}}").unwrap();

    baton()
        .args(["validate-config", "--config", "bad.toml"])
        .current_dir(dir.path())
        .assert()
        .code(predicate::ne(0));
}

#[test]
fn validate_config_warnings_printed() {
    let dir = TempDir::new().unwrap();
    // Config with a validator referencing a non-existent provider triggers a warning
    let toml = r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[gates.review]

[[gates.review.validators]]
name = "llm-check"
type = "llm"
prompt = "Review this"
provider = "nonexistent"
model = "test"
"#;
    fs::write(dir.path().join("baton.toml"), toml).unwrap();

    let output = baton()
        .args(["validate-config"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Warning")
            || stderr.contains("warning")
            || stderr.contains("Error")
            || stderr.contains("error"),
        "Should show warning or error for undefined provider: {stderr}"
    );
}

// ─── cmd_version gaps ────────────────────────────────────

#[test]
fn version_includes_spec_version() {
    let output = baton().arg("version").output().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("spec version: 0.5"),
        "Should show spec version, got: {stdout}"
    );
}

#[test]
fn version_config_not_found_in_empty_dir() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .arg("version")
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not found"),
        "Should show 'not found' for config, got: {stdout}"
    );
}

// ─── cmd_clean gaps ──────────────────────────────────────

#[test]
fn clean_dry_run_does_not_delete() {
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("baton.toml"), &toml).unwrap();
    let tmp_dir = dir.path().join(".baton/tmp");
    fs::create_dir_all(&tmp_dir).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();

    // Create a file in tmp
    let tmp_file = tmp_dir.join("old-artifact.txt");
    fs::write(&tmp_file, "stale content").unwrap();

    baton()
        .args(["clean", "--dry-run"])
        .current_dir(dir.path())
        .assert()
        .success();

    // File should still exist after dry-run
    assert!(tmp_file.exists(), "dry-run should not delete files");
}

// ─── cmd_check_provider gaps ─────────────────────────────

#[test]
fn check_provider_missing_api_key_env() {
    let dir = TempDir::new().unwrap();
    let toml = r#"version = "0.6"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[runtimes.default]
type = "api"
base_url = "http://localhost:1"
api_key_env = "BATON_CLI_TEST_NONEXISTENT_KEY"
default_model = "test-model"

[gates.review]

[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"
"#;
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();

    let output = baton()
        .args(["check-provider"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("BATON_CLI_TEST_NONEXISTENT_KEY") || stderr.contains("not set"),
        "Should mention missing env var: {stderr}"
    );
    assert!(!output.status.success());
}

// ─── cmd_check_runtime gaps ──────────────────────────────

#[test]
fn check_runtime_no_runtimes_exits_1() {
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();

    let output = baton()
        .args(["check-runtime"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "check-runtime with no runtimes should fail"
    );
}

#[test]
fn check_runtime_named_not_found() {
    let dir = TempDir::new().unwrap();
    let toml = r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[runtimes.alpha]
type = "openhands"
base_url = "http://localhost:1"
timeout_seconds = 600
max_iterations = 30

[gates.review]

[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"
"#;
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();

    let output = baton()
        .args(["check-runtime", "beta"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "Should fail when named runtime not found"
    );
    assert!(
        stderr.contains("beta") || stderr.contains("not found"),
        "Should mention the missing runtime: {stderr}"
    );
}
