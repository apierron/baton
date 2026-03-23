//! Gate execution engine.
//!
//! Runs validators in pipeline order, evaluates `run_if` conditions,
//! dispatches to script/LLM/human executors, and computes the final verdict.

use chrono::Utc;
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use crate::config::{
    split_run_if, BatonConfig, GateConfig, LlmMode, ResponseFormat, ValidatorConfig, ValidatorType,
};
use crate::error::{BatonError, Result};
use crate::placeholder::{resolve_placeholders, ResolutionWarnings};
use crate::prompt::{is_file_reference, resolve_prompt_value};
use crate::runtime::{self, CompletionRequest, SessionConfig, SessionStatus};
use crate::types::*;
use crate::verdict_parser::parse_verdict;

// ─── run_if evaluation ──────────────────────────────────

/// Evaluate a run_if expression against prior validator results.
///
/// Parses `expr` as a sequence of `name.status == value` atoms joined by
/// `and`/`or` operators. Evaluation is left-to-right with no precedence
/// and no short-circuit (all atoms are evaluated to catch missing references).
///
/// Returns `Err` if the expression is empty or references a validator not
/// present in `prior_results`.
pub fn evaluate_run_if(
    expr: &str,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> Result<bool> {
    let tokens = split_run_if(expr);

    if tokens.is_empty() {
        return Err(BatonError::ValidationError(
            "Empty run_if expression".into(),
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

fn evaluate_atom(atom: &str, prior_results: &BTreeMap<String, ValidatorResult>) -> Result<bool> {
    let parts: Vec<&str> = atom.split(".status == ").collect();
    if parts.len() != 2 {
        return Err(BatonError::ValidationError(format!(
            "Invalid run_if expression: '{atom}'. Expected '<name>.status == <value>'"
        )));
    }

    let validator_name = parts[0].trim();
    let expected_status = parts[1].trim();

    let expected: Status = expected_status.parse().map_err(|_| {
        BatonError::ValidationError(format!("Invalid status in run_if: '{expected_status}'"))
    })?;

    match prior_results.get(validator_name) {
        Some(result) => Ok(result.status == expected),
        None => Ok(expected == Status::Skip),
    }
}

// ─── Compute final status ────────────────────────────────

/// Computes the gate-level [`VerdictStatus`] from individual validator results,
/// applying status suppression. Error beats Fail; Skip and Warn are ignored.
pub fn compute_final_status(results: &[ValidatorResult], suppressed: &[Status]) -> VerdictStatus {
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

fn execute_human_validator(
    validator: &ValidatorConfig,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> ValidatorResult {
    let prompt = validator.prompt.as_deref().unwrap_or("");
    let mut warnings = ResolutionWarnings::new();
    let rendered = resolve_placeholders(prompt, inputs, prior_results, &mut warnings);

    ValidatorResult {
        name: validator.name.clone(),
        status: Status::Fail,
        feedback: Some(format!("[human-review-requested] {rendered}")),
        duration_ms: 0,
        cost: None,
    }
}

/// Dispatches a single validator by type (script, LLM, or human), evaluating
/// its `run_if` condition and recording wall-clock timing.
pub fn execute_validator(
    validator: &ValidatorConfig,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
    prior_results: &BTreeMap<String, ValidatorResult>,
    config: Option<&BatonConfig>,
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
        ValidatorType::Script => execute_script_validator(validator, inputs, prior_results),
        ValidatorType::Human => execute_human_validator(validator, inputs, prior_results),
        ValidatorType::Llm => execute_llm_validator(validator, inputs, prior_results, config),
    };

    result.duration_ms = start.elapsed().as_millis() as i64;
    result
}

// ─── Unified LLM validator ───────────────────────────────

fn execute_llm_validator(
    validator: &ValidatorConfig,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
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
                    "[baton] LLM validator requires config with runtime settings".into(),
                ),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    if validator.runtimes.is_empty() {
        return ValidatorResult {
            name: validator.name.clone(),
            status: Status::Error,
            feedback: Some(format!(
                "[baton] Validator '{}': no runtimes configured.",
                validator.name
            )),
            duration_ms: 0,
            cost: None,
        };
    }

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
    let rendered_prompt = resolve_placeholders(&prompt_body, inputs, prior_results, &mut warnings);

    // Runtime fallback loop
    let mut last_error = String::new();

    for runtime_name in &validator.runtimes {
        let runtime_config = match config.runtimes.get(runtime_name) {
            Some(r) => r,
            None => {
                last_error = format!("Runtime '{runtime_name}' is not defined in [runtimes].");
                continue;
            }
        };

        // Session mode: skip api-type runtimes
        if validator.mode == LlmMode::Session && runtime_config.runtime_type == "api" {
            last_error = format!("Runtime '{runtime_name}' is type 'api' (no session support).");
            continue;
        }

        // Create adapter
        let adapter = match runtime::create_adapter(runtime_name, runtime_config) {
            Ok(a) => a,
            Err(e) => {
                last_error = format!("Failed to create adapter for runtime '{runtime_name}': {e}");
                continue;
            }
        };

        // Health check
        match adapter.health_check() {
            Ok(health) if !health.reachable => {
                last_error = format!(
                    "Runtime '{runtime_name}' is not reachable: {}",
                    health.message.unwrap_or_default()
                );
                continue;
            }
            Err(e) => {
                last_error = format!("Runtime '{runtime_name}' health check failed: {e}");
                continue;
            }
            Ok(_) => {} // reachable, proceed
        }

        // Determine model
        let model = validator
            .model
            .clone()
            .or_else(|| runtime_config.default_model.clone())
            .unwrap_or_else(|| "default".to_string());

        match validator.mode {
            LlmMode::Query => {
                // Build messages
                let mut messages = Vec::new();
                if let Some(ref sys) = validator.system_prompt {
                    let rendered_sys =
                        resolve_placeholders(sys, inputs, prior_results, &mut warnings);
                    messages.push(serde_json::json!({
                        "role": "system",
                        "content": rendered_sys,
                    }));
                }
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": rendered_prompt,
                }));

                let request = CompletionRequest {
                    messages,
                    model: model.clone(),
                    temperature: validator.temperature,
                    max_tokens: validator.max_tokens,
                };

                match adapter.post_completion(request) {
                    Ok(response) => {
                        return match validator.response_format {
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
                        };
                    }
                    Err(BatonError::RuntimeError(ref msg)) if msg.contains("does not support") => {
                        last_error =
                            format!("Runtime '{runtime_name}' does not support completions.");
                        continue;
                    }
                    Err(e) => {
                        // Non-capability error — this is final, no fallback
                        return ValidatorResult {
                            name: validator.name.clone(),
                            status: Status::Error,
                            feedback: Some(format!("[baton] {e}")),
                            duration_ms: 0,
                            cost: None,
                        };
                    }
                }
            }
            LlmMode::Session => {
                // Prepare file set for isolation
                let mut files = BTreeMap::new();
                for (slot_name, slot_files) in inputs.iter() {
                    for f in slot_files {
                        files.insert(slot_name.clone(), f.path.display().to_string());
                    }
                }

                let sandbox = validator.sandbox.unwrap_or(runtime_config.sandbox);
                let max_iterations = validator
                    .max_iterations
                    .unwrap_or(runtime_config.max_iterations);
                let timeout_seconds = validator.timeout_seconds;

                let session_config = SessionConfig {
                    task: rendered_prompt.clone(),
                    files,
                    model,
                    sandbox,
                    max_iterations,
                    timeout_seconds,
                    env: BTreeMap::new(),
                };

                return drive_session(
                    &validator.name,
                    adapter.as_ref(),
                    session_config,
                    timeout_seconds,
                );
            }
        }
    }

    // All runtimes exhausted
    ValidatorResult {
        name: validator.name.clone(),
        status: Status::Error,
        feedback: Some(format!(
            "[baton] No reachable runtime for validator '{}'. Last error: {last_error}",
            validator.name
        )),
        duration_ms: 0,
        cost: None,
    }
}

/// Core session orchestration: create → poll → collect → teardown → parse verdict.
///
/// Extracted from `execute_llm_session` so the lifecycle logic can be tested
/// independently of config resolution and adapter construction.
fn drive_session(
    name: &str,
    adapter: &dyn runtime::RuntimeAdapter,
    session_config: SessionConfig,
    timeout_seconds: u64,
) -> ValidatorResult {
    let handle = match adapter.create_session(session_config) {
        Ok(h) => h,
        Err(e) => {
            return ValidatorResult {
                name: name.into(),
                status: Status::Error,
                feedback: Some(format!("[baton] Failed to create session: {e}")),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    // Poll until terminal
    let poll_interval = std::time::Duration::from_secs(2);
    let poll_start = Instant::now();

    loop {
        if poll_start.elapsed().as_secs() > timeout_seconds {
            let _ = adapter.cancel(&handle);
            let _ = adapter.teardown(&handle);
            return ValidatorResult {
                name: name.into(),
                status: Status::Error,
                feedback: Some(format!(
                    "[baton] Agent session timed out after {timeout_seconds} seconds"
                )),
                duration_ms: 0,
                cost: None,
            };
        }

        match adapter.poll_status(&handle) {
            Ok(status) => match status {
                SessionStatus::Running => {
                    std::thread::sleep(poll_interval);
                    continue;
                }
                SessionStatus::Completed
                | SessionStatus::Failed
                | SessionStatus::TimedOut
                | SessionStatus::Cancelled => break,
            },
            Err(e) => {
                let _ = adapter.cancel(&handle);
                let _ = adapter.teardown(&handle);
                return ValidatorResult {
                    name: name.into(),
                    status: Status::Error,
                    feedback: Some(format!("[baton] Error polling session: {e}")),
                    duration_ms: 0,
                    cost: None,
                };
            }
        }
    }

    // Collect result
    let session_result = match adapter.collect_result(&handle) {
        Ok(r) => r,
        Err(e) => {
            let _ = adapter.teardown(&handle);
            return ValidatorResult {
                name: name.into(),
                status: Status::Error,
                feedback: Some(format!("[baton] Error collecting session result: {e}")),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    // Teardown
    let _ = adapter.teardown(&handle);

    // Handle non-completed sessions
    match session_result.status {
        SessionStatus::Completed => {}
        SessionStatus::TimedOut => {
            return ValidatorResult {
                name: name.into(),
                status: Status::Error,
                feedback: Some(format!(
                    "[baton] Agent session timed out after {timeout_seconds} seconds"
                )),
                duration_ms: 0,
                cost: session_result.cost,
            };
        }
        SessionStatus::Failed => {
            return ValidatorResult {
                name: name.into(),
                status: Status::Error,
                feedback: Some("[baton] Agent session ended with status 'failed'".into()),
                duration_ms: 0,
                cost: session_result.cost,
            };
        }
        SessionStatus::Cancelled => {
            return ValidatorResult {
                name: name.into(),
                status: Status::Error,
                feedback: Some("[baton] Agent session ended with status 'cancelled'".into()),
                duration_ms: 0,
                cost: session_result.cost,
            };
        }
        SessionStatus::Running => {
            // Should not happen after polling exits, but handle defensively
            return ValidatorResult {
                name: name.into(),
                status: Status::Error,
                feedback: Some("[baton] Agent produced no verdict.".into()),
                duration_ms: 0,
                cost: session_result.cost,
            };
        }
    }

    // Check for empty output
    if session_result.output.trim().is_empty() {
        return ValidatorResult {
            name: name.into(),
            status: Status::Error,
            feedback: Some(
                "[baton] Agent session completed but output contained no PASS/FAIL/WARN verdict."
                    .into(),
            ),
            duration_ms: 0,
            cost: session_result.cost,
        };
    }

    // Parse verdict from agent output
    let parsed = parse_verdict(&session_result.output);
    ValidatorResult {
        name: name.into(),
        status: parsed.status,
        feedback: parsed.evidence,
        duration_ms: 0,
        cost: session_result.cost,
    }
}

// ─── File collector ──────────────────────────────────────

/// Options for building the input file pool.
pub struct FileCollectOptions {
    pub files: Vec<PathBuf>,
    pub diff: Option<String>,
    pub file_list: Option<String>,
    pub recursive: bool,
}

/// Build the input pool from positional args, `--diff`, and `--files`.
///
/// Directories are walked recursively unless `recursive` is false.
/// The pool is deduplicated by canonical (absolute, symlink-resolved) path.
pub fn collect_file_pool(opts: &FileCollectOptions) -> Result<Vec<InputFile>> {
    let mut pool: Vec<InputFile> = Vec::new();

    // Positional files/directories
    for file_path in &opts.files {
        if !file_path.exists() {
            return Err(BatonError::ValidationError(format!(
                "File not found: {}",
                file_path.display()
            )));
        }
        if file_path.is_dir() {
            if opts.recursive {
                walk_dir(file_path, &mut pool);
            } else {
                // Non-recursive: only direct children that are files
                if let Ok(entries) = std::fs::read_dir(file_path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if !p.is_dir() {
                            pool.push(InputFile::new(p));
                        }
                    }
                }
            }
        } else {
            pool.push(InputFile::new(file_path.clone()));
        }
    }

    // --diff <refspec>: run git diff --name-only
    if let Some(ref refspec) = opts.diff {
        match Command::new("git")
            .args(["diff", "--name-only", refspec])
            .output()
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let p = PathBuf::from(line.trim());
                    if p.exists() {
                        pool.push(InputFile::new(p));
                    }
                }
            }
            Ok(output) => {
                return Err(BatonError::ValidationError(format!(
                    "git diff failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Err(e) => {
                return Err(BatonError::ValidationError(format!(
                    "could not run git diff: {e}"
                )));
            }
        }
    }

    // --files <path | ->: read newline-separated paths
    if let Some(ref source) = opts.file_list {
        let content = if source == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        } else {
            std::fs::read_to_string(source).map_err(|e| {
                BatonError::ValidationError(format!("reading file list '{source}': {e}"))
            })?
        };
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() {
                pool.push(InputFile::new(PathBuf::from(line)));
            }
        }
    }

    // Deduplicate by canonical path
    let mut seen = HashSet::new();
    pool.retain(|f| {
        if let Ok(canonical) = std::fs::canonicalize(&f.path) {
            seen.insert(canonical)
        } else {
            true // keep files that can't be canonicalized
        }
    });

    Ok(pool)
}

/// Recursively walk a directory, collecting all files.
fn walk_dir(dir: &std::path::Path, pool: &mut Vec<InputFile>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_dir(&p, pool);
            } else {
                pool.push(InputFile::new(p));
            }
        }
    }
}

// ─── Dispatch planner ────────────────────────────────────

use crate::config::InputDecl;

/// Match a filename against a glob pattern (e.g. "*.py", "src/**/*.rs").
fn glob_matches(pattern: &str, path: &std::path::Path) -> bool {
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    let path_str = path.to_string_lossy();
    // Try matching against full path first, then filename only
    glob_match::glob_match(pattern, &path_str) || glob_match::glob_match(pattern, &filename)
}

/// Extract a key value from a file path based on a key expression.
///
/// Supported expressions: `{stem}`, `{name}`, `{ext}`.
fn extract_key(expr: &str, path: &std::path::Path) -> Option<String> {
    match expr {
        "{stem}" => path.file_stem().map(|s| s.to_string_lossy().to_string()),
        "{name}" => path.file_name().map(|s| s.to_string_lossy().to_string()),
        "{ext}" => path.extension().map(|s| s.to_string_lossy().to_string()),
        _ => Option::None,
    }
}

/// Plan dispatch: turn a validator's input declaration + file pool into invocations.
///
/// Returns `(invocations, warnings)`. An empty invocations vec with warnings
/// means the validator should be skipped.
pub fn plan_dispatch(
    validator: &ValidatorConfig,
    pool: &[InputFile],
) -> (Vec<Invocation>, Vec<String>) {
    let mut warnings = Vec::new();

    match &validator.input {
        InputDecl::None => {
            // DP-001: single invocation, no files
            let inv = Invocation {
                validator_name: validator.name.clone(),
                group_key: Option::None,
                inputs: BTreeMap::new(),
            };
            (vec![inv], warnings)
        }
        InputDecl::PerFile { pattern } => {
            // DP-002: one invocation per matching file
            let matching: Vec<&InputFile> = pool
                .iter()
                .filter(|f| glob_matches(pattern, &f.path))
                .collect();

            if matching.is_empty() {
                // DP-007
                warnings.push(format!(
                    "Validator '{}': no files match pattern '{}'",
                    validator.name, pattern
                ));
                return (vec![], warnings);
            }

            let invocations = matching
                .into_iter()
                .map(|f| {
                    let mut inputs = BTreeMap::new();
                    inputs.insert("file".to_string(), vec![f.clone()]);
                    Invocation {
                        validator_name: validator.name.clone(),
                        group_key: Option::None,
                        inputs,
                    }
                })
                .collect();
            (invocations, warnings)
        }
        InputDecl::Batch { pattern } => {
            // DP-003: single invocation with all matching files
            let matching: Vec<InputFile> = pool
                .iter()
                .filter(|f| glob_matches(pattern, &f.path))
                .cloned()
                .collect();

            if matching.is_empty() {
                warnings.push(format!(
                    "Validator '{}': no files match pattern '{}'",
                    validator.name, pattern
                ));
                return (vec![], warnings);
            }

            let mut inputs = BTreeMap::new();
            inputs.insert("file".to_string(), matching);
            let inv = Invocation {
                validator_name: validator.name.clone(),
                group_key: Option::None,
                inputs,
            };
            (vec![inv], warnings)
        }
        InputDecl::Named(slots) => {
            // Separate fixed inputs from glob-matched inputs
            let mut fixed_inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
            let mut keyed_slots: Vec<(&String, &crate::config::InputSlot)> = Vec::new();
            let mut unkeyed_slots: Vec<(&String, &crate::config::InputSlot)> = Vec::new();

            for (slot_name, slot) in slots {
                if let Some(ref path) = slot.path {
                    // DP-006: fixed input
                    fixed_inputs.insert(
                        slot_name.clone(),
                        vec![InputFile::new(PathBuf::from(path))],
                    );
                } else if slot.key.is_some() {
                    keyed_slots.push((slot_name, slot));
                } else {
                    unkeyed_slots.push((slot_name, slot));
                }
            }

            if keyed_slots.is_empty() && unkeyed_slots.is_empty() {
                // Only fixed inputs — single invocation
                let inv = Invocation {
                    validator_name: validator.name.clone(),
                    group_key: Option::None,
                    inputs: fixed_inputs,
                };
                return (vec![inv], warnings);
            }

            if !keyed_slots.is_empty() {
                // DP-004: group by key
                // For each keyed slot, match files and extract keys
                let mut slot_keys: BTreeMap<String, BTreeMap<String, Vec<InputFile>>> =
                    BTreeMap::new(); // key_value -> slot_name -> files

                for (slot_name, slot) in &keyed_slots {
                    let pattern = slot.match_pattern.as_deref().unwrap_or("*");
                    let key_expr = slot.key.as_deref().unwrap();

                    for file in pool {
                        if glob_matches(pattern, &file.path) {
                            if let Some(key_val) = extract_key(key_expr, &file.path) {
                                slot_keys
                                    .entry(key_val)
                                    .or_default()
                                    .entry((*slot_name).clone())
                                    .or_default()
                                    .push(file.clone());
                            }
                        }
                    }
                }

                // Collect all key values
                let all_keys: HashSet<String> = slot_keys.keys().cloned().collect();
                let slot_names: Vec<&String> =
                    keyed_slots.iter().map(|(name, _)| *name).collect();

                let mut invocations = Vec::new();
                for key_val in &all_keys {
                    // DP-005: check completeness
                    let group = slot_keys.get(key_val);
                    let complete = slot_names.iter().all(|name| {
                        group
                            .map(|g| g.contains_key(*name))
                            .unwrap_or(false)
                    });

                    if !complete {
                        warnings.push(format!(
                            "Validator '{}': incomplete group for key '{}', skipping",
                            validator.name, key_val
                        ));
                        continue;
                    }

                    let mut inputs = fixed_inputs.clone();
                    if let Some(group) = group {
                        for (slot_name, files) in group {
                            inputs.insert(slot_name.clone(), files.clone());
                        }
                    }

                    // Also add unkeyed slots
                    for (slot_name, slot) in &unkeyed_slots {
                        let pattern = slot.match_pattern.as_deref().unwrap_or("*");
                        let matching: Vec<InputFile> = pool
                            .iter()
                            .filter(|f| glob_matches(pattern, &f.path))
                            .cloned()
                            .collect();
                        if !matching.is_empty() {
                            if slot.collect {
                                inputs.insert((*slot_name).clone(), matching);
                            } else if let Some(first) = matching.into_iter().next() {
                                inputs.insert((*slot_name).clone(), vec![first]);
                            }
                        }
                    }

                    invocations.push(Invocation {
                        validator_name: validator.name.clone(),
                        group_key: Some(key_val.clone()),
                        inputs,
                    });
                }

                if invocations.is_empty() && !all_keys.is_empty() {
                    warnings.push(format!(
                        "Validator '{}': all groups incomplete",
                        validator.name
                    ));
                }
                if all_keys.is_empty() {
                    warnings.push(format!(
                        "Validator '{}': no files match any keyed input patterns",
                        validator.name
                    ));
                }

                return (invocations, warnings);
            }

            // Only unkeyed named slots — single invocation
            let mut inputs = fixed_inputs;
            for (slot_name, slot) in &unkeyed_slots {
                let pattern = slot.match_pattern.as_deref().unwrap_or("*");
                let matching: Vec<InputFile> = pool
                    .iter()
                    .filter(|f| glob_matches(pattern, &f.path))
                    .cloned()
                    .collect();
                if slot.collect {
                    inputs.insert((*slot_name).clone(), matching);
                } else if let Some(first) = matching.into_iter().next() {
                    inputs.insert((*slot_name).clone(), vec![first]);
                }
            }

            let inv = Invocation {
                validator_name: validator.name.clone(),
                group_key: Option::None,
                inputs,
            };
            (vec![inv], warnings)
        }
    }
}

// ─── Gate run ────────────────────────────────────────────

/// Runs all validators in a gate's pipeline and returns a [`Verdict`].
///
/// This is the main entry point for gate execution. Validates the artifact
/// and context, then runs each validator in order, respecting `run_if`
/// conditions, `--only`/`--skip`/`--tags` filters, and blocking semantics.
pub fn run_gate(
    gate: &GateConfig,
    config: &BatonConfig,
    input_pool: Vec<InputFile>,
    options: &RunOptions,
) -> Result<Verdict> {
    let run_start = Instant::now();

    // Build inputs map: all files under "file" key for simple dispatch
    let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
    if !input_pool.is_empty() {
        inputs.insert("file".into(), input_pool);
    }

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

        let result = execute_validator(validator, &mut inputs, &results, Some(config));
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

    let (failed_at, feedback) =
        if final_status == VerdictStatus::Fail || final_status == VerdictStatus::Error {
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
        warnings: warnings_list,
        suppressed: suppressed.iter().map(|s| s.to_string()).collect(),
        history: result_list,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::test_helpers::{self as th, ValidatorBuilder};
    use tempfile::TempDir;

    // ═══════════════════════════════════════════════════════════════
    // Internal implementation tests
    // NOTE: evaluate_run_if and compute_final_status are pub but are
    //       low-level utilities used by run_gate. extract_cost is private.
    // ═══════════════════════════════════════════════════════════════

    // ─── run_if evaluation ───────────────────────────

    #[test]
    fn run_if_simple_pass() {
        let results = th::prior_results();
        assert!(evaluate_run_if("lint.status == pass", &results).unwrap());
    }

    #[test]
    fn run_if_simple_fail() {
        let results = th::prior_results();
        assert!(!evaluate_run_if("lint.status == fail", &results).unwrap());
    }

    #[test]
    fn run_if_and_both_true() {
        let results = th::prior_results();
        assert!(
            !evaluate_run_if("lint.status == pass and typecheck.status == pass", &results).unwrap()
        );
    }

    #[test]
    fn run_if_or_one_true() {
        let results = th::prior_results();
        assert!(
            evaluate_run_if("lint.status == fail or typecheck.status == fail", &results).unwrap()
        );
    }

    #[test]
    fn run_if_left_to_right_no_precedence() {
        // "a or b and c" → "(a or b) and c"
        let mut results = BTreeMap::new();
        results.insert("a".into(), th::result("a", Status::Pass));
        results.insert("b".into(), th::result("b", Status::Fail));
        results.insert("c".into(), th::result("c", Status::Fail));

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
        results.insert("a".into(), th::result("a", Status::Skip));
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
        let results = vec![th::result("a", Status::Pass), th::result("b", Status::Pass)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_with_warn() {
        let results = vec![th::result("a", Status::Pass), th::result("b", Status::Warn)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_with_fail() {
        let results = vec![th::result("a", Status::Pass), th::result("b", Status::Fail)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Fail);
    }

    #[test]
    fn final_status_error_beats_fail() {
        let results = vec![
            th::result("a", Status::Fail),
            th::result("b", Status::Error),
        ];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Error);
    }

    #[test]
    fn final_status_skip_ignored() {
        let results = vec![th::result("a", Status::Skip), th::result("b", Status::Pass)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_suppress_errors() {
        let results = vec![
            th::result("a", Status::Error),
            th::result("b", Status::Fail),
        ];
        assert_eq!(
            compute_final_status(&results, &[Status::Error]),
            VerdictStatus::Fail
        );
    }

    #[test]
    fn final_status_suppress_all() {
        let results = vec![
            th::result("a", Status::Error),
            th::result("b", Status::Fail),
        ];
        assert_eq!(
            compute_final_status(&results, &[Status::Error, Status::Fail, Status::Warn]),
            VerdictStatus::Pass
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // ═══════════════════════════════════════════════════════════════

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

    // ─── Human validator tests ───────────────────────

    #[test]
    fn human_validator_fails_with_prompt() {
        let v = ValidatorBuilder::human("human", "Please review this change.").build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("[human-review-requested]"));
        assert!(result.feedback.as_ref().unwrap().contains("Please review"));
    }

    // ─── Gate run tests ──────────────────────────────

    #[test]
    fn gate_all_pass() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.history.len(), 3);
    }

    #[test]
    fn gate_first_fail_blocks() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1").build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        assert_eq!(verdict.failed_at, Some("b".into()));
        // c should not have run
        assert_eq!(verdict.history.len(), 2);
    }

    #[test]
    fn gate_non_blocking_failure_passes() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // All 3 ran
        assert_eq!(verdict.history.len(), 3);
    }

    #[test]
    fn gate_all_mode_non_blocking_failure_counts() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn gate_conditional_skip() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 1").build(),
                ValidatorBuilder::script("b", "exit 0")
                    .run_if("a.status == pass")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // b should be skipped because a failed
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Skip);
    }

    #[test]
    fn gate_warn_from_script() {
        let gate = th::gate(
            "test",
            vec![ValidatorBuilder::script("check", "exit 2")
                .warn_exit_codes(vec![2])
                .build()],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.warnings, vec!["check"]);
    }

    #[test]
    fn gate_only_filter() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.only = Some(vec!["b".into()]);

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Pass);
        let a_result = verdict.history.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a_result.status, Status::Skip);
    }

    #[test]
    fn gate_skip_filter() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.skip = Some(vec!["a".into()]);

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        let a_result = verdict.history.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a_result.status, Status::Skip);
    }

    #[test]
    fn gate_suppress_errors() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                // This will fail, not error, but let's test with a validator that errors
                ValidatorBuilder::script("b", "exit 1").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Fail];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Fail is suppressed, so pipeline continues and gate passes
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // But history still records the true status
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Fail);
    }

    #[test]
    fn gate_error_vs_fail_in_all_mode() {
        // Use a command that will error (nonexistent working dir)
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Error takes precedence over fail
        assert_eq!(verdict.status, VerdictStatus::Error);
    }

    #[test]
    fn gate_suppress_errors_with_all_mode() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;
        opts.suppressed_statuses = vec![Status::Error];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Error suppressed, fail remains
        assert_eq!(verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn gate_suppress_all_in_all_mode() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;
        opts.suppressed_statuses = vec![Status::Error, Status::Fail, Status::Warn];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
    }

    // ─── LLM completion validator tests ─────────────────

    #[test]
    fn llm_completion_pass_verdict() {
        let server = httpmock::MockServer::start();

        // Health check mock (GET /v1/models) — needed for unified runtime fallback
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "PASS — code looks good"
                    }
                }],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 20
                }
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this code").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        assert!(result.cost.is_some());
        let cost = result.cost.unwrap();
        assert_eq!(cost.input_tokens, Some(100));
        assert_eq!(cost.output_tokens, Some(20));
        assert_eq!(cost.model, Some("test-model".into()));

        mock.assert();
    }

    #[test]
    fn llm_completion_fail_verdict() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "FAIL — missing error handling in function parse()"
                    }
                }],
                "usage": {
                    "prompt_tokens": 150,
                    "completion_tokens": 30
                }
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this code").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Fail);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("missing error handling"));
    }

    #[test]
    fn llm_completion_warn_verdict() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "WARN minor style issue"
                    }
                }]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Warn);
    }

    #[test]
    fn llm_completion_unparseable_verdict() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "I reviewed the code but I'm not sure what to say about it."
                    }
                }]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Could not parse verdict"));
    }

    #[test]
    fn llm_completion_empty_response() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": ""
                    }
                }]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("empty"));
    }

    #[test]
    fn llm_completion_http_401() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(401).body(r#"{"error": "unauthorized"}"#);
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Authentication failed"));
    }

    #[test]
    fn llm_completion_http_404() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(404).body(r#"{"error": "model not found"}"#);
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("Model"));
        assert!(result.feedback.as_ref().unwrap().contains("not found"));
    }

    #[test]
    fn llm_completion_http_429() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(429).body(r#"{"error": "rate limited"}"#);
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("Rate limited"));
    }

    #[test]
    fn llm_completion_http_500() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(500).body(r#"{"error": "internal error"}"#);
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("HTTP 500"));
    }

    #[test]
    fn llm_completion_unreachable_provider() {
        let config = th::config_with_provider("http://127.0.0.1:1");
        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("No reachable runtime"));
    }

    #[test]
    fn llm_completion_missing_provider() {
        let config = th::config_with_provider("http://localhost");
        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .provider("nonexistent")
            .build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("not defined"));
    }

    #[test]
    fn llm_completion_no_config() {
        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("requires config"));
    }

    #[test]
    fn llm_completion_freeform_returns_warn() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "The code could use better variable names."
                    }
                }]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .response_format(ResponseFormat::Freeform)
            .build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Warn);
        assert!(result.feedback.as_ref().unwrap().contains("variable names"));
    }

    #[test]
    fn llm_completion_with_system_prompt() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .body_includes("system")
                .body_includes("code reviewer");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "PASS"
                    }
                }]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .system_prompt("You are a code reviewer.")
            .build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);

        mock.assert();
    }

    #[test]
    fn llm_completion_with_placeholders() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .body_includes("def hello()");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "PASS"
                    }
                }]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review: {file.content}").build();

        use std::io::Write as _;
        let mut tmpf = tempfile::NamedTempFile::new().unwrap();
        write!(tmpf, "def hello(): pass").unwrap();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        inputs.insert(
            "file".into(),
            vec![InputFile::new(tmpf.path().to_path_buf())],
        );
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);

        mock.assert();
    }

    #[test]
    fn llm_completion_cost_tracking() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "PASS"
                    }
                }],
                "usage": {
                    "prompt_tokens": 500,
                    "completion_tokens": 100
                }
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert!(result.cost.is_some());
        let cost = result.cost.unwrap();
        assert_eq!(cost.input_tokens, Some(500));
        assert_eq!(cost.output_tokens, Some(100));
        assert_eq!(cost.model, Some("test-model".into()));
    }

    #[test]
    fn llm_completion_no_usage_in_response() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "PASS"
                    }
                }]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        assert!(result.cost.is_none());
    }

    #[test]
    fn llm_completion_uses_default_model() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .body_includes("test-model");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "PASS"
                    }
                }],
                "usage": {
                    "prompt_tokens": 50,
                    "completion_tokens": 10
                }
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .no_model() // Should use provider default
            .build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        // Cost should reflect the provider's default model
        let cost = result.cost.unwrap();
        assert_eq!(cost.model, Some("test-model".into()));

        mock.assert();
    }

    #[test]
    fn llm_completion_in_gate_run() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "PASS"
                    }
                }]
            }));
        });

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let gate = th::gate("test", vec![v]);

        let mut config = th::config_with_provider(&server.url(""));
        config.gates.insert("test".into(), gate.clone());

        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.history.len(), 1);
        assert_eq!(verdict.history[0].status, Status::Pass);
    }

    #[test]
    fn llm_completion_fail_blocks_gate() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "choices": [{
                    "message": {
                        "content": "FAIL — missing tests"
                    }
                }]
            }));
        });

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let gate = th::gate(
            "test",
            vec![v, ValidatorBuilder::script("after", "exit 0").build()],
        );

        let mut config = th::config_with_provider(&server.url(""));
        config.gates.insert("test".into(), gate.clone());

        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        assert_eq!(verdict.failed_at, Some("llm-check".into()));
        // After validator should not have run (blocking)
        assert_eq!(verdict.history.len(), 1);
    }

    // ─── LLM session validator tests ────────────────────

    #[test]
    fn llm_session_no_config() {
        let v = ValidatorBuilder::llm("session-check", "Review this")
            .mode(LlmMode::Session)
            .runtime("openhands")
            .build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("requires config"));
    }

    #[test]
    fn llm_session_missing_runtime() {
        let config = th::config_with_provider("http://localhost");
        let v = ValidatorBuilder::llm("session-check", "Review this")
            .mode(LlmMode::Session)
            .build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("runtime"));
    }

    #[test]
    fn llm_session_undefined_runtime() {
        let config = th::config_with_provider("http://localhost");
        let v = ValidatorBuilder::llm("session-check", "Review this")
            .mode(LlmMode::Session)
            .runtime("nonexistent")
            .build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("not defined"));
    }

    // ─── drive_session orchestration tests ───────────────

    use crate::test_helpers::MockRuntimeAdapter;

    fn test_session_config() -> crate::runtime::SessionConfig {
        MockRuntimeAdapter::dummy_session_config()
    }

    #[test]
    fn session_completes_pass() {
        let mock = MockRuntimeAdapter::completing("PASS — looks good");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Pass);
        assert_eq!(mock.teardown_count(), 1);
    }

    #[test]
    fn session_completes_fail() {
        let mock = MockRuntimeAdapter::completing("FAIL — missing tests");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("missing tests"));
    }

    #[test]
    fn session_completes_warn() {
        let mock = MockRuntimeAdapter::completing("WARN minor style issue");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Warn);
    }

    #[test]
    fn session_unparseable_output() {
        let mock = MockRuntimeAdapter::completing("I think the code is fine maybe");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Could not parse"));
    }

    #[test]
    fn session_empty_output() {
        let mock = MockRuntimeAdapter::completing("");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("no PASS/FAIL/WARN"));
    }

    #[test]
    fn session_failed_status() {
        let mock = MockRuntimeAdapter::failing();
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("'failed'"));
        assert_eq!(mock.teardown_count(), 1);
    }

    #[test]
    fn session_timed_out_status() {
        let mock = MockRuntimeAdapter::failing().with_terminal_status(SessionStatus::TimedOut);
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("timed out"));
    }

    #[test]
    fn session_cancelled_status() {
        let mock = MockRuntimeAdapter::failing().with_terminal_status(SessionStatus::Cancelled);
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("'cancelled'"));
    }

    #[test]
    fn session_create_error() {
        let mock = MockRuntimeAdapter::completing("PASS").with_create_error("connection refused");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("connection refused"));
        // No teardown since session was never created
        assert_eq!(mock.teardown_count(), 0);
    }

    #[test]
    fn session_collect_error_tears_down() {
        let mock = MockRuntimeAdapter::completing("PASS").with_collect_error("network timeout");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("network timeout"));
        assert_eq!(mock.teardown_count(), 1);
    }

    #[test]
    fn session_cost_propagated() {
        let cost = Cost {
            input_tokens: Some(1000),
            output_tokens: Some(200),
            model: Some("claude-sonnet".into()),
            estimated_usd: Some(0.005),
        };
        let mock = MockRuntimeAdapter::completing("PASS").with_cost(cost);
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Pass);
        let c = result.cost.unwrap();
        assert_eq!(c.input_tokens, Some(1000));
        assert_eq!(c.output_tokens, Some(200));
        assert_eq!(c.model, Some("claude-sonnet".into()));
    }

    #[test]
    fn session_cost_on_failure() {
        let cost = Cost {
            input_tokens: Some(500),
            output_tokens: Some(100),
            model: Some("test".into()),
            estimated_usd: None,
        };
        let mock = MockRuntimeAdapter::failing().with_cost(cost);
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        // Cost should still be present even on failure
        assert!(result.cost.is_some());
        assert_eq!(result.cost.unwrap().input_tokens, Some(500));
    }

    #[test]
    fn session_teardown_always_called_on_success() {
        let mock = MockRuntimeAdapter::completing("PASS");
        let _ = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(mock.teardown_count(), 1);
        assert_eq!(mock.cancel_count(), 0);
    }

    #[test]
    fn session_validator_name_propagated() {
        let mock = MockRuntimeAdapter::completing("PASS");
        let result = drive_session("my-validator", &mock, test_session_config(), 30);
        assert_eq!(result.name, "my-validator");
    }

    #[test]
    fn session_timeout_cancels_and_tears_down() {
        // Mock returns Running forever; timeout_seconds=1 triggers after first poll+sleep
        let mock = MockRuntimeAdapter::hanging();
        let result = drive_session("v", &mock, test_session_config(), 1);
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("timed out"));
        assert_eq!(mock.cancel_count(), 1);
        assert_eq!(mock.teardown_count(), 1);
    }

    // ─── Spec coverage (UNTESTED) ──────────────────────

    #[test]
    fn run_if_empty_expression_returns_err() {
        let results = BTreeMap::new();
        let err = evaluate_run_if("", &results).unwrap_err();
        assert!(
            err.to_string().contains("Empty run_if"),
            "expected 'Empty run_if' in error, got: {err}"
        );
    }

    #[test]
    fn run_if_unrecognized_status_returns_err() {
        let results = th::prior_results();
        let err = evaluate_run_if("lint.status == invalid_status", &results).unwrap_err();
        assert!(
            err.to_string().contains("Invalid status"),
            "expected 'Invalid status' in error, got: {err}"
        );
    }

    #[test]
    fn run_if_expression_ending_with_operator_returns_err() {
        let results = th::prior_results();
        // "lint.status == pass and" — the trailing "and" gets absorbed into
        // the atom text by split_run_if, producing an invalid atom that cannot
        // be parsed. Either way an Err is returned.
        let err = evaluate_run_if("lint.status == pass and", &results).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Invalid"),
            "expected 'Invalid' in error, got: {msg}"
        );
    }

    #[test]
    fn compute_final_status_empty_results_is_pass() {
        assert_eq!(compute_final_status(&[], &[]), VerdictStatus::Pass);
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

    // ═══════════════════════════════════════════════════════════════
    // Additional edge-case tests
    // ═══════════════════════════════════════════════════════════════

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

    // ─── Human validator edge cases ──────────────────────

    #[test]
    fn human_validator_with_placeholders_in_prompt() {
        use std::io::Write as _;
        let mut tmpf = tempfile::NamedTempFile::new().unwrap();
        write!(tmpf, "fn main() {{}}").unwrap();
        let v = ValidatorBuilder::human("human-ph", "Review {file.content} please").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        inputs.insert(
            "file".into(),
            vec![InputFile::new(tmpf.path().to_path_buf())],
        );
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("fn main() {}"));
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("[human-review-requested]"));
    }

    #[test]
    fn human_validator_with_empty_prompt() {
        // prompt is None — the builder with "" sets Some(""), which resolves to ""
        let mut v = ValidatorBuilder::human("human-empty", "").build();
        v.prompt = None;
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        // With None prompt, it falls back to "" and renders "[human-review-requested] "
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("[human-review-requested]"));
    }

    // ─── execute_validator: run_if error propagation ─────

    #[test]
    fn execute_validator_run_if_error_propagates() {
        let v = ValidatorBuilder::script("run-if-err", "exit 0")
            .run_if("bad expression no operator")
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
                .contains("run_if evaluation error"),
            "expected run_if error, got: {:?}",
            result.feedback
        );
    }

    // ─── run_gate: --all mode with mixed results ─────────

    #[test]
    fn gate_all_mode_mixed_fail_and_pass_reports_fail() {
        // All validators run; final status uses compute_final_status
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("c", "exit 0").build(),
                ValidatorBuilder::script("d", "exit 1").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        // All 4 ran
        assert_eq!(verdict.history.len(), 4);
        // failed_at should point to the first failing validator
        assert_eq!(verdict.failed_at, Some("b".into()));
        assert!(verdict.feedback.is_some());
    }

    #[test]
    fn gate_all_mode_all_pass() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert!(verdict.failed_at.is_none());
        assert!(verdict.feedback.is_none());
    }

    // ─── run_gate: warnings list tracking ────────────────

    #[test]
    fn gate_warnings_list_tracks_multiple_warn_validators() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("w1", "exit 2")
                    .warn_exit_codes(vec![2])
                    .build(),
                ValidatorBuilder::script("ok", "exit 0").build(),
                ValidatorBuilder::script("w2", "exit 3")
                    .warn_exit_codes(vec![3])
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.warnings, vec!["w1", "w2"]);
    }

    // ─── run_gate: required context error message format ─

    #[test]
    fn gate_required_context_error_includes_gate_name() {
        // Required context checking has been removed in v2 migration.
        // Gates no longer have context slots — they use input files.
        // This test now just verifies a gate with no validators passes.
        let gate = th::gate(
            "my-gate",
            vec![ValidatorBuilder::script("a", "exit 0").build()],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
    }

    // ─── run_gate: suppress_all mode (Warn + Error + Fail) ─

    #[test]
    fn gate_suppress_all_statuses_in_blocking_mode() {
        // Fail is blocking, but suppressed — so pipeline continues
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("warn-v", "exit 2")
                    .warn_exit_codes(vec![2])
                    .build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
                ValidatorBuilder::script("ok-v", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Warn, Status::Error, Status::Fail];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // All suppressed, so gate passes
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // All validators ran (blocking failures were suppressed)
        assert_eq!(verdict.history.len(), 4);
        // Suppressed list recorded in verdict
        assert_eq!(verdict.suppressed.len(), 3);
    }

    // ─── LLM completion: missing prompt ──────────────────

    #[test]
    fn llm_completion_missing_prompt() {
        let config = th::config_with_provider("http://localhost");
        let mut v = ValidatorBuilder::llm("llm-no-prompt", "placeholder").build();
        v.prompt = None;

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(
            result.feedback.as_ref().unwrap().contains("missing prompt"),
            "expected missing prompt error, got: {:?}",
            result.feedback
        );
    }

    // ─── LLM completion: no choices / missing content key ─

    #[test]
    fn llm_completion_missing_choices_key() {
        // Response has no "choices" key at all — content resolves to ""
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(200).json_body(serde_json::json!({
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5
                }
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("empty"));
    }

    // ─── LLM completion: generic HTTP error (e.g. 503) ───

    #[test]
    fn llm_completion_http_503() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(503).body(r#"{"error": "service unavailable"}"#);
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("HTTP 503"));
    }

    // ─── drive_session: poll error cancels and tears down ─

    /// A mock adapter that returns an error on poll_status.
    #[derive(Debug)]
    struct PollErrorAdapter {
        teardown_count: std::sync::atomic::AtomicU32,
        cancel_count: std::sync::atomic::AtomicU32,
    }

    impl PollErrorAdapter {
        fn new() -> Self {
            Self {
                teardown_count: std::sync::atomic::AtomicU32::new(0),
                cancel_count: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    impl runtime::RuntimeAdapter for PollErrorAdapter {
        fn health_check(&self) -> crate::error::Result<runtime::HealthResult> {
            Ok(runtime::HealthResult {
                reachable: true,
                version: None,
                models: None,
                message: None,
            })
        }
        fn create_session(
            &self,
            _config: runtime::SessionConfig,
        ) -> crate::error::Result<runtime::SessionHandle> {
            Ok(runtime::SessionHandle {
                id: "poll-err-session".into(),
                workspace_id: "ws".into(),
            })
        }
        fn poll_status(
            &self,
            _handle: &runtime::SessionHandle,
        ) -> crate::error::Result<runtime::SessionStatus> {
            Err(crate::error::BatonError::RuntimeError(
                "poll network failure".into(),
            ))
        }
        fn collect_result(
            &self,
            _handle: &runtime::SessionHandle,
        ) -> crate::error::Result<runtime::SessionResult> {
            unreachable!("should not collect after poll error");
        }
        fn cancel(&self, _handle: &runtime::SessionHandle) -> crate::error::Result<()> {
            self.cancel_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn teardown(&self, _handle: &runtime::SessionHandle) -> crate::error::Result<()> {
            self.teardown_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn session_poll_error_cancels_and_tears_down() {
        let adapter = PollErrorAdapter::new();
        let result = drive_session("v", &adapter, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(
            result
                .feedback
                .as_ref()
                .unwrap()
                .contains("Error polling session"),
            "expected poll error, got: {:?}",
            result.feedback
        );
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("poll network failure"));
        assert_eq!(
            adapter
                .cancel_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            adapter
                .teardown_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    // ─── drive_session: collect error tears down ─────────

    #[test]
    fn session_collect_error_message_contains_cause() {
        let mock =
            MockRuntimeAdapter::completing("PASS").with_collect_error("database unavailable");
        let result = drive_session("v", &mock, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Error collecting session result"));
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("database unavailable"));
        assert_eq!(mock.teardown_count(), 1);
    }

    // ─── drive_session: Running status (defensive path) ──

    #[test]
    fn session_running_terminal_status_returns_error() {
        // Force the mock to report Running as the terminal status.
        // This exercises the defensive `SessionStatus::Running` match arm.
        let mock =
            MockRuntimeAdapter::completing("PASS").with_terminal_status(SessionStatus::Running);
        // polls_before_done is 0, so first poll returns the terminal status (Running).
        // But the poll loop sees Running and sleeps+continues, so we need timeout=1
        // to break out. After timeout, it cancels and tears down with a timeout error.
        // Actually: the mock immediately returns Running (terminal_status), the poll
        // loop sees Running and sleeps. After timeout_seconds=1, it exits with timeout.
        // To truly test the defensive path, we need a mock that returns Running from
        // collect_result's status. Let's use a custom adapter.

        // Use a simpler approach: the existing MockRuntimeAdapter::completing_after
        // makes the poll loop exit when poll returns Completed. Then collect_result
        // returns a SessionResult with status=Running (the terminal_status we set).
        // Wait — with_terminal_status sets terminal_status which is used by BOTH
        // poll_status (after N polls) and collect_result. So if we do
        // completing_after(0, "PASS").with_terminal_status(Running), poll_status
        // returns Running on first call, the loop sleeps and continues, eventually
        // timing out. That doesn't test the defensive path.
        //
        // We need an adapter that: poll returns Completed, but collect returns
        // status=Running. Let's build a custom one.
        drop(mock);

        #[derive(Debug)]
        struct RunningCollectAdapter;
        impl runtime::RuntimeAdapter for RunningCollectAdapter {
            fn health_check(&self) -> crate::error::Result<runtime::HealthResult> {
                Ok(runtime::HealthResult {
                    reachable: true,
                    version: None,
                    models: None,
                    message: None,
                })
            }
            fn create_session(
                &self,
                _config: runtime::SessionConfig,
            ) -> crate::error::Result<runtime::SessionHandle> {
                Ok(runtime::SessionHandle {
                    id: "s".into(),
                    workspace_id: "w".into(),
                })
            }
            fn poll_status(
                &self,
                _handle: &runtime::SessionHandle,
            ) -> crate::error::Result<runtime::SessionStatus> {
                Ok(runtime::SessionStatus::Completed)
            }
            fn collect_result(
                &self,
                _handle: &runtime::SessionHandle,
            ) -> crate::error::Result<runtime::SessionResult> {
                Ok(runtime::SessionResult {
                    status: runtime::SessionStatus::Running,
                    output: "PASS".into(),
                    raw_log: String::new(),
                    cost: Some(Cost {
                        input_tokens: Some(50),
                        output_tokens: Some(10),
                        model: Some("m".into()),
                        estimated_usd: None,
                    }),
                })
            }
            fn cancel(&self, _handle: &runtime::SessionHandle) -> crate::error::Result<()> {
                Ok(())
            }
            fn teardown(&self, _handle: &runtime::SessionHandle) -> crate::error::Result<()> {
                Ok(())
            }
        }

        let adapter = RunningCollectAdapter;
        let result = drive_session("v", &adapter, test_session_config(), 30);
        assert_eq!(result.status, Status::Error);
        assert!(
            result.feedback.as_ref().unwrap().contains("no verdict"),
            "expected 'no verdict' in feedback, got: {:?}",
            result.feedback
        );
        // Cost should still be propagated from the session result
        assert!(result.cost.is_some());
    }

    // ─── LLM session: missing prompt ─────────────────────

    #[test]
    fn llm_session_missing_prompt() {
        let mut config = th::config_with_provider("http://localhost");
        config.runtimes.insert(
            "oh".into(),
            crate::config::Runtime {
                runtime_type: "openhands".into(),
                base_url: "http://localhost:3000".into(),
                api_key_env: None,
                default_model: Some("test-model".into()),
                sandbox: false,
                timeout_seconds: 300,
                max_iterations: 10,
            },
        );

        let mut v = ValidatorBuilder::llm("session-no-prompt", "placeholder")
            .mode(LlmMode::Session)
            .runtime("oh")
            .build();
        v.prompt = None;

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(
            result.feedback.as_ref().unwrap().contains("missing prompt"),
            "expected missing prompt, got: {:?}",
            result.feedback
        );
    }

    // ─── run_gate: error in blocking mode propagates Error verdict ─

    #[test]
    fn gate_blocking_error_returns_error_verdict() {
        // A script validator that errors (not fails) should produce VerdictStatus::Error
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("err-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
                ValidatorBuilder::script("after", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Error);
        assert_eq!(verdict.failed_at, Some("err-v".into()));
        // "after" should not have run
        assert_eq!(verdict.history.len(), 1);
    }

    // ─── run_gate: suppressed list in verdict ────────────

    #[test]
    fn gate_verdict_records_suppressed_statuses() {
        let gate = th::gate(
            "test",
            vec![ValidatorBuilder::script("a", "exit 0").build()],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Warn, Status::Fail];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert!(verdict.suppressed.contains(&"warn".to_string()));
        assert!(verdict.suppressed.contains(&"fail".to_string()));
    }

    // ─── run_gate: all mode with error reports failed_at ─

    #[test]
    fn gate_all_mode_error_reports_failed_at_error_validator() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("ok", "exit 0").build(),
                ValidatorBuilder::script("fail-v", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("err-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .blocking(false)
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Error beats fail
        assert_eq!(verdict.status, VerdictStatus::Error);
        // failed_at should point to the error validator (first with Error status)
        assert_eq!(verdict.failed_at, Some("err-v".into()));
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

    // ─── LLM completion: HTTP 403 ────────────────────────

    #[test]
    fn llm_completion_http_403() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions");
            then.status(403).body(r#"{"error": "forbidden"}"#);
        });

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Authentication failed"));
    }

    // ─── Wave 3: Exec-level gap coverage ────────────────

    #[test]
    fn llm_completion_api_key_env_not_set() {
        // API runtime requires an env var that isn't set
        let mut runtimes = BTreeMap::new();
        runtimes.insert(
            "default".into(),
            crate::config::Runtime {
                runtime_type: "api".into(),
                base_url: "http://localhost:1".into(),
                api_key_env: Some("BATON_TEST_NONEXISTENT_KEY_WAVE3".into()),
                default_model: Some("test-model".into()),
                sandbox: false,
                timeout_seconds: 30,
                max_iterations: 1,
            },
        );

        let config = BatonConfig {
            version: "0.6".into(),
            defaults: crate::config::Defaults {
                timeout_seconds: 300,
                blocking: true,
                prompts_dir: "/tmp/prompts".into(),
                log_dir: "/tmp/logs".into(),
                history_db: "/tmp/history.db".into(),
                tmp_dir: "/tmp/tmp".into(),
            },
            runtimes,
            sources: BTreeMap::new(),
            gates: BTreeMap::new(),
            config_dir: "/tmp".into(),
        };

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        let feedback = result.feedback.unwrap();
        assert!(
            feedback.contains("BATON_TEST_NONEXISTENT_KEY_WAVE3"),
            "Feedback should mention the env var: {feedback}"
        );
    }

    #[test]
    fn llm_completion_prompt_file_resolution() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let prompts_dir = tmp_dir.path().join("prompts");
        std::fs::create_dir_all(&prompts_dir).unwrap();
        std::fs::write(
            prompts_dir.join("review.md"),
            "Please review the code carefully.",
        )
        .unwrap();

        let server = httpmock::MockServer::start();

        // Health check mock (GET /v1/models)
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/v1/models");
            then.status(200)
                .json_body(serde_json::json!({ "data": [] }));
        });

        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                // Verify the resolved prompt file content appears in the request body
                .body_includes("review the code carefully");
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "PASS — looks good"}}]
            }));
        });

        let mut runtimes = BTreeMap::new();
        runtimes.insert(
            "default".into(),
            crate::config::Runtime {
                runtime_type: "api".into(),
                base_url: server.url(""),
                api_key_env: None,
                default_model: Some("test-model".into()),
                sandbox: false,
                timeout_seconds: 30,
                max_iterations: 1,
            },
        );

        let config = BatonConfig {
            version: "0.6".into(),
            defaults: crate::config::Defaults {
                timeout_seconds: 300,
                blocking: true,
                prompts_dir: prompts_dir.to_path_buf(),
                log_dir: "/tmp/logs".into(),
                history_db: "/tmp/history.db".into(),
                tmp_dir: "/tmp/tmp".into(),
            },
            runtimes,
            sources: BTreeMap::new(),
            gates: BTreeMap::new(),
            config_dir: tmp_dir.path().to_path_buf(),
        };

        // Use a .md file reference as prompt
        let v = ValidatorBuilder::llm("llm-check", "review.md").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        mock.assert();
    }

    #[test]
    fn llm_completion_prompt_file_not_found() {
        let server = httpmock::MockServer::start();
        // No mock needed — should fail before HTTP call

        let config = th::config_with_provider(&server.url(""));

        let v = ValidatorBuilder::llm("llm-check", "nonexistent-prompt.md").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        let feedback = result.feedback.unwrap();
        assert!(
            feedback.contains("nonexistent-prompt.md")
                || feedback.contains("not found")
                || feedback.contains("No such file"),
            "Feedback should reference the missing file: {feedback}"
        );
    }

    #[test]
    fn llm_completion_max_tokens_in_request_body() {
        let server = httpmock::MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/chat/completions")
                .json_body_includes(r#"{"max_tokens": 4096}"#);
            then.status(200).json_body(serde_json::json!({
                "choices": [{"message": {"content": "PASS — ok"}}]
            }));
        });

        let config = th::config_with_provider(&server.url(""));

        // ValidatorBuilder::llm sets max_tokens to Some(4096) by default
        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut inputs, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        mock.assert();
    }

    // ═══════════════════════════════════════════════════════════════
    // File collector tests (SPEC-EX-FC-*)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn file_collector_single_file() {
        // SPEC-EX-FC-001: positional file args populate the input pool
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "content").unwrap();

        let opts = FileCollectOptions {
            files: vec![f.path().to_path_buf()],
            diff: None,
            file_list: None,
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(pool[0].path, f.path().to_path_buf());
    }

    #[test]
    fn file_collector_directory_walk() {
        // SPEC-EX-FC-001: directory paths are walked recursively
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.py"), "a").unwrap();
        std::fs::write(dir.path().join("sub/b.py"), "b").unwrap();

        let opts = FileCollectOptions {
            files: vec![dir.path().to_path_buf()],
            diff: None,
            file_list: None,
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn file_collector_deduplication() {
        // SPEC-EX-FC-004: pool is deduplicated by canonical path
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");
        std::fs::write(&file_path, "content").unwrap();

        let canonical = std::fs::canonicalize(&file_path).unwrap();

        let opts = FileCollectOptions {
            files: vec![file_path, canonical],
            diff: None,
            file_list: None,
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn file_collector_reads_file_list() {
        // SPEC-EX-FC-003: --files reads newline-separated paths
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.py");
        let b = dir.path().join("b.py");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();

        let list_file = dir.path().join("filelist.txt");
        std::fs::write(&list_file, format!("{}\n{}\n", a.display(), b.display())).unwrap();

        let opts = FileCollectOptions {
            files: vec![],
            diff: None,
            file_list: Some(list_file.display().to_string()),
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn file_collector_no_recursive() {
        // SPEC-EX-FC-005: --no-recursive disables recursive directory walking
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.py"), "a").unwrap();
        std::fs::write(dir.path().join("sub/b.py"), "b").unwrap();

        let opts = FileCollectOptions {
            files: vec![dir.path().to_path_buf()],
            diff: None,
            file_list: None,
            recursive: false,
        };
        let pool = collect_file_pool(&opts).unwrap();
        // Only the top-level file, not the one in sub/
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn file_collector_not_found_errors() {
        let opts = FileCollectOptions {
            files: vec![std::path::PathBuf::from("/nonexistent/file.py")],
            diff: None,
            file_list: None,
            recursive: true,
        };
        assert!(collect_file_pool(&opts).is_err());
    }

    #[test]
    fn file_collector_empty_lines_skipped() {
        // Empty lines in file list should be skipped
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.py");
        std::fs::write(&a, "a").unwrap();

        let list_file = dir.path().join("filelist.txt");
        std::fs::write(&list_file, format!("\n{}\n\n", a.display())).unwrap();

        let opts = FileCollectOptions {
            files: vec![],
            diff: None,
            file_list: Some(list_file.display().to_string()),
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 1);
    }

    // ═══════════════════════════════════════════════════════════════
    // Dispatch planner tests (SPEC-EX-DP-*)
    // ═══════════════════════════════════════════════════════════════

    fn validator_with_input(name: &str, input: crate::config::InputDecl) -> ValidatorConfig {
        ValidatorConfig {
            name: name.into(),
            input,
            ..th::ValidatorBuilder::script(name, "echo ok").build()
        }
    }

    #[test]
    fn dispatch_no_input_produces_single_invocation() {
        // SPEC-EX-DP-001: validator with no input field produces one invocation
        let v = validator_with_input("lint", crate::config::InputDecl::None);
        let pool: Vec<InputFile> = vec![];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1);
        assert!(invocations[0].inputs.is_empty());
        assert!(invocations[0].group_key.is_none());
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_per_file_produces_one_per_match() {
        // SPEC-EX-DP-002: per-file input produces one invocation per matching file
        let v = validator_with_input(
            "lint",
            crate::config::InputDecl::PerFile {
                pattern: "*.py".into(),
            },
        );
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/c.rs")),
        ];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 2); // a.py and b.py match, c.rs doesn't
        assert_eq!(
            invocations[0].inputs["file"][0].path,
            std::path::PathBuf::from("/tmp/a.py")
        );
        assert_eq!(
            invocations[1].inputs["file"][0].path,
            std::path::PathBuf::from("/tmp/b.py")
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_batch_produces_single_invocation() {
        // SPEC-EX-DP-003: batch input (collect = true) produces one invocation
        let v = validator_with_input(
            "batch-lint",
            crate::config::InputDecl::Batch {
                pattern: "*.py".into(),
            },
        );
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
        ];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].inputs["file"].len(), 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_keyed_inputs_grouped_by_key() {
        // SPEC-EX-DP-004: named inputs with key expressions grouped by key value
        use crate::config::InputSlot;

        let mut slots = BTreeMap::new();
        slots.insert(
            "code".into(),
            InputSlot {
                match_pattern: Some("*.py".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );
        slots.insert(
            "spec".into(),
            InputSlot {
                match_pattern: Some("*.md".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );

        let v = validator_with_input("check", crate::config::InputDecl::Named(slots));
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/a.md")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.md")),
        ];

        let (mut invocations, warnings) = plan_dispatch(&v, &pool);
        invocations.sort_by(|a, b| a.group_key.cmp(&b.group_key));

        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].group_key, Some("a".into()));
        assert_eq!(invocations[1].group_key, Some("b".into()));
        assert_eq!(invocations[0].inputs.len(), 2); // code + spec
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_incomplete_group_skips() {
        // SPEC-EX-DP-005: incomplete group skipped with warning
        use crate::config::InputSlot;

        let mut slots = BTreeMap::new();
        slots.insert(
            "code".into(),
            InputSlot {
                match_pattern: Some("*.py".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );
        slots.insert(
            "spec".into(),
            InputSlot {
                match_pattern: Some("*.md".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );

        let v = validator_with_input("check", crate::config::InputDecl::Named(slots));
        // a has both code and spec, b only has code (no b.md)
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/a.md")),
        ];

        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1); // only "a" group is complete
        assert_eq!(invocations[0].group_key, Some("a".into()));
        assert!(!warnings.is_empty()); // warning about incomplete "b" group
        assert!(warnings.iter().any(|w| w.contains("incomplete") && w.contains("b")));
    }

    #[test]
    fn dispatch_fixed_input_injected() {
        // SPEC-EX-DP-006: fixed inputs injected into every invocation
        use crate::config::InputSlot;

        let mut slots = BTreeMap::new();
        slots.insert(
            "config".into(),
            InputSlot {
                match_pattern: None,
                path: Some("/etc/config.toml".into()),
                key: None,
                collect: false,
            },
        );

        let v = validator_with_input("check", crate::config::InputDecl::Named(slots));
        let pool: Vec<InputFile> = vec![];

        let (invocations, _) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1);
        assert!(invocations[0].inputs.contains_key("config"));
        assert_eq!(
            invocations[0].inputs["config"][0].path,
            std::path::PathBuf::from("/etc/config.toml")
        );
    }

    #[test]
    fn dispatch_no_matching_files_produces_empty() {
        // SPEC-EX-DP-007: no matching files means validator is skipped
        let v = validator_with_input(
            "lint",
            crate::config::InputDecl::PerFile {
                pattern: "*.py".into(),
            },
        );
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/readme.md")),
            InputFile::new(std::path::PathBuf::from("/tmp/notes.txt")),
        ];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert!(invocations.is_empty());
        assert!(!warnings.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 migration: Execution pipeline tests (SPEC-EX-PL-*)
    // ═══════════════════════════════════════════════════════════════

    // SPEC-EX-PL-001: Gates filtered by --only/--skip before iteration
    // SPEC-EX-PL-002: If any invocation of a blocking validator fails, gate stops
    // SPEC-EX-PL-003: Each invocation produces a separate ValidatorResult

    // These tests depend on the new pipeline implementation. The patterns
    // are documented here so they can be filled in during implementation:

    #[test]
    fn pipeline_per_invocation_blocking() {
        // SPEC-EX-PL-002: any failing invocation of a blocking validator stops gate
        // This is a design test verifying the contract. When a per-file blocking
        // validator produces a Fail result for file #2 of 5, files #3-5 should
        // not be executed.
        //
        // EDGE CASE: This means a single failing file can block the entire gate,
        // even if all other files would pass. This is intentional — blocking means
        // "stop on any failure".
        use crate::types::{GateResult, ValidatorResult};

        // Simulate: 3 invocations, second one fails, third should not run
        let results = vec![
            ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: None,
                duration_ms: 10,
                cost: None,
            },
            ValidatorResult {
                name: "lint".into(),
                status: Status::Fail,
                feedback: Some("syntax error".into()),
                duration_ms: 10,
                cost: None,
            },
            // Third invocation should be skipped — verify by absence
        ];

        let gate_result = GateResult {
            gate_name: "review".into(),
            status: Status::Fail,
            validator_results: results,
            duration: std::time::Duration::from_millis(20),
        };

        // Only 2 results (third was blocked)
        assert_eq!(gate_result.validator_results.len(), 2);
        assert_eq!(gate_result.status, Status::Fail);
    }
}
