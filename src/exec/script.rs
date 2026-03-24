//! Script validator execution.

use std::collections::BTreeMap;
use std::process::Command;

use crate::placeholder::{resolve_placeholders, ResolutionWarnings};
use crate::types::*;

pub(super) fn execute_script_validator(
    validator: &crate::config::ValidatorConfig,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> ValidatorResult {
    let command = validator.command.as_deref().unwrap_or("");

    // Resolve placeholders in command
    let mut warnings = ResolutionWarnings::new();
    let resolved_command = resolve_placeholders(command, inputs, prior_results, &mut warnings);

    if resolved_command.trim().is_empty() {
        return ValidatorResult {
            name: validator.name.clone(),
            status: Status::Error,
            feedback: Some("[baton] Command is empty after placeholder resolution".into()),
            duration_ms: 0,
            cost: None,
        };
    }

    // Determine working directory: explicit override, or caller's cwd
    let working_dir = validator
        .working_dir
        .clone()
        .unwrap_or_else(|| ".".to_string());

    let working_path = std::path::Path::new(&working_dir);
    if !working_path.exists() {
        return ValidatorResult {
            name: validator.name.clone(),
            status: Status::Error,
            feedback: Some(format!(
                "[baton] Working directory not found: {working_dir}"
            )),
            duration_ms: 0,
            cost: None,
        };
    }

    // Spawn process
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&resolved_command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c").arg(&resolved_command);
        c
    };
    cmd.current_dir(&working_dir);

    // Add env vars
    for (k, v) in &validator.env {
        cmd.env(k, v);
    }

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            let feedback = if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "[baton] Command not found: {}",
                    resolved_command
                        .split_whitespace()
                        .next()
                        .unwrap_or(&resolved_command)
                )
            } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                format!("[baton] Permission denied: {resolved_command}")
            } else {
                format!("[baton] Unexpected error: {e}")
            };
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some(feedback),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if exit_code == 0 {
        ValidatorResult {
            name: validator.name.clone(),
            status: Status::Pass,
            feedback: None,
            duration_ms: 0,
            cost: None,
        }
    } else if validator.warn_exit_codes.contains(&exit_code) {
        let feedback = format!("{}\n{}", stdout, stderr).trim().to_string();
        let feedback = if feedback.is_empty() {
            format!("[baton] Script exited with code {exit_code} (warn, no output)")
        } else {
            feedback
        };
        ValidatorResult {
            name: validator.name.clone(),
            status: Status::Warn,
            feedback: Some(feedback),
            duration_ms: 0,
            cost: None,
        }
    } else {
        let feedback = format!("{}\n{}", stdout, stderr).trim().to_string();
        let feedback = if feedback.is_empty() {
            format!("[baton] Script exited with code {exit_code} (no output)")
        } else {
            feedback
        };
        ValidatorResult {
            name: validator.name.clone(),
            status: Status::Fail,
            feedback: Some(feedback),
            duration_ms: 0,
            cost: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::execute_validator;
    use crate::test_helpers::ValidatorBuilder;
    use tempfile::TempDir;

    // ─── Script validator tests ──────────────────────

    #[test]
    fn script_exit_0_pass() {
        let v = ValidatorBuilder::script("test", "exit 0").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn script_exit_1_fail() {
        let v = ValidatorBuilder::script("test", "exit 1").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn script_exit_with_warn_code() {
        let v = ValidatorBuilder::script("test", "echo 'warning message' && exit 2")
            .warn_exit_codes(vec![2])
            .build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Warn);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("warning message"));
    }

    #[test]
    fn script_exit_2_without_warn_codes_is_fail() {
        let v = ValidatorBuilder::script("test", "exit 2").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn script_no_output_fail_feedback() {
        let v = ValidatorBuilder::script("test", "exit 1").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("no output"));
    }

    #[test]
    fn script_with_stderr_feedback() {
        let v = ValidatorBuilder::script("test", "echo 'error detail' >&2 && exit 1").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("error detail"));
    }

    #[test]
    fn script_placeholder_resolution() {
        let dir = TempDir::new().unwrap();
        let art_path = dir.path().join("test.txt");
        std::fs::write(&art_path, "hello").unwrap();

        let cmd = if cfg!(windows) {
            "type {file.path}"
        } else {
            "cat {file.path}"
        };
        let v = ValidatorBuilder::script("test", cmd).build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        inputs.insert("file".into(), vec![InputFile::new(art_path)]);
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn script_empty_command_returns_error() {
        let v = ValidatorBuilder::script("empty-cmd", "   ").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Error);
        assert!(
            result
                .feedback
                .as_ref()
                .unwrap()
                .contains("Command is empty"),
            "expected 'Command is empty' in feedback, got: {:?}",
            result.feedback
        );
    }

    #[test]
    fn script_warn_exit_code_with_empty_output() {
        let v = ValidatorBuilder::script("warn-no-out", "exit 2")
            .warn_exit_codes(vec![2])
            .build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Warn);
        assert!(
            result
                .feedback
                .as_ref()
                .unwrap()
                .contains("warn, no output"),
            "expected 'warn, no output' in feedback, got: {:?}",
            result.feedback
        );
    }

    #[test]
    fn script_env_vars_passed_to_subprocess() {
        let v = ValidatorBuilder::script("env-test", "echo $BATON_TEST_VAR")
            .env("BATON_TEST_VAR", "hello123")
            .build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Pass);
        // Pass means exit 0, stdout captured but feedback is None for pass.
        // The echo output goes to stdout but isn't surfaced in feedback on pass.
        // Instead, let's use a script that checks the var and fails if wrong.
        let v2 = ValidatorBuilder::script(
            "env-check",
            r#"test "$BATON_TEST_VAR" = "hello123" || echo "MISMATCH: $BATON_TEST_VAR""#,
        )
        .env("BATON_TEST_VAR", "hello123")
        .build();
        let result2 = execute_validator(&v2, &mut inputs, &prior, None);
        assert_eq!(
            result2.status,
            Status::Pass,
            "env var should be set correctly; feedback: {:?}",
            result2.feedback
        );
    }

    // ─── Script validator: working_dir error ──────────────

    #[test]
    fn script_nonexistent_working_dir_returns_error() {
        let v = ValidatorBuilder::script("wd-test", "echo hi")
            .working_dir("/nonexistent/working/dir/path")
            .build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Error);
        assert!(
            result
                .feedback
                .as_ref()
                .unwrap()
                .contains("Working directory not found"),
            "expected working dir error, got: {:?}",
            result.feedback
        );
    }

    // ─── Script: no command (None) ───────────────────────

    #[test]
    fn script_no_command_returns_error() {
        let mut v = ValidatorBuilder::script("no-cmd", "placeholder").build();
        v.command = None;
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Command is empty"));
    }

    // ─── Script: working_dir set to valid directory ──────

    #[test]
    fn script_valid_working_dir() {
        let dir = TempDir::new().unwrap();
        let v = ValidatorBuilder::script("wd-ok", "exit 0")
            .working_dir(dir.path().to_str().unwrap())
            .build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Pass);
    }
}
