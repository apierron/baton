use chrono::Utc;
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;

use crate::config::{
    split_run_if, BatonConfig, GateConfig, LlmMode, ValidatorConfig, ValidatorType,
};
use crate::error::{BatonError, Result};
use crate::placeholder::{resolve_placeholders, ResolutionWarnings};
use crate::types::*;

// ─── run_if evaluation ──────────────────────────────────

/// Evaluate a run_if expression against prior validator results.
pub fn evaluate_run_if(
    expr: &str,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> Result<bool> {
    let tokens = split_run_if(expr);

    if tokens.is_empty() {
        return Err(BatonError::ValidationError(
            "Empty run_if expression".into()
        ));
    }

    // Evaluate first atom
    let mut current_result = evaluate_atom(&tokens[0], prior_results)?;

    // Evaluate remaining atoms left-to-right, no precedence, NO short-circuit.
    let mut i = 1;
    while i < tokens.len() {
        if i + 1 >= tokens.len() {
            return Err(BatonError::ValidationError(format!(
                "Invalid run_if expression: '{expr}'"
            )));
        }
        let operator = &tokens[i];
        let next_result = evaluate_atom(&tokens[i + 1], prior_results)?;

        match operator.as_str() {
            "and" => current_result = current_result && next_result,
            "or" => current_result = current_result || next_result,
            other => {
                return Err(BatonError::ValidationError(format!(
                    "Invalid operator in run_if: '{other}'. Expected 'and' or 'or'."
                )));
            }
        }

        i += 2;
    }

    Ok(current_result)
}

fn evaluate_atom(
    atom: &str,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> Result<bool> {
    let parts: Vec<&str> = atom.split(".status == ").collect();
    if parts.len() != 2 {
        return Err(BatonError::ValidationError(format!(
            "Invalid run_if expression: '{atom}'. Expected '<name>.status == <value>'"
        )));
    }

    let validator_name = parts[0].trim();
    let expected_status = parts[1].trim();

    let expected: Status = expected_status.parse().map_err(|_| {
        BatonError::ValidationError(format!(
            "Invalid status in run_if: '{expected_status}'"
        ))
    })?;

    match prior_results.get(validator_name) {
        Some(result) => Ok(result.status == expected),
        None => Ok(expected == Status::Skip),
    }
}

// ─── Compute final status ────────────────────────────────

pub fn compute_final_status(
    results: &[ValidatorResult],
    suppressed: &[Status],
) -> VerdictStatus {
    let effective: Vec<Status> = results
        .iter()
        .filter(|r| r.status != Status::Skip)
        .map(|r| {
            if suppressed.contains(&r.status) {
                Status::Pass
            } else {
                r.status
            }
        })
        .collect();

    if effective.contains(&Status::Error) {
        VerdictStatus::Error
    } else if effective.contains(&Status::Fail) {
        VerdictStatus::Fail
    } else {
        VerdictStatus::Pass
    }
}

// ─── Execute individual validators ──────────────────────

fn execute_script_validator(
    validator: &ValidatorConfig,
    artifact: &mut Artifact,
    context: &mut Context,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> ValidatorResult {
    let command = validator.command.as_deref().unwrap_or("");

    // Resolve placeholders in command
    let mut warnings = ResolutionWarnings::new();
    let resolved_command =
        resolve_placeholders(command, artifact, context, prior_results, &mut warnings);

    if resolved_command.trim().is_empty() {
        return ValidatorResult {
            name: validator.name.clone(),
            status: Status::Error,
            feedback: Some("[baton] Command is empty after placeholder resolution".into()),
            duration_ms: 0,
            cost: None,
        };
    }

    // Determine working directory
    let working_dir = validator
        .working_dir
        .clone()
        .or_else(|| artifact.parent_dir())
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
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(&resolved_command).current_dir(&working_dir);

    // Add env vars
    for (k, v) in &validator.env {
        cmd.env(k, v);
    }

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            let feedback = if e.kind() == std::io::ErrorKind::NotFound {
                format!("[baton] Command not found: {}", resolved_command.split_whitespace().next().unwrap_or(&resolved_command))
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

fn execute_human_validator(
    validator: &ValidatorConfig,
    artifact: &mut Artifact,
    context: &mut Context,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> ValidatorResult {
    let prompt = validator.prompt.as_deref().unwrap_or("");
    let mut warnings = ResolutionWarnings::new();
    let rendered =
        resolve_placeholders(prompt, artifact, context, prior_results, &mut warnings);

    ValidatorResult {
        name: validator.name.clone(),
        status: Status::Fail,
        feedback: Some(format!("[human-review-requested] {rendered}")),
        duration_ms: 0,
        cost: None,
    }
}

/// Execute a single validator (dispatch by type).
pub fn execute_validator(
    validator: &ValidatorConfig,
    artifact: &mut Artifact,
    context: &mut Context,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> ValidatorResult {
    let start = Instant::now();

    // Evaluate run_if
    if let Some(ref run_if_expr) = validator.run_if {
        match evaluate_run_if(run_if_expr, prior_results) {
            Ok(true) => {} // proceed
            Ok(false) => {
                return ValidatorResult {
                    name: validator.name.clone(),
                    status: Status::Skip,
                    feedback: None,
                    duration_ms: 0,
                    cost: None,
                };
            }
            Err(e) => {
                return ValidatorResult {
                    name: validator.name.clone(),
                    status: Status::Error,
                    feedback: Some(format!("[baton] run_if evaluation error: {e}")),
                    duration_ms: 0,
                    cost: None,
                };
            }
        }
    }

    let mut result = match validator.validator_type {
        ValidatorType::Script => {
            execute_script_validator(validator, artifact, context, prior_results)
        }
        ValidatorType::Human => {
            execute_human_validator(validator, artifact, context, prior_results)
        }
        ValidatorType::Llm => {
            // LLM validators are stubbed for now - they require HTTP calls
            match validator.mode {
                LlmMode::Completion => execute_llm_stub(validator),
                LlmMode::Session => execute_llm_stub(validator),
            }
        }
    };

    result.duration_ms = start.elapsed().as_millis() as i64;
    result
}

fn execute_llm_stub(validator: &ValidatorConfig) -> ValidatorResult {
    // Placeholder for LLM execution - requires HTTP client
    ValidatorResult {
        name: validator.name.clone(),
        status: Status::Error,
        feedback: Some("[baton] LLM validators not yet implemented".into()),
        duration_ms: 0,
        cost: None,
    }
}

// ─── Gate run ────────────────────────────────────────────

pub fn run_gate(
    gate: &GateConfig,
    _config: &BatonConfig,
    artifact: &mut Artifact,
    context: &mut Context,
    options: &RunOptions,
) -> Result<Verdict> {
    let run_start = Instant::now();

    // ── Pre-flight checks ──

    // Validate artifact exists (if file-backed)
    if let Some(ref path) = artifact.path {
        if !path.exists() {
            return Err(BatonError::ArtifactNotFound(path.display().to_string()));
        }
        if path.is_dir() {
            return Err(BatonError::ArtifactIsDirectory(path.display().to_string()));
        }
    }

    // Validate required context
    for (slot_name, slot) in &gate.context {
        if slot.required && !context.items.contains_key(slot_name) {
            return Err(BatonError::MissingRequiredContext {
                name: slot_name.clone(),
                gate: gate.name.clone(),
            });
        }
    }

    // Warn on unexpected context
    for item_name in context.items.keys() {
        if !gate.context.contains_key(item_name) {
            eprintln!(
                "warning: Unknown context item '{item_name}' for gate '{}' — ignored",
                gate.name
            );
        }
    }

    // Validate context paths
    for (item_name, item) in &context.items {
        if let Some(ref path) = item.path {
            if !path.exists() {
                return Err(BatonError::ContextNotFound {
                    name: item_name.clone(),
                    path: path.display().to_string(),
                });
            }
            if path.is_dir() {
                return Err(BatonError::ContextIsDirectory {
                    name: item_name.clone(),
                    path: path.display().to_string(),
                });
            }
        }
    }

    // Compute hashes
    let artifact_hash = artifact.get_hash()?;
    let context_hash = context.get_hash()?;

    // Run validators
    let mut results: BTreeMap<String, ValidatorResult> = BTreeMap::new();
    let mut warnings_list: Vec<String> = Vec::new();

    let suppressed = &options.suppressed_statuses;

    for validator in &gate.validators {
        // Apply filters
        if let Some(ref only) = options.only {
            if !only.contains(&validator.name) {
                results.insert(
                    validator.name.clone(),
                    ValidatorResult {
                        name: validator.name.clone(),
                        status: Status::Skip,
                        feedback: None,
                        duration_ms: 0,
                        cost: None,
                    },
                );
                continue;
            }
        }
        if let Some(ref skip) = options.skip {
            if skip.contains(&validator.name) {
                results.insert(
                    validator.name.clone(),
                    ValidatorResult {
                        name: validator.name.clone(),
                        status: Status::Skip,
                        feedback: None,
                        duration_ms: 0,
                        cost: None,
                    },
                );
                continue;
            }
        }
        if let Some(ref tags) = options.tags {
            if !validator.tags.iter().any(|t| tags.contains(t)) {
                results.insert(
                    validator.name.clone(),
                    ValidatorResult {
                        name: validator.name.clone(),
                        status: Status::Skip,
                        feedback: None,
                        duration_ms: 0,
                        cost: None,
                    },
                );
                continue;
            }
        }

        let result = execute_validator(validator, artifact, context, &results);
        results.insert(validator.name.clone(), result.clone());

        if result.status == Status::Warn {
            warnings_list.push(validator.name.clone());
        }

        // Determine effective status (apply suppression)
        let effective_status = if suppressed.contains(&result.status) {
            Status::Pass
        } else {
            result.status
        };

        if (effective_status == Status::Fail || effective_status == Status::Error)
            && validator.blocking
            && !options.run_all
        {
            let verdict_status = match effective_status {
                Status::Error => VerdictStatus::Error,
                Status::Fail => VerdictStatus::Fail,
                _ => unreachable!(),
            };
            return Ok(Verdict {
                status: verdict_status,
                gate: gate.name.clone(),
                failed_at: Some(validator.name.clone()),
                feedback: result.feedback.clone(),
                duration_ms: run_start.elapsed().as_millis() as i64,
                timestamp: Utc::now(),
                artifact_hash,
                context_hash,
                warnings: warnings_list,
                suppressed: suppressed.iter().map(|s| s.to_string()).collect(),
                history: results.values().cloned().collect(),
            });
        }
    }

    // Compute final status
    let result_list: Vec<ValidatorResult> = results.values().cloned().collect();
    let final_status = if options.run_all {
        compute_final_status(&result_list, suppressed)
    } else {
        // In normal mode, if we reach here, no blocking validator failed/errored
        VerdictStatus::Pass
    };

    let (failed_at, feedback) = if final_status == VerdictStatus::Fail
        || final_status == VerdictStatus::Error
    {
        let target_status = match final_status {
            VerdictStatus::Error => Status::Error,
            VerdictStatus::Fail => Status::Fail,
            _ => unreachable!(),
        };
        let first = result_list.iter().find(|r| r.status == target_status);
        (
            first.map(|r| r.name.clone()),
            first.and_then(|r| r.feedback.clone()),
        )
    } else {
        (None, None)
    };

    Ok(Verdict {
        status: final_status,
        gate: gate.name.clone(),
        failed_at,
        feedback,
        duration_ms: run_start.elapsed().as_millis() as i64,
        timestamp: Utc::now(),
        artifact_hash,
        context_hash,
        warnings: warnings_list,
        suppressed: suppressed.iter().map(|s| s.to_string()).collect(),
        history: result_list,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use tempfile::TempDir;

    // ─── run_if evaluation ───────────────────────────

    fn make_results() -> BTreeMap<String, ValidatorResult> {
        let mut map = BTreeMap::new();
        map.insert(
            "lint".into(),
            ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: None,
                duration_ms: 0,
                cost: None,
            },
        );
        map.insert(
            "typecheck".into(),
            ValidatorResult {
                name: "typecheck".into(),
                status: Status::Fail,
                feedback: Some("error".into()),
                duration_ms: 0,
                cost: None,
            },
        );
        map
    }

    #[test]
    fn run_if_simple_pass() {
        let results = make_results();
        assert!(evaluate_run_if("lint.status == pass", &results).unwrap());
    }

    #[test]
    fn run_if_simple_fail() {
        let results = make_results();
        assert!(!evaluate_run_if("lint.status == fail", &results).unwrap());
    }

    #[test]
    fn run_if_and_both_true() {
        let results = make_results();
        assert!(!evaluate_run_if(
            "lint.status == pass and typecheck.status == pass",
            &results
        )
        .unwrap());
    }

    #[test]
    fn run_if_or_one_true() {
        let results = make_results();
        assert!(evaluate_run_if(
            "lint.status == fail or typecheck.status == fail",
            &results
        )
        .unwrap());
    }

    #[test]
    fn run_if_left_to_right_no_precedence() {
        // "a or b and c" → "(a or b) and c"
        let mut results = BTreeMap::new();
        results.insert("a".into(), ValidatorResult { name: "a".into(), status: Status::Pass, feedback: None, duration_ms: 0, cost: None });
        results.insert("b".into(), ValidatorResult { name: "b".into(), status: Status::Fail, feedback: None, duration_ms: 0, cost: None });
        results.insert("c".into(), ValidatorResult { name: "c".into(), status: Status::Fail, feedback: None, duration_ms: 0, cost: None });

        // a.pass or b.pass → true or false → true
        // true and c.pass → true and false → false
        let result = evaluate_run_if(
            "a.status == pass or b.status == pass and c.status == pass",
            &results,
        )
        .unwrap();
        assert!(!result);
    }

    #[test]
    fn run_if_skipped_validator() {
        let mut results = BTreeMap::new();
        results.insert(
            "a".into(),
            ValidatorResult {
                name: "a".into(),
                status: Status::Skip,
                feedback: None,
                duration_ms: 0,
                cost: None,
            },
        );
        assert!(evaluate_run_if("a.status == skip", &results).unwrap());
    }

    #[test]
    fn run_if_nonexistent_treated_as_skip() {
        let results = BTreeMap::new();
        assert!(evaluate_run_if("nonexistent.status == skip", &results).unwrap());
        assert!(!evaluate_run_if("nonexistent.status == pass", &results).unwrap());
    }

    #[test]
    fn run_if_invalid_expression() {
        let results = BTreeMap::new();
        assert!(evaluate_run_if("invalid expression", &results).is_err());
    }

    // ─── compute_final_status ────────────────────────

    #[test]
    fn final_status_all_pass() {
        let results = vec![
            ValidatorResult { name: "a".into(), status: Status::Pass, feedback: None, duration_ms: 0, cost: None },
            ValidatorResult { name: "b".into(), status: Status::Pass, feedback: None, duration_ms: 0, cost: None },
        ];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_with_warn() {
        let results = vec![
            ValidatorResult { name: "a".into(), status: Status::Pass, feedback: None, duration_ms: 0, cost: None },
            ValidatorResult { name: "b".into(), status: Status::Warn, feedback: None, duration_ms: 0, cost: None },
        ];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_with_fail() {
        let results = vec![
            ValidatorResult { name: "a".into(), status: Status::Pass, feedback: None, duration_ms: 0, cost: None },
            ValidatorResult { name: "b".into(), status: Status::Fail, feedback: None, duration_ms: 0, cost: None },
        ];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Fail);
    }

    #[test]
    fn final_status_error_beats_fail() {
        let results = vec![
            ValidatorResult { name: "a".into(), status: Status::Fail, feedback: None, duration_ms: 0, cost: None },
            ValidatorResult { name: "b".into(), status: Status::Error, feedback: None, duration_ms: 0, cost: None },
        ];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Error);
    }

    #[test]
    fn final_status_skip_ignored() {
        let results = vec![
            ValidatorResult { name: "a".into(), status: Status::Skip, feedback: None, duration_ms: 0, cost: None },
            ValidatorResult { name: "b".into(), status: Status::Pass, feedback: None, duration_ms: 0, cost: None },
        ];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_suppress_errors() {
        let results = vec![
            ValidatorResult { name: "a".into(), status: Status::Error, feedback: None, duration_ms: 0, cost: None },
            ValidatorResult { name: "b".into(), status: Status::Fail, feedback: None, duration_ms: 0, cost: None },
        ];
        assert_eq!(
            compute_final_status(&results, &[Status::Error]),
            VerdictStatus::Fail
        );
    }

    #[test]
    fn final_status_suppress_all() {
        let results = vec![
            ValidatorResult { name: "a".into(), status: Status::Error, feedback: None, duration_ms: 0, cost: None },
            ValidatorResult { name: "b".into(), status: Status::Fail, feedback: None, duration_ms: 0, cost: None },
        ];
        assert_eq!(
            compute_final_status(&results, &[Status::Error, Status::Fail, Status::Warn]),
            VerdictStatus::Pass
        );
    }

    // ─── Script validator tests ──────────────────────

    fn make_script_validator(name: &str, command: &str) -> ValidatorConfig {
        ValidatorConfig {
            name: name.into(),
            validator_type: ValidatorType::Script,
            blocking: true,
            run_if: None,
            timeout_seconds: 300,
            tags: vec![],
            command: Some(command.into()),
            warn_exit_codes: vec![],
            working_dir: None,
            env: BTreeMap::new(),
            mode: LlmMode::Completion,
            provider: "default".into(),
            model: None,
            prompt: None,
            context_refs: vec![],
            temperature: 0.0,
            response_format: ResponseFormat::Verdict,
            max_tokens: None,
            system_prompt: None,
            runtime: None,
            sandbox: None,
            max_iterations: None,
        }
    }

    #[test]
    fn script_exit_0_pass() {
        let v = make_script_validator("test", "exit 0");
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn script_exit_1_fail() {
        let v = make_script_validator("test", "exit 1");
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn script_exit_with_warn_code() {
        let mut v = make_script_validator("test", "echo 'warning message' && exit 2");
        v.warn_exit_codes = vec![2];
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Warn);
        assert!(result.feedback.as_ref().unwrap().contains("warning message"));
    }

    #[test]
    fn script_exit_2_without_warn_codes_is_fail() {
        let v = make_script_validator("test", "exit 2");
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn script_no_output_fail_feedback() {
        let v = make_script_validator("test", "exit 1");
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("no output"));
    }

    #[test]
    fn script_with_stderr_feedback() {
        let v = make_script_validator("test", "echo 'error detail' >&2 && exit 1");
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("error detail"));
    }

    #[test]
    fn script_placeholder_resolution() {
        let dir = TempDir::new().unwrap();
        let art_path = dir.path().join("test.txt");
        std::fs::write(&art_path, "hello").unwrap();

        let v = make_script_validator("test", "cat {artifact}");
        let mut art = Artifact::from_file(&art_path).unwrap();
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Pass);
    }

    // ─── Human validator tests ───────────────────────

    #[test]
    fn human_validator_fails_with_prompt() {
        let mut v = make_script_validator("human", "");
        v.validator_type = ValidatorType::Human;
        v.prompt = Some("Please review this change.".into());
        v.command = None;

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("[human-review-requested]"));
        assert!(result.feedback.as_ref().unwrap().contains("Please review"));
    }

    // ─── Gate run tests ──────────────────────────────

    fn make_test_config(gate: GateConfig) -> BatonConfig {
        let mut gates = BTreeMap::new();
        gates.insert(gate.name.clone(), gate);
        BatonConfig {
            version: "0.4".into(),
            defaults: Defaults {
                timeout_seconds: 300,
                blocking: true,
                prompts_dir: "/tmp/prompts".into(),
                log_dir: "/tmp/logs".into(),
                history_db: "/tmp/history.db".into(),
                tmp_dir: "/tmp/tmp".into(),
            },
            providers: BTreeMap::new(),
            runtimes: BTreeMap::new(),
            gates,
            config_dir: "/tmp".into(),
        }
    }

    fn make_gate(name: &str, validators: Vec<ValidatorConfig>) -> GateConfig {
        GateConfig {
            name: name.into(),
            description: None,
            context: BTreeMap::new(),
            validators,
        }
    }

    #[test]
    fn gate_all_pass() {
        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                make_script_validator("b", "exit 0"),
                make_script_validator("c", "exit 0"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.history.len(), 3);
    }

    #[test]
    fn gate_first_fail_blocks() {
        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                make_script_validator("b", "exit 1"),
                make_script_validator("c", "exit 0"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        assert_eq!(verdict.failed_at, Some("b".into()));
        // c should not have run
        assert_eq!(verdict.history.len(), 2);
    }

    #[test]
    fn gate_non_blocking_failure_passes() {
        let mut fail_v = make_script_validator("b", "exit 1");
        fail_v.blocking = false;

        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                fail_v,
                make_script_validator("c", "exit 0"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // All 3 ran
        assert_eq!(verdict.history.len(), 3);
    }

    #[test]
    fn gate_all_mode_runs_everything() {
        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                make_script_validator("b", "exit 1"),
                make_script_validator("c", "exit 0"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        assert_eq!(verdict.history.len(), 3);
    }

    #[test]
    fn gate_all_mode_non_blocking_failure_counts() {
        let mut fail_v = make_script_validator("b", "exit 1");
        fail_v.blocking = false;

        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                fail_v,
                make_script_validator("c", "exit 0"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn gate_conditional_skip() {
        let mut cond_v = make_script_validator("b", "exit 0");
        cond_v.run_if = Some("a.status == pass".into());

        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 1"),
                cond_v,
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        // b should be skipped because a failed
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Skip);
    }

    #[test]
    fn gate_warn_from_script() {
        let mut v = make_script_validator("check", "exit 2");
        v.warn_exit_codes = vec![2];

        let gate = make_gate("test", vec![v]);
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.warnings, vec!["check"]);
    }

    #[test]
    fn gate_only_filter() {
        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                make_script_validator("b", "exit 0"),
                make_script_validator("c", "exit 0"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.only = Some(vec!["b".into()]);

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Pass);
        let a_result = verdict.history.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a_result.status, Status::Skip);
    }

    #[test]
    fn gate_skip_filter() {
        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                make_script_validator("b", "exit 0"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.skip = Some(vec!["a".into()]);

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        let a_result = verdict.history.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a_result.status, Status::Skip);
    }

    #[test]
    fn gate_tags_filter() {
        let mut v1 = make_script_validator("fast", "exit 0");
        v1.tags = vec!["quick".into()];
        let mut v2 = make_script_validator("slow", "exit 0");
        v2.tags = vec!["deep".into()];

        let gate = make_gate("test", vec![v1, v2]);
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.tags = Some(vec!["quick".into()]);

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        let fast = verdict.history.iter().find(|r| r.name == "fast").unwrap();
        assert_eq!(fast.status, Status::Pass);
        let slow = verdict.history.iter().find(|r| r.name == "slow").unwrap();
        assert_eq!(slow.status, Status::Skip);
    }

    #[test]
    fn gate_suppress_errors() {
        let gate = make_gate(
            "test",
            vec![
                make_script_validator("a", "exit 0"),
                // This will fail, not error, but let's test with a validator that errors
                make_script_validator("b", "exit 1"),
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Fail];

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        // Fail is suppressed, so pipeline continues and gate passes
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // But history still records the true status
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Fail);
    }

    #[test]
    fn gate_required_context_missing() {
        let mut gate = make_gate("test", vec![make_script_validator("a", "exit 0")]);
        gate.context.insert(
            "spec".into(),
            ContextSlot {
                description: None,
                required: true,
            },
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let result = run_gate(&gate, &config, &mut art, &mut ctx, &opts);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Missing required context"));
    }

    #[test]
    fn gate_error_vs_fail_in_all_mode() {
        // Use a command that will error (nonexistent working dir)
        let mut err_v = make_script_validator("error-v", "exit 0");
        err_v.working_dir = Some("/nonexistent/dir".into());

        let gate = make_gate(
            "test",
            vec![
                make_script_validator("fail-v", "exit 1"),
                err_v,
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        // Error takes precedence over fail
        assert_eq!(verdict.status, VerdictStatus::Error);
    }

    #[test]
    fn gate_suppress_errors_with_all_mode() {
        let mut err_v = make_script_validator("error-v", "exit 0");
        err_v.working_dir = Some("/nonexistent/dir".into());

        let gate = make_gate(
            "test",
            vec![
                make_script_validator("fail-v", "exit 1"),
                err_v,
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;
        opts.suppressed_statuses = vec![Status::Error];

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        // Error suppressed, fail remains
        assert_eq!(verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn gate_suppress_all_in_all_mode() {
        let mut err_v = make_script_validator("error-v", "exit 0");
        err_v.working_dir = Some("/nonexistent/dir".into());

        let gate = make_gate(
            "test",
            vec![
                make_script_validator("fail-v", "exit 1"),
                err_v,
            ],
        );
        let config = make_test_config(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;
        opts.suppressed_statuses = vec![Status::Error, Status::Fail, Status::Warn];

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
    }
}
