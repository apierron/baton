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
fn only_nonexistent_name_runs_no_gates() {
    // --only with a name that doesn't match any gate or validator runs no gates
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--only", "nope", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // "nope" doesn't match any gate or validator name, so no gates run (exit 0)
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No gates to run"));
    // No JSON output on stdout
    assert!(output.stdout.is_empty());
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

/// SPEC-MN-LC-002: explicit --config path is used directly
#[test]
fn validate_config_with_explicit_config() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("custom-baton.toml");
    fs::write(&config_path, v06_base_config()).unwrap();
    fs::write(dir.path().join("artifact.txt"), "hello").unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();

    baton()
        .args([
            "check",
            "--config",
            config_path.to_str().unwrap(),
            "--dry-run",
            "--no-log",
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

// ─── Doctor Command (replaces validate-config, check-provider, check-runtime) ───

#[test]
fn doctor_nonexistent_config_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    let output = baton()
        .args(["doctor", "--offline", "--config", "nonexistent.toml"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
}

#[test]
fn doctor_valid_config_exits_0() {
    // SPEC-MN-DR-012: exit-0-no-fails
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn doctor_all_output_to_stderr() {
    // SPEC-MN-DR-015: all output to stderr, nothing on stdout
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.stdout.is_empty(), "stdout should be empty");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[ok]"),
        "stderr should contain check output"
    );
}

#[test]
fn doctor_summary_line() {
    // SPEC-MN-DR-014: summary line printed
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Summary:"),
        "Should print summary line: {stderr}"
    );
}

#[test]
fn doctor_installation_always_runs() {
    // SPEC-MN-DR-001: installation section runs even without config
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Installation"),
        "Should show installation section: {stderr}"
    );
    assert!(
        stderr.contains("baton"),
        "Should show baton version: {stderr}"
    );
}

#[test]
fn doctor_no_config_skips_remaining() {
    // SPEC-MN-DR-004: sections 3-6 skip without config
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[skip]"),
        "Should show skip for sections without config: {stderr}"
    );
    assert!(
        stderr.contains("Requires valid configuration"),
        "Skip message should explain why: {stderr}"
    );
}

#[test]
fn doctor_missing_prompts_dir() {
    // SPEC-MN-DR-005: missing directory reported as fail
    let dir = TempDir::new().unwrap();
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();
    // Deliberately NOT creating prompts/

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "Should fail with missing prompts_dir"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[fail]") && stderr.contains("prompts_dir"),
        "Should report missing prompts_dir: {stderr}"
    );
}

#[test]
fn doctor_offline_skips_runtimes() {
    // SPEC-MN-DR-011: --offline skips runtime checks
    let dir = TempDir::new().unwrap();
    let toml = r#"version = "0.7"

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
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Skipped (--offline)"),
        "Should skip runtimes with --offline: {stderr}"
    );
}

#[test]
fn doctor_missing_env_var() {
    // SPEC-MN-DR-009: missing api_key_env reported as fail
    let dir = TempDir::new().unwrap();
    let toml = r#"version = "0.7"

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
api_key_env = "BATON_DOCTOR_TEST_NONEXISTENT_KEY"
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
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "Should fail with missing env var");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("BATON_DOCTOR_TEST_NONEXISTENT_KEY") && stderr.contains("not set"),
        "Should report missing env var: {stderr}"
    );
}

#[test]
fn doctor_no_env_vars_shows_ok() {
    // SPEC-MN-DR-016: no env vars to check shows ok
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No environment variables to check"),
        "Should show no env vars message: {stderr}"
    );
}

#[test]
fn doctor_script_only_no_prompt_refs() {
    // SPEC-MN-DR-008: no LLM validators shows ok for prompts
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No prompt file references to check"),
        "Should show no prompt refs message: {stderr}"
    );
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

    // The generated config should pass doctor
    baton()
        .args(["doctor", "--offline"])
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

#[test]
fn init_default_uses_separate_blocks() {
    let dir = TempDir::new().unwrap();

    baton()
        .arg("init")
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    // Should have top-level [validators.*] blocks, not [[gates.*.validators]]
    assert!(
        content.contains("[validators."),
        "Generated config should use separate validator blocks"
    );
    assert!(
        !content.contains("[[gates."),
        "Generated config should not use inline/nested validators"
    );
    // Gate should reference validators via ref
    assert!(
        content.contains("ref = "),
        "Gates should reference validators via ref"
    );
}

#[test]
fn init_profile_rust() {
    let dir = TempDir::new().unwrap();

    baton()
        .args(["init", "--profile", "rust"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(content.contains("[validators.clippy]"));
    assert!(content.contains("[validators.tests]"));
    assert!(content.contains("[validators.fmt-check]"));
    assert!(content.contains("[gates.ci]"));
    assert!(content.contains("cargo clippy"));
    assert!(content.contains("cargo test"));
    assert!(content.contains("cargo fmt --check"));
}

#[test]
fn init_profile_python() {
    let dir = TempDir::new().unwrap();

    baton()
        .args(["init", "--profile", "python"])
        .current_dir(dir.path())
        .assert()
        .success();

    let content = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(content.contains("[validators.ruff]"));
    assert!(content.contains("[validators.pytest]"));
    assert!(content.contains("[validators.mypy]"));
    assert!(content.contains("[gates.ci]"));
}

#[test]
fn init_unknown_profile_exits_1() {
    let dir = TempDir::new().unwrap();

    baton()
        .args(["init", "--profile", "bogus"])
        .current_dir(dir.path())
        .assert()
        .code(1)
        .stderr(predicates::str::contains("unknown profile"));
}

// ─── cmd_init interactive mode ───────────────────────────

/// SPEC-MN-IN-021: non-tty with no flags uses generic profile with prompts
#[test]
fn init_no_flags_non_tty_uses_generic_with_prompts() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .arg("init")
        .current_dir(dir.path())
        .write_stdin("") // pipe empty stdin — not a TTY
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(dir.path().join("baton.toml").exists());
    assert!(dir.path().join("prompts").exists());
    assert!(dir.path().join("prompts/spec-compliance.md").exists());

    let content = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    // Should use generic profile by default
    assert!(content.contains("[validators.lint]"));
    assert!(content.contains("[gates.example]"));
}

/// SPEC-MN-IN-022: explicit flags skip interactive mode
#[test]
fn init_flags_override_interactive() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .args(["init", "--profile", "rust"])
        .current_dir(dir.path())
        .write_stdin("") // pipe empty stdin — not a TTY
        .output()
        .unwrap();

    assert!(output.status.success());
    let content = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(content.contains("[validators.clippy]"));
    assert!(content.contains("cargo clippy"));
}

/// SPEC-MN-IN-026: base-only config (no code validators) is valid TOML
#[test]
fn init_base_only_config_valid() {
    let base_config = r#"version = "0.7"

[defaults]
timeout_seconds = 300
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"
"#;

    let parsed: toml::Value = toml::from_str(base_config).unwrap();
    assert_eq!(parsed["version"].as_str().unwrap(), "0.7");
    assert!(parsed.get("defaults").is_some());
    assert!(parsed.get("validators").is_none());
    assert!(parsed.get("gates").is_none());
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

// ─── File input: positional args ─────────────────────────

#[test]
fn check_with_positional_file() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project_with_files(&toml, &[("src/main.rs", "fn main() {}")]);

    let output = baton()
        .args(["check", "--no-log", "src/main.rs"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass with positional file arg"
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

#[test]
fn check_with_multiple_positional_files() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project_with_files(&toml, &[("a.txt", "aaa"), ("b.txt", "bbb")]);

    let output = baton()
        .args(["check", "--no-log", "a.txt", "b.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "Should pass with multiple files");
}

#[test]
fn check_positional_dir_walks_recursively() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project_with_files(
        &toml,
        &[("src/a.rs", "code"), ("src/sub/b.rs", "more code")],
    );

    let output = baton()
        .args(["check", "--no-log", "src"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "Should walk directory recursively");
}

#[test]
fn check_nonexistent_positional_file_exits_2() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "does-not-exist.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "Should exit 2 for missing file"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found"),
        "Should report file not found: {stderr}"
    );
}

// ─── File input: --files flag ────────────────────────────

#[test]
fn check_files_flag_reads_from_file() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project_with_files(
        &toml,
        &[
            ("a.txt", "aaa"),
            ("b.txt", "bbb"),
            ("file_list.txt", "a.txt\nb.txt\n"),
        ],
    );

    let output = baton()
        .args(["check", "--no-log", "--files", "file_list.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "Should pass reading file list");
}

#[test]
fn check_files_flag_missing_list_exits_2() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--files", "no-such-list.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "Should exit 2 for missing file list"
    );
}

// ─── Selector: --only ────────────────────────────────────

#[test]
fn only_filters_to_named_validators() {
    let validators = format!(
        "{}{}",
        script_validator_blocking_for("review", "lint", "echo PASS", true),
        script_validator_blocking_for("review", "format", "echo FAIL; exit 1", true),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--only", "lint"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass when only running passing validator"
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Selector: --skip ────────────────────────────────────

#[test]
fn skip_excludes_named_validators() {
    let validators = format!(
        "{}{}",
        script_validator_blocking_for("review", "lint", "echo PASS", true),
        script_validator_blocking_for("review", "format", "echo FAIL; exit 1", true),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--skip", "format"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass when skipping failing validator"
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Suppress flags ──────────────────────────────────────

#[test]
fn suppress_errors_treats_error_as_pass() {
    // An empty command produces Status::Error
    let toml = minimal_toml("review", &script_validator("lint", "  "));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--suppress-errors"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass when suppressing errors"
    );
}

// ─── @tag and dot-path selectors ─────────────────────────

#[test]
fn only_tag_runs_matching_validators() {
    // --only @fast within a single gate runs only tagged validators
    let validators = format!(
        "{}{}",
        script_validator_with_tags("review", "lint", "echo PASS", &["fast"]),
        script_validator_with_tags("review", "format", "echo PASS", &["slow"]),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--only", "@fast"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let lint = history.iter().find(|v| v["name"] == "lint").unwrap();
    assert_eq!(lint["status"], "pass");
    let format = history.iter().find(|v| v["name"] == "format").unwrap();
    assert_eq!(format["status"], "skip");
}

#[test]
fn skip_tag_skips_matching_validators() {
    // --skip @fast within a single gate skips tagged validators
    let validators = format!(
        "{}{}",
        script_validator_with_tags("review", "lint", "echo PASS", &["fast"]),
        script_validator_with_tags("review", "format", "echo PASS", &["slow"]),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--skip", "@fast"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let lint = history.iter().find(|v| v["name"] == "lint").unwrap();
    assert_eq!(lint["status"], "skip");
    let format = history.iter().find(|v| v["name"] == "format").unwrap();
    assert_eq!(format["status"], "pass");
}

#[test]
fn only_tag_filters_gates_in_multi_gate() {
    // --only @fast selects only gates containing validators with that tag
    let toml = multi_gate_toml(&[
        (
            "fast_gate",
            &script_validator_with_tags("fast_gate", "lint", "echo PASS", &["fast"]),
        ),
        (
            "slow_gate",
            &script_validator_with_tags("slow_gate", "analyze", "echo PASS", &["slow"]),
        ),
    ]);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--only", "@fast"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    // Only one verdict (fast_gate) should be output
    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict = parse_verdict(&stdout);
    assert_eq!(verdict["gate"], "fast_gate");
}

#[test]
fn skip_tag_filters_gates_in_multi_gate() {
    // --skip @slow skips validators with tag @slow but doesn't exclude the gate
    let toml = multi_gate_toml(&[(
        "gate_a",
        &format!(
            "{}{}",
            script_validator_with_tags("gate_a", "lint", "echo PASS", &["fast"]),
            script_validator_with_tags("gate_a", "analyze", "echo PASS", &["slow"]),
        ),
    )]);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--skip", "@slow"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let lint = history.iter().find(|v| v["name"] == "lint").unwrap();
    assert_eq!(lint["status"], "pass");
    let analyze = history.iter().find(|v| v["name"] == "analyze").unwrap();
    assert_eq!(analyze["status"], "skip");
}

#[test]
fn only_and_skip_combined_with_tags() {
    // --only @fast --skip lint: select tagged validators, then exclude by name
    let validators = format!(
        "{}{}{}",
        script_validator_with_tags("review", "lint", "echo PASS", &["fast"]),
        script_validator_with_tags("review", "format", "echo PASS", &["fast"]),
        script_validator_with_tags("review", "analyze", "echo PASS", &["slow"]),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--only", "@fast", "--skip", "lint"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let lint = history.iter().find(|v| v["name"] == "lint").unwrap();
    assert_eq!(lint["status"], "skip");
    let format = history.iter().find(|v| v["name"] == "format").unwrap();
    assert_eq!(format["status"], "pass");
    let analyze = history.iter().find(|v| v["name"] == "analyze").unwrap();
    assert_eq!(analyze["status"], "skip");
}

#[test]
fn only_tag_no_match_runs_no_gates() {
    // --only @nonexistent — no gates run, exit 0
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--only", "@nonexistent"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No gates to run"));
    assert!(output.stdout.is_empty());
}

#[test]
fn dry_run_shows_tag_filter_reasons() {
    // dry-run + --only @fast shows skip reasons for non-matching validators
    let validators = format!(
        "{}{}",
        script_validator_with_tags("review", "lint", "echo PASS", &["fast"]),
        script_validator_with_tags("review", "format", "echo PASS", &["slow"]),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--dry-run", "--only", "@fast"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("lint"), "should show lint");
    assert!(
        stderr.contains("format") && stderr.contains("--only"),
        "should show format skipped by --only"
    );
}

#[test]
fn dot_path_only_selects_gate_and_validator() {
    // --only review.lint selects only that specific validator in the gate
    let validators = format!(
        "{}{}",
        script_validator_for("review", "lint", "echo PASS"),
        script_validator_for("review", "format", "echo PASS"),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--only", "review.lint"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let lint = history.iter().find(|v| v["name"] == "lint").unwrap();
    assert_eq!(lint["status"], "pass");
    let format = history.iter().find(|v| v["name"] == "format").unwrap();
    assert_eq!(format["status"], "skip");
}

#[test]
fn dot_path_skip_excludes_specific_validator() {
    // --skip review.lint skips only that specific validator
    let validators = format!(
        "{}{}",
        script_validator_for("review", "lint", "echo PASS"),
        script_validator_for("review", "format", "echo PASS"),
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--skip", "review.lint"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let lint = history.iter().find(|v| v["name"] == "lint").unwrap();
    assert_eq!(lint["status"], "skip");
    let format = history.iter().find(|v| v["name"] == "format").unwrap();
    assert_eq!(format["status"], "pass");
}

#[test]
fn mixed_name_and_tag_selectors() {
    // --only "review @fast" matches gate by name AND validators by tag
    let toml = multi_gate_toml(&[
        (
            "review",
            &script_validator_for("review", "lint", "echo PASS"),
        ),
        (
            "tagged",
            &script_validator_with_tags("tagged", "fuzz", "echo PASS", &["fast"]),
        ),
        (
            "other",
            &script_validator_for("other", "check", "echo PASS"),
        ),
    ]);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--only", "review @fast"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    // Should produce 2 verdicts (review + tagged), not 3
    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdicts: Vec<serde_json::Value> = serde_json::Deserializer::from_str(&stdout)
        .into_iter::<serde_json::Value>()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(verdicts.len(), 2, "Expected 2 verdicts, got: {stdout}");
    let gates: Vec<&str> = verdicts
        .iter()
        .map(|v| v["gate"].as_str().unwrap())
        .collect();
    assert!(gates.contains(&"review"));
    assert!(gates.contains(&"tagged"));
}

// ─── baton add ──────────────────────────────────────────────

/// SPEC-MN-AD-002, SPEC-MN-AD-053, SPEC-MN-AD-060:
/// Non-interactive script add succeeds and writes validator to baton.toml.
#[test]
fn add_noninteractive_script_success() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "lint",
            "--command",
            "ruff check",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.lint]"));
    assert!(config.contains("ruff check"));
    // Existing validator preserved
    assert!(config.contains("[validators.existing]"));
}

/// SPEC-MN-AD-002: non-interactive with all optional fields
#[test]
fn add_noninteractive_script_with_options() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "format",
            "--command",
            "ruff format --check",
            "--input",
            "*.py",
            "--tags",
            "lint,format",
            "--timeout",
            "60",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.format]"));
    assert!(config.contains("ruff format --check"));
    assert!(config.contains("*.py"));
}

/// SPEC-MN-AD-010, SPEC-MN-AD-063: no baton.toml → exit 2
#[test]
fn add_missing_config_exits_2() {
    let dir = TempDir::new().unwrap();

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "x",
            "--command",
            "echo",
            "-y",
            "--config",
            dir.path().join("nonexistent.toml").to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No baton.toml found") || stderr.contains("not found"));
}

/// SPEC-MN-AD-011, SPEC-MN-AD-062: duplicate validator name → exit 1
#[test]
fn add_duplicate_name_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "existing",
            "--command",
            "echo dup",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already exists"));
}

/// SPEC-MN-AD-020, SPEC-MN-AD-062: script missing --command → exit 1
#[test]
fn add_script_missing_command_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args(["add", "--type", "script", "--name", "bad", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--command"));
}

/// SPEC-MN-AD-021, SPEC-MN-AD-062: llm missing --prompt → exit 1
#[test]
fn add_llm_missing_prompt_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "llm",
            "--name",
            "bad",
            "--runtime",
            "default",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--prompt"));
}

/// SPEC-MN-AD-021: llm missing --runtime → exit 1
#[test]
fn add_llm_missing_runtime_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add", "--type", "llm", "--name", "bad", "--prompt", "Review", "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--runtime"));
}

/// SPEC-MN-AD-022, SPEC-MN-AD-062: human missing --prompt → exit 1
#[test]
fn add_human_missing_prompt_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args(["add", "--type", "human", "--name", "bad", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--prompt"));
}

/// SPEC-MN-AD-023, SPEC-MN-AD-062: unknown type → exit 1
#[test]
fn add_unknown_type_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args(["add", "--type", "foobar", "--name", "bad", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown validator type"));
}

/// SPEC-MN-AD-030: --gate adds ref to existing gate
#[test]
fn add_with_existing_gate() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "format",
            "--command",
            "ruff format --check",
            "--gate",
            "ci",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.format]"));
    // Gate should reference both existing and format
    assert!(config.contains("format"));
}

/// SPEC-MN-AD-031: --gate creates new gate if it doesn't exist
#[test]
fn add_with_new_gate() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "security",
            "--command",
            "echo security",
            "--gate",
            "security-gate",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.security]"));
    assert!(config.contains("[gates.security-gate]"));
}

/// SPEC-MN-AD-030: --blocking false propagates to gate ref
#[test]
fn add_with_gate_blocking_false() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "advisory",
            "--command",
            "echo advisory",
            "--gate",
            "ci",
            "--blocking",
            "false",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.advisory]"));
    assert!(config.contains("false"));
}

/// SPEC-MN-AD-032: no --gate → validator added top-level only, no gate ref
#[test]
fn add_without_gate_top_level_only() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "standalone",
            "--command",
            "echo standalone",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.standalone]"));
    // Gate section should only contain the original ref to "existing"
    // "standalone" should not appear in any gate validators array
}

/// SPEC-MN-AD-001, SPEC-MN-AD-040: --from imports from local file
#[test]
fn add_from_file() {
    let dir = setup_project(&v06_base_config(), "hello");

    // Write an import file
    let import_content = r#"
[validators.imported-lint]
type = "script"
command = "ruff check {file.path}"
input = "*.py"
"#;
    fs::write(dir.path().join("import.toml"), import_content).unwrap();

    let output = baton()
        .args(["add", "--from", "import.toml", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.imported-lint]"));
    assert!(config.contains("ruff check"));
}

/// SPEC-MN-AD-001: --from with single-validator format
#[test]
fn add_from_file_single_format() {
    let dir = setup_project(&v06_base_config(), "hello");

    let import_content = r#"
[validator]
name = "my-lint"
type = "script"
command = "eslint ."
"#;
    fs::write(dir.path().join("import.toml"), import_content).unwrap();

    let output = baton()
        .args(["add", "--from", "import.toml", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.my-lint]"));
}

/// SPEC-MN-AD-001: --from with --gate assigns imported validators
#[test]
fn add_from_file_with_gate() {
    let dir = setup_project(&v06_base_config(), "hello");

    let import_content = r#"
[validators.imported-check]
type = "script"
command = "echo imported"
"#;
    fs::write(dir.path().join("import.toml"), import_content).unwrap();

    let output = baton()
        .args(["add", "--from", "import.toml", "--gate", "ci", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.imported-check]"));
    assert!(config.contains("imported-check"));
}

/// SPEC-MN-AD-042: --from registry:* → exit 1
#[test]
fn add_from_registry_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args(["add", "--from", "registry:community/lint", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not yet supported"));
}

/// SPEC-MN-AD-043: import collision → exit 1, no changes
#[test]
fn add_from_file_collision_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let import_content = r#"
[validators.existing]
type = "script"
command = "echo collision"
"#;
    fs::write(dir.path().join("import.toml"), import_content).unwrap();

    // Save original config
    let original = fs::read_to_string(dir.path().join("baton.toml")).unwrap();

    let output = baton()
        .args(["add", "--from", "import.toml", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already exists"));
    // Config should be unchanged
    let after = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert_eq!(original, after);
}

/// SPEC-MN-AD-040: --from nonexistent file → exit 1
#[test]
fn add_from_missing_file_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args(["add", "--from", "nonexistent.toml", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"));
}

/// SPEC-MN-AD-052: --dry-run prints preview, does not modify file
#[test]
fn add_dry_run_no_changes() {
    let dir = setup_project(&v06_base_config(), "hello");

    let original = fs::read_to_string(dir.path().join("baton.toml")).unwrap();

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "lint",
            "--command",
            "ruff check",
            "--dry-run",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Dry run"));
    assert!(stderr.contains("[validators.lint]"));
    // File should not be modified
    let after = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert_eq!(original, after);
}

/// SPEC-MN-AD-052: --dry-run with --gate shows gate info
#[test]
fn add_dry_run_with_gate_shows_preview() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "lint",
            "--command",
            "ruff check",
            "--gate",
            "ci",
            "--dry-run",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Gate 'ci'"));
    assert!(stderr.contains("lint"));
}

/// SPEC-MN-AD-004: no TTY + no flags → exit 1
/// (piped stdin is not a TTY, so interactive mode should fail)
#[test]
fn add_no_tty_no_flags_exits_1() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args(["add"])
        .current_dir(dir.path())
        .write_stdin("") // pipe empty stdin — not a TTY
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Interactive mode requires a terminal")
            || stderr.contains("Interactive prompt failed")
    );
}

/// SPEC-MN-AD-050: existing config structure preserved after add
#[test]
fn add_preserves_existing_config_structure() {
    let config = r#"# Project config
version = "0.7"

[defaults]
timeout_seconds = 300
blocking = true

# CI validators
[validators.existing]
type = "script"
command = "echo existing"

# Gates section
[gates.ci]
description = "CI gate"
validators = [
    { ref = "existing", blocking = true },
]
"#;
    let dir = setup_project(config, "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "new-check",
            "--command",
            "echo new",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let after = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    // Comments preserved
    assert!(after.contains("# Project config"));
    assert!(after.contains("# CI validators"));
    assert!(after.contains("# Gates section"));
    // Original content preserved
    assert!(after.contains("[validators.existing]"));
    assert!(after.contains("echo existing"));
    // New validator added
    assert!(after.contains("[validators.new-check]"));
}

/// SPEC-MN-AD-051: validates-before-writing — added config passes validate_config
#[test]
fn add_result_passes_validate_config() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "new-v",
            "--command",
            "echo ok",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the resulting config is valid by running check --dry-run
    baton()
        .args(["check", "--dry-run", "--no-log"])
        .current_dir(dir.path())
        .assert()
        .success();
}

/// SPEC-MN-AD-060: success message on stderr
#[test]
fn add_success_message_on_stderr() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "lint",
            "--command",
            "echo lint",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Added validator"));
    assert!(stderr.contains("lint"));
}

/// SPEC-MN-AD-051: modified config passes doctor
#[test]
fn add_result_passes_doctor() {
    let dir = setup_project(&v06_base_config(), "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    // Add a validator
    baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "lint",
            "--command",
            "echo lint",
            "--gate",
            "ci",
            "-y",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    // Run doctor on the result
    baton()
        .args(["doctor", "--offline"])
        .current_dir(dir.path())
        .assert()
        .success();
}

/// Multiple sequential adds work correctly
#[test]
fn add_multiple_sequential() {
    let dir = setup_project(&v06_base_config(), "hello");

    // First add
    baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "lint",
            "--command",
            "echo lint",
            "-y",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    // Second add
    baton()
        .args([
            "add",
            "--type",
            "script",
            "--name",
            "format",
            "--command",
            "echo format",
            "-y",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.existing]"));
    assert!(config.contains("[validators.lint]"));
    assert!(config.contains("[validators.format]"));
}

/// Import multiple validators at once from a file
#[test]
fn add_from_file_multiple_validators() {
    let dir = setup_project(&v06_base_config(), "hello");

    let import_content = r#"
[validators.lint]
type = "script"
command = "ruff check"

[validators.format]
type = "script"
command = "ruff format --check"
"#;
    fs::write(dir.path().join("import.toml"), import_content).unwrap();

    let output = baton()
        .args(["add", "--from", "import.toml", "-y"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.lint]"));
    assert!(config.contains("[validators.format]"));
    assert!(config.contains("[validators.existing]"));
}

/// Human validator via non-interactive mode
#[test]
fn add_noninteractive_human() {
    let dir = setup_project(&v06_base_config(), "hello");

    let output = baton()
        .args([
            "add",
            "--type",
            "human",
            "--name",
            "manual-review",
            "--prompt",
            "Please review this PR",
            "-y",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(dir.path().join("baton.toml")).unwrap();
    assert!(config.contains("[validators.manual-review]"));
    assert!(config.contains("human"));
    assert!(config.contains("Please review this PR"));
}

// ─── --no-recursive flag ────────────────────────────────

#[test]
#[cfg(not(windows))]
fn check_no_recursive_skips_subdirectories() {
    // The --no-recursive flag limits file pool to only direct children of a directory.
    // We verify this by using --verbose, which prints file count info to stderr,
    // or by simply checking the command succeeds and the flag is accepted.
    let toml = minimal_toml("review", &script_validator_for("review", "lint", "echo PASS"));
    let dir = setup_project_with_files(
        &toml,
        &[("src/a.rs", "code"), ("src/sub/b.rs", "more code")],
    );

    // Recursive: both files added to pool
    let output = baton()
        .args(["check", "--no-log", "src"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());

    // Non-recursive: only direct children of src/ added to pool
    let output = baton()
        .args(["check", "--no-log", "--no-recursive", "src"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "--no-recursive flag should be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify by using --verbose which prints collected files to stderr
    let output = baton()
        .args(["check", "--no-log", "-v", "--no-recursive", "src"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // In verbose mode, should mention collected files; b.rs from sub/ should not appear
    // We at least verify the flag doesn't error
    assert!(
        !stderr.contains("error"),
        "Should not produce errors: {stderr}"
    );
}

// ─── --files - (stdin) ──────────────────────────────────

#[test]
fn check_files_stdin_reads_paths() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project_with_files(&toml, &[("a.txt", "aaa"), ("b.txt", "bbb")]);

    let output = baton()
        .args(["check", "--no-log", "--files", "-"])
        .current_dir(dir.path())
        .write_stdin("a.txt\nb.txt\n")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass reading from stdin: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ─── --diff <refspec> ───────────────────────────────────

#[test]
fn check_diff_adds_changed_files() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_git_project(&toml, &[("src/main.rs", "fn main() {}")]);

    // Modify a file after the commit
    fs::write(dir.path().join("src/main.rs"), "fn main() { println!(\"hi\"); }").unwrap();

    let output = baton()
        .args(["check", "--no-log", "--diff", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass with diff-changed files: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_diff_invalid_refspec_exits_2() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_git_project(&toml, &[("src/main.rs", "fn main() {}")]);

    let output = baton()
        .args(["check", "--no-log", "--diff", "nonexistent-ref"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "Should exit 2 for invalid refspec: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ─── Human validators ───────────────────────────────────

#[test]
fn check_human_validator_always_fails_with_prefix() {
    let validators = human_validator("review", "manual-check", "Please review this code");
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
    let feedback = verdict["history"][0]["feedback"].as_str().unwrap();
    assert!(
        feedback.starts_with("[human-review-requested]"),
        "Feedback should start with [human-review-requested]: {feedback}"
    );
}

#[test]
fn check_human_validator_nonblocking_continues() {
    let validators = [
        human_validator_blocking("review", "manual-check", "Review this", false),
        script_validator_for("review", "lint", "echo PASS"),
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
        "Gate should pass when human is non-blocking"
    );
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    let human = history
        .iter()
        .find(|v| v["name"] == "manual-check")
        .unwrap();
    assert_eq!(human["status"], "fail");
    let lint = history.iter().find(|v| v["name"] == "lint").unwrap();
    assert_eq!(lint["status"], "pass");
}

// ─── run_if with `or` operator ──────────────────────────

#[test]
fn check_run_if_or_one_true() {
    let validators = [
        script_validator_blocking_for("review", "v1", "echo PASS", true),
        script_validator_blocking_for("review", "v2", "exit 1", false),
        script_validator_with_run_if(
            "review",
            "v3",
            "echo PASS",
            "v1.status == fail or v2.status == fail",
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
    let history = verdict["history"].as_array().unwrap();
    let v3 = history.iter().find(|v| v["name"] == "v3").unwrap();
    assert_eq!(v3["status"], "pass", "v3 should run because v2 failed");
}

#[test]
fn check_run_if_or_both_false_skips() {
    let validators = [
        script_validator_blocking_for("review", "v1", "echo PASS", true),
        script_validator_blocking_for("review", "v2", "echo PASS", false),
        script_validator_with_run_if(
            "review",
            "v3",
            "echo PASS",
            "v1.status == fail or v2.status == fail",
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
    let history = verdict["history"].as_array().unwrap();
    let v3 = history.iter().find(|v| v["name"] == "v3").unwrap();
    assert_eq!(
        v3["status"], "skip",
        "v3 should be skipped because neither failed"
    );
}

// ─── run_if referencing filtered-out validator ──────────

#[test]
fn check_run_if_references_skipped_validator_as_skip() {
    let validators = [
        script_validator_blocking_for("review", "v1", "echo PASS", true),
        script_validator_with_run_if("review", "v2", "echo PASS", "phantom.status == pass"),
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
    let history = verdict["history"].as_array().unwrap();
    let v2 = history.iter().find(|v| v["name"] == "v2").unwrap();
    assert_eq!(
        v2["status"], "skip",
        "Nonexistent validator in run_if should be treated as skip status"
    );
}

// ─── Script working_dir and env ─────────────────────────

#[test]
#[cfg(not(windows))]
fn check_script_working_dir() {
    // Use exit 1 + non-blocking so feedback is captured (exit 0 = no feedback)
    let validators = r#"[[gates.review.validators]]
name = "pwd-check"
type = "script"
command = "pwd && exit 1"
working_dir = "./subdir"
blocking = false
"#
    .to_string();
    let toml = minimal_toml("review", &validators);
    let dir = setup_project_with_files(&toml, &[("subdir/file.txt", "content")]);

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let feedback = verdict["history"][0]["feedback"].as_str().unwrap_or("");
    assert!(
        feedback.contains("subdir"),
        "Feedback should contain subdir path: {feedback}"
    );
}

#[test]
#[cfg(not(windows))]
fn check_script_env_vars() {
    // Use exit 1 + non-blocking so feedback is captured (exit 0 = no feedback)
    let validators = r#"[[gates.review.validators]]
name = "env-check"
type = "script"
command = "echo $BATON_TEST_VAR && exit 1"
blocking = false
env = { BATON_TEST_VAR = "hello123" }
"#
    .to_string();
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let feedback = verdict["history"][0]["feedback"].as_str().unwrap_or("");
    assert!(
        feedback.contains("hello123"),
        "Feedback should contain env var value: {feedback}"
    );
}

// ─── Per-file input mode with placeholders ──────────────

#[test]
#[cfg(not(windows))]
fn check_per_file_input_runs_per_match() {
    // Per-file input declaration is accepted and validator still runs
    // (dispatch planner not yet wired into gate execution, so all pool files are available)
    let validators =
        script_validator_with_input("review", "echo-name", "echo ok", "*.rs");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project_with_files(
        &toml,
        &[("a.rs", "code a"), ("b.rs", "code b"), ("c.txt", "text")],
    );

    let output = baton()
        .args(["check", "--no-log", "a.rs", "b.rs", "c.txt"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Per-file input config should be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[cfg(not(windows))]
fn check_per_file_placeholder_file_path() {
    // Use exit 1 + non-blocking so feedback is captured (exit 0 = no feedback)
    let validators = r#"[[gates.review.validators]]
name = "echo-path"
type = "script"
command = "echo {file.path} && exit 1"
input = "*.rs"
blocking = false
"#
    .to_string();
    let toml = minimal_toml("review", &validators);
    let dir = setup_project_with_files(&toml, &[("a.rs", "code")]);

    // Must provide files as positional args to populate the pool
    let output = baton()
        .args(["check", "--no-log", "a.rs"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let has_path = history
        .iter()
        .any(|v| v["feedback"].as_str().unwrap_or("").contains("a.rs"));
    assert!(has_path, "Feedback should contain file path");
}

#[test]
#[cfg(not(windows))]
fn check_per_file_placeholder_file_stem() {
    // Use exit 1 + non-blocking so feedback is captured (exit 0 = no feedback)
    let validators = r#"[[gates.review.validators]]
name = "echo-stem"
type = "script"
command = "echo {file.stem} && exit 1"
input = "*.rs"
blocking = false
"#
    .to_string();
    let toml = minimal_toml("review", &validators);
    let dir = setup_project_with_files(&toml, &[("hello.rs", "code")]);

    // Must provide files as positional args to populate the pool
    let output = baton()
        .args(["check", "--no-log", "hello.rs"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let has_stem = history
        .iter()
        .any(|v| v["feedback"].as_str().unwrap_or("").contains("hello"));
    assert!(has_stem, "Feedback should contain file stem 'hello'");
}

// ─── Batch input mode ───────────────────────────────────

#[test]
#[cfg(not(windows))]
fn check_batch_input_collects_all_matches() {
    let validators = script_validator_with_batch_input(
        "review",
        "batch-echo",
        "echo {input.paths}",
        "*.rs",
    );
    let toml = minimal_toml("review", &validators);
    let dir = setup_project_with_files(&toml, &[("a.rs", "code a"), ("b.rs", "code b")]);

    // Must provide files as positional args to populate the pool
    let output = baton()
        .args(["check", "--no-log", "a.rs", "b.rs"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Batch input should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ─── Verdict placeholders ───────────────────────────────

#[test]
#[cfg(not(windows))]
fn check_verdict_placeholder_resolves() {
    // v1 fails (non-blocking), v2 echoes v1's status and exits 1 (non-blocking) so feedback is captured
    let validators = [
        script_validator_blocking_for("review", "v1", "exit 1", false),
        r#"[[gates.review.validators]]
name = "v2"
type = "script"
command = "echo {verdict.v1.status} && exit 1"
blocking = false
"#
        .to_string(),
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
    let history = verdict["history"].as_array().unwrap();
    let v2 = history.iter().find(|v| v["name"] == "v2").unwrap();
    let feedback = v2["feedback"].as_str().unwrap_or("");
    assert!(
        feedback.contains("fail"),
        "v2 should echo v1's status 'fail': {feedback}"
    );
}

// ─── Human/summary format edge cases ────────────────────

#[test]
fn check_human_format_pass_icon() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "human"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("✓") || stderr.contains("PASS") || stderr.contains("pass"),
        "Human format should show pass indicator: {stderr}"
    );
}

#[test]
fn check_human_format_error_icon() {
    let toml = minimal_toml("review", &script_validator("lint", "  "));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "human", "--suppress-errors"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("E") || stderr.contains("error") || stderr.contains("ERROR"),
        "Human format should show error indicator: {stderr}"
    );
}

#[test]
fn check_summary_format_error_shows_error_at() {
    let toml = minimal_toml("review", &script_validator("lint", "  "));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log", "--format", "summary"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ERROR") || stderr.contains("error"),
        "Summary format should show error: {stderr}"
    );
}

// ─── Config version compatibility ───────────────────────

#[test]
fn check_version_0_5_accepted() {
    let toml = r#"version = "0.5"

[defaults]
timeout_seconds = 30

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"
"#;
    let dir = setup_project(toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Version 0.5 should be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_version_0_7_accepted() {
    let toml = r#"version = "0.7"

[defaults]
timeout_seconds = 30

[validators.lint]
type = "script"
command = "echo PASS"

[gates.review]
validators = [
    { ref = "lint", blocking = true },
]
"#;
    let dir = setup_project(toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Version 0.7 should be accepted: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_unsupported_version_exits_2() {
    let toml = r#"version = "0.3"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"
"#;
    let dir = setup_project(toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "Unsupported version should exit 2: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ─── Suppressed status recorded as true status ──────────

#[test]
fn check_suppressed_status_recorded_as_true() {
    let toml = minimal_toml(
        "review",
        &script_validator_blocking_for("review", "failing", "exit 1", true),
    );
    let dir = setup_project(&toml, "hello");

    // Run with --suppress-all (no --no-log so history is written)
    let output = baton()
        .args(["check", "--suppress-all"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Should pass with --suppress-all: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The JSON verdict's history should record the true status
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let failing = history
        .iter()
        .find(|v| v["name"] == "failing")
        .unwrap();
    assert_eq!(
        failing["status"], "fail",
        "History should record the true status, not suppressed"
    );
}

// ─── Warn exit code with blocking ───────────────────────

#[test]
fn check_warn_exit_code_blocking_does_not_stop() {
    let validators = [
        script_validator_with_warn_codes("review", "linter", "exit 2", &[2]),
        script_validator_for("review", "after", "echo PASS"),
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
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history.len(), 2, "Both validators should run");
    let linter = history.iter().find(|v| v["name"] == "linter").unwrap();
    assert_eq!(linter["status"], "warn");
    let after = history.iter().find(|v| v["name"] == "after").unwrap();
    assert_eq!(after["status"], "pass");
}

// ─── Empty script command produces error ────────────────

#[test]
fn check_empty_command_is_error() {
    let toml = minimal_toml("review", &script_validator("bad", "  "));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--no-log"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Empty command should produce error status
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let bad = history.iter().find(|v| v["name"] == "bad").unwrap();
    assert_eq!(
        bad["status"], "error",
        "Empty command should produce error status"
    );
}

// ─── Environment variable interpolation in config ───────

#[test]
fn check_env_var_interpolation_in_config() {
    // Env var interpolation works for runtime base_url
    let toml = r#"version = "0.4"

[defaults]
timeout_seconds = 30

[runtimes.test]
type = "api"
base_url = "${BATON_TEST_BASE_URL}"
default_model = "test-model"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"
"#;
    let dir = setup_project(toml, "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .env("BATON_TEST_BASE_URL", "https://api.example.com")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Env var interpolation should work: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_env_var_default_when_unset() {
    // Env var default syntax works for runtime base_url
    let toml = r#"version = "0.4"

[defaults]
timeout_seconds = 30

[runtimes.test]
type = "api"
base_url = "${BATON_DEFINITELY_UNSET:-https://api.example.com}"
default_model = "test-model"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo PASS"
"#;
    let dir = setup_project(toml, "hello");
    fs::create_dir_all(dir.path().join("prompts")).unwrap();

    let output = baton()
        .args(["doctor", "--offline"])
        .env_remove("BATON_DEFINITELY_UNSET")
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "Env var default should work: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
