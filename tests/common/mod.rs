#![allow(dead_code)]

use assert_cmd::Command;
use std::fs;
use tempfile::TempDir;

pub fn baton() -> Command {
    Command::cargo_bin("baton").unwrap()
}

/// Creates a temp dir with a baton.toml and artifact file, returning the TempDir handle.
pub fn setup_project(toml: &str, artifact_content: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::write(dir.path().join("artifact.txt"), artifact_content).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();
    dir
}

/// Creates a temp dir with a baton.toml and multiple named files.
pub fn setup_project_with_files(toml: &str, files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
    dir
}

pub fn minimal_toml(gate: &str, validators: &str) -> String {
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

pub fn multi_gate_toml(gates: &[(&str, &str)]) -> String {
    let mut s = r#"version = "0.4"

[defaults]
timeout_seconds = 30
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

"#
    .to_string();

    for (gate_name, validators) in gates {
        s.push_str(&format!("[gates.{gate_name}]\n{validators}\n"));
    }
    s
}

pub fn script_validator(name: &str, command: &str) -> String {
    format!(
        r#"[[gates.review.validators]]
name = "{name}"
type = "script"
command = "{command}"
"#
    )
}

pub fn script_validator_for(gate: &str, name: &str, command: &str) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
"#
    )
}

pub fn script_validator_blocking(name: &str, command: &str, blocking: bool) -> String {
    format!(
        r#"[[gates.review.validators]]
name = "{name}"
type = "script"
command = "{command}"
blocking = {blocking}
"#
    )
}

pub fn script_validator_blocking_for(
    gate: &str,
    name: &str,
    command: &str,
    blocking: bool,
) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
blocking = {blocking}
"#
    )
}

pub fn script_validator_with_run_if(gate: &str, name: &str, command: &str, run_if: &str) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
run_if = "{run_if}"
"#
    )
}

pub fn script_validator_with_warn_codes(
    gate: &str,
    name: &str,
    command: &str,
    codes: &[i32],
) -> String {
    let codes_str: Vec<String> = codes.iter().map(|c| c.to_string()).collect();
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
warn_exit_codes = [{codes}]
"#,
        codes = codes_str.join(", ")
    )
}

pub fn script_validator_with_tags(gate: &str, name: &str, command: &str, tags: &[&str]) -> String {
    let tags_str: Vec<String> = tags.iter().map(|t| format!("\"{t}\"")).collect();
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
tags = [{tags}]
"#,
        tags = tags_str.join(", ")
    )
}

pub fn llm_validator(gate: &str, name: &str, prompt: &str, runtime: &str) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "llm"
prompt = "{prompt}"
runtime = ["{runtime}"]
model = "test-model"
"#
    )
}

pub fn llm_validator_with_system_prompt(
    gate: &str,
    name: &str,
    prompt: &str,
    system_prompt: &str,
    runtime: &str,
) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "llm"
prompt = "{prompt}"
system_prompt = "{system_prompt}"
runtime = ["{runtime}"]
model = "test-model"
"#
    )
}

pub fn runtime_toml(name: &str, base_url: &str) -> String {
    format!(
        r#"[runtimes.{name}]
type = "api"
base_url = "{base_url}"
default_model = "test-model"
"#
    )
}

pub fn parse_verdict(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout).expect("Failed to parse JSON verdict")
}

/// Creates a v0.7 config with top-level validators and gate refs (the format `baton add` targets).
pub fn v06_config_with_validators(validators_toml: &str, gates_toml: &str) -> String {
    format!(
        r#"version = "0.7"

[defaults]
timeout_seconds = 300
blocking = true

{validators_toml}

{gates_toml}
"#
    )
}

/// Creates a temp dir with a git repo, baton.toml, and committed files.
/// Useful for `--diff` tests.
pub fn setup_git_project(toml: &str, files: &[(&str, &str)]) -> TempDir {
    use std::process::Command as StdCommand;
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("baton.toml"), toml).unwrap();
    fs::create_dir_all(dir.path().join(".baton/tmp")).unwrap();
    fs::create_dir_all(dir.path().join(".baton/logs")).unwrap();
    for (name, content) in files {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }
    // Initialize git repo and commit
    StdCommand::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    StdCommand::new("git")
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@test.com",
            "commit",
            "-m",
            "initial",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

pub fn human_validator(gate: &str, name: &str, prompt: &str) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "human"
prompt = "{prompt}"
"#
    )
}

pub fn human_validator_blocking(gate: &str, name: &str, prompt: &str, blocking: bool) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "human"
prompt = "{prompt}"
blocking = {blocking}
"#
    )
}

pub fn script_validator_with_input(
    gate: &str,
    name: &str,
    command: &str,
    input_glob: &str,
) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
input = "{input_glob}"
"#
    )
}

pub fn script_validator_with_batch_input(
    gate: &str,
    name: &str,
    command: &str,
    match_glob: &str,
) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
input = {{ match = "{match_glob}", collect = true }}
"#
    )
}

pub fn script_validator_with_working_dir(
    gate: &str,
    name: &str,
    command: &str,
    dir: &str,
) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
working_dir = "{dir}"
"#
    )
}

pub fn script_validator_with_env(
    gate: &str,
    name: &str,
    command: &str,
    env_pairs: &[(&str, &str)],
) -> String {
    let env_entries: Vec<String> = env_pairs
        .iter()
        .map(|(k, v)| format!("{k} = \"{v}\""))
        .collect();
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "script"
command = "{command}"
env = {{ {env} }}
"#,
        env = env_entries.join(", ")
    )
}

pub fn llm_validator_freeform(gate: &str, name: &str, prompt: &str, runtime: &str) -> String {
    format!(
        r#"[[gates.{gate}.validators]]
name = "{name}"
type = "llm"
prompt = "{prompt}"
runtime = ["{runtime}"]
model = "test-model"
response_format = "freeform"
"#
    )
}

/// A minimal v0.7 config with one script validator and one gate.
pub fn v06_base_config() -> String {
    v06_config_with_validators(
        r#"[validators.existing]
type = "script"
command = "echo existing""#,
        r#"[gates.ci]
description = "CI gate"
validators = [
    { ref = "existing", blocking = true },
]"#,
    )
}
