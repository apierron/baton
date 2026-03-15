use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

// ─── Helpers ──────────────────────────────────────────────

fn baton() -> Command {
    Command::cargo_bin("baton").unwrap()
}

/// Creates a temp dir with a baton.toml and artifact file, returning the TempDir handle.
fn setup_project(toml: &str, artifact_content: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::write(dir.path().join("artifact.txt"), artifact_content).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();
    dir
}

fn minimal_toml(gate: &str, validators: &str) -> String {
    format!(
        r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

[gates.{gate}]
{validators}
"#
    )
}

fn script_validator(name: &str, command: &str) -> String {
    format!(
        r#"[[gates.review.validators]]
name = "{name}"
type = "script"
command = "{command}"
"#
    )
}

fn script_validator_for(gate: &str, name: &str, command: &str) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
"#
    )
}

fn script_validator_blocking(name: &str, command: &str, blocking: bool) -> String {
    format!(
        r#"[[gates.review.validators]]
name = "{name}"
type = "script"
command = "{command}"
blocking = {blocking}
"#
    )
}

fn script_validator_with_tags(name: &str, command: &str, tags: &[&str]) -> String {
    let tags_str = tags
        .iter()
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"[[gates.review.validators]]
name = "{name}"
type = "script"
command = "{command}"
tags = [{tags_str}]
"#
    )
}

fn parse_verdict(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout).expect("Failed to parse JSON verdict")
}

// ─── Pass / Fail ──────────────────────────────────────────

#[test]
fn check_pass() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args(["check", "--gate", "review", "--artifact", "artifact.txt", "--no-log"])
        .current_dir(dir.path())
        .assert()
        .success();
}

#[test]
fn check_pass_json_output() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--gate", "review", "--artifact", "artifact.txt", "--no-log"])
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
fn check_fail_exit_code_1() {
    let toml = minimal_toml("review", &script_validator("lint", "echo FAIL; exit 1"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args(["check", "--gate", "review", "--artifact", "artifact.txt", "--no-log"])
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
        .args(["check", "--gate", "review", "--artifact", "artifact.txt", "--no-log"])
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
        .args(["check", "--gate", "review", "--artifact", "artifact.txt", "--no-log"])
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
        .args(["check", "--gate", "review", "--artifact", "artifact.txt", "--no-log"])
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
        .args(["check", "--gate", "review", "--artifact", "artifact.txt", "--no-log"])
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

#[test]
fn all_flag_runs_past_blocking_failure() {
    let validators = [
        script_validator_blocking("blocker", "exit 1", true),
        script_validator_blocking("after", "echo PASS", true),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--all",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "fail");
    let history = verdict["history"].as_array().unwrap();
    let after = history.iter().find(|v| v["name"] == "after").unwrap();
    assert_eq!(after["status"], "pass");
}

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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--dry-run",
        ])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty());

    // Dry run output goes to stderr
    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--dry-run",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("v1"));
    assert!(stderr.contains("v2"));
    assert!(stderr.contains("Dry run"));
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--dry-run", "--only", "v1",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--only", "v1,v3",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--skip", "v1",
        ])
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

#[test]
fn tags_filters_validators() {
    let validators = [
        script_validator_with_tags("fast", "echo PASS", &["quick"]),
        script_validator_with_tags("slow", "echo PASS", &["thorough"]),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--tags", "quick",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let history = verdict["history"].as_array().unwrap();
    let slow = history.iter().find(|v| v["name"] == "slow").unwrap();
    assert_eq!(slow["status"], "skip");
    let fast = history.iter().find(|v| v["name"] == "fast").unwrap();
    assert_eq!(fast["status"], "pass");
}

#[test]
fn only_invalid_validator_exits_2() {
    let toml = minimal_toml("review", &script_validator("v1", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--only", "nonexistent",
        ])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--only references unknown validator"));
}

// ─── Stdin Artifact ──────────────────────────────────────

#[test]
fn stdin_artifact() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "-", "--no-log",
        ])
        .current_dir(dir.path())
        .write_stdin("stdin content here")
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Missing Config ──────────────────────────────────────

#[test]
fn missing_config_exits_2() {
    let dir = TempDir::new().unwrap();
    // Put a .git so discover_config stops searching
    fs::create_dir(dir.path().join(".git")).unwrap();

    baton()
        .args(["check", "--gate", "review", "--artifact", "foo.txt"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Error"));
}

#[test]
fn explicit_missing_config_exits_2() {
    let dir = TempDir::new().unwrap();

    baton()
        .args([
            "check", "--gate", "review", "--artifact", "foo.txt",
            "--config", "nonexistent.toml",
        ])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not found"));
}

// ─── Nonexistent Gate ─────────────────────────────────────

#[test]
fn nonexistent_gate_exits_2() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args([
            "check", "--gate", "nope", "--artifact", "artifact.txt",
        ])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not found"))
        .stderr(predicate::str::contains("review"));
}

// ─── Output Formats ──────────────────────────────────────

#[test]
fn format_json_on_stdout() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--format", "json",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--format", "human",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--format", "summary",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--format", "summary",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--format", "bogus",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    let db_path = dir.path().join(".baton/history.db");
    assert!(!db_path.exists(), "history.db should not be created with --no-log");
}

#[test]
fn without_no_log_creates_db() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    let db_path = dir.path().join(".baton/history.db");
    assert!(db_path.exists(), "history.db should be created without --no-log");
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--suppress-warnings",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--suppress-errors",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--suppress-all",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Context ─────────────────────────────────────────────

#[test]
fn context_file_passed_to_validator() {
    let toml = minimal_toml(
        "review",
        &script_validator("check", "cat $BATON_CONTEXT_spec && echo PASS"),
    );
    let dir = setup_project(&toml, "hello");
    fs::write(dir.path().join("spec.md"), "spec content").unwrap();

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--context", "spec=spec.md",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

#[test]
fn context_hash_in_verdict() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");
    fs::write(dir.path().join("ref.md"), "reference").unwrap();

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--context", "ref=ref.md",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let ctx_hash = verdict["context_hash"].as_str().unwrap();
    assert!(!ctx_hash.is_empty());
}

// ─── Artifact Hash ──────────────────────────────────────

#[test]
fn artifact_hash_is_deterministic() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "exact content");

    let output1 = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let output2 = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let v1 = parse_verdict(&String::from_utf8_lossy(&output1.stdout));
    let v2 = parse_verdict(&String::from_utf8_lossy(&output2.stdout));
    assert_eq!(v1["artifact_hash"], v2["artifact_hash"]);
}

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
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--config",
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

    baton()
        .args([
            "check", "--gate", "pass_gate", "--artifact", "artifact.txt",
            "--no-log",
        ])
        .current_dir(dir.path())
        .assert()
        .success();

    baton()
        .args([
            "check", "--gate", "fail_gate", "--artifact", "artifact.txt",
            "--no-log",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    let feedback = verdict["history"][0]["feedback"].as_str().unwrap();
    assert!(feedback.contains("missing semicolons"));
}

// ─── Skip Warns on Unknown Validator ─────────────────────

#[test]
fn skip_unknown_validator_warns_but_runs() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--skip", "nonexistent",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    // Should still succeed (it's a warning, not an error)
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning"));
    assert!(stderr.contains("nonexistent"));
}

// ─── Artifact via Environment Variable ──────────────────

#[test]
fn artifact_path_available_to_script() {
    let toml = minimal_toml(
        "review",
        &script_validator("check", "test -f $BATON_ARTIFACT && echo PASS"),
    );
    let dir = setup_project(&toml, "hello world");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
}

// ─── Duration Tracked ────────────────────────────────────

#[test]
fn duration_tracked_in_verdict() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log",
        ])
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
        .args(["check", "--gate", "review", "--artifact", "artifact.txt"])
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
        .args(["check", "--gate", "review", "--artifact", "artifact.txt"])
        .current_dir(dir.path())
        .assert()
        .code(2);
}

// ─── Nonexistent Artifact ────────────────────────────────

#[test]
fn nonexistent_artifact_exits_2() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    baton()
        .args([
            "check", "--gate", "review", "--artifact", "does_not_exist.txt",
            "--no-log",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
        ])
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
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--format", "human",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("VERDICT: FAIL"));
    assert!(stderr.contains("lint"));
}

// ─── Skip References Warning ─────────────────────────────

#[test]
fn skip_unknown_only_warns() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    // --skip with unknown name should warn but not error
    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--skip", "bogus",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Warning: --skip references unknown validator 'bogus'"));
}

// ─── Dry Run with Tags ──────────────────────────────────

#[test]
fn dry_run_with_tags_filter() {
    let validators = [
        script_validator_with_tags("tagged", "echo PASS", &["ci"]),
        script_validator_with_tags("untagged", "echo PASS", &["local"]),
    ]
    .join("\n");
    let toml = minimal_toml("review", &validators);
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--dry-run", "--tags", "ci",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("untagged"));
    assert!(stderr.contains("--tags"));
}

// ─── Empty Gate (No Validators) ──────────────────────────

#[test]
fn all_validators_skipped_still_passes() {
    let toml = minimal_toml("review", &script_validator("lint", "echo PASS"));
    let dir = setup_project(&toml, "hello");

    let output = baton()
        .args([
            "check", "--gate", "review", "--artifact", "artifact.txt",
            "--no-log", "--skip", "lint",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let verdict = parse_verdict(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(verdict["status"], "pass");
    let history = verdict["history"].as_array().unwrap();
    assert_eq!(history[0]["status"], "skip");
}
