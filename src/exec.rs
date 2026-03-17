//! Gate execution engine.
//!
//! Runs validators in pipeline order, evaluates `run_if` conditions,
//! dispatches to script/LLM/human executors, and computes the final verdict.

use chrono::Utc;
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;

use crate::config::{
    split_run_if, BatonConfig, GateConfig, LlmMode, ResponseFormat, ValidatorConfig, ValidatorType,
};
use crate::error::{BatonError, Result};
use crate::placeholder::{resolve_placeholders, ResolutionWarnings};
use crate::prompt::{is_file_reference, resolve_prompt_value};
use crate::provider::{ProviderClient, ProviderError};
use crate::runtime::{self, SessionConfig, SessionStatus};
use crate::types::*;
use crate::verdict_parser::parse_verdict;

// ─── run_if evaluation ──────────────────────────────────

/// Evaluate a run_if expression against prior validator results.
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
    artifact: &mut Artifact,
    context: &mut Context,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> ValidatorResult {
    let prompt = validator.prompt.as_deref().unwrap_or("");
    let mut warnings = ResolutionWarnings::new();
    let rendered = resolve_placeholders(prompt, artifact, context, prior_results, &mut warnings);

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
    artifact: &mut Artifact,
    context: &mut Context,
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
        ValidatorType::Script => {
            execute_script_validator(validator, artifact, context, prior_results)
        }
        ValidatorType::Human => {
            execute_human_validator(validator, artifact, context, prior_results)
        }
        ValidatorType::Llm => match validator.mode {
            LlmMode::Completion => {
                execute_llm_completion(validator, artifact, context, prior_results, config)
            }
            LlmMode::Session => {
                execute_llm_session(validator, artifact, context, prior_results, config)
            }
        },
    };

    result.duration_ms = start.elapsed().as_millis() as i64;
    result
}

// ─── LLM completion validator ────────────────────────────

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

// ─── LLM session validator ──────────────────────────────

fn execute_llm_session(
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
                    "[baton] LLM session validator requires config with runtime settings".into(),
                ),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    // Resolve runtime
    let runtime_name = match &validator.runtime {
        Some(name) => name.clone(),
        None => {
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some(format!(
                    "[baton] Validator '{}': mode 'session' requires a 'runtime' field.",
                    validator.name
                )),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    let runtime_config = match config.runtimes.get(&runtime_name) {
        Some(r) => r,
        None => {
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some(format!(
                    "[baton] Runtime '{runtime_name}' is not defined in [runtimes]."
                )),
                duration_ms: 0,
                cost: None,
            };
        }
    };

    // Create adapter
    let adapter = match runtime::create_adapter(&runtime_name, runtime_config) {
        Ok(a) => a,
        Err(e) => {
            return ValidatorResult {
                name: validator.name.clone(),
                status: Status::Error,
                feedback: Some(format!(
                    "[baton] Failed to create session on runtime '{runtime_name}': {e}"
                )),
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
                feedback: Some("[baton] LLM session validator missing prompt".into()),
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

    // Prepare file set for isolation
    let mut files = BTreeMap::new();
    if let Some(ref path) = artifact.path {
        files.insert("artifact".into(), path.display().to_string());
    }
    for ref_name in &validator.context_refs {
        if let Some(item) = context.items.get(ref_name) {
            if let Some(ref path) = item.path {
                files.insert(ref_name.clone(), path.display().to_string());
            }
        }
    }

    // Determine model
    let model = validator
        .model
        .clone()
        .or_else(|| runtime_config.default_model.clone())
        .unwrap_or_else(|| "default".to_string());

    // Determine session parameters
    let sandbox = validator.sandbox.unwrap_or(runtime_config.sandbox);
    let max_iterations = validator
        .max_iterations
        .unwrap_or(runtime_config.max_iterations);
    let timeout_seconds = validator.timeout_seconds;

    // Create session
    let session_config = SessionConfig {
        task: rendered_prompt,
        files,
        model,
        sandbox,
        max_iterations,
        timeout_seconds,
        env: BTreeMap::new(),
    };

    drive_session(
        &validator.name,
        adapter.as_ref(),
        session_config,
        timeout_seconds,
    )
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

// ─── Gate run ────────────────────────────────────────────

/// Runs all validators in a gate's pipeline and returns a [`Verdict`].
///
/// This is the main entry point for gate execution. Validates the artifact
/// and context, then runs each validator in order, respecting `run_if`
/// conditions, `--only`/`--skip`/`--tags` filters, and blocking semantics.
pub fn run_gate(
    gate: &GateConfig,
    config: &BatonConfig,
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

        let result = execute_validator(validator, artifact, context, &results, Some(config));
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Pass);
    }

    #[test]
    fn script_exit_1_fail() {
        let v = ValidatorBuilder::script("test", "exit 1").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn script_exit_with_warn_code() {
        let v = ValidatorBuilder::script("test", "echo 'warning message' && exit 2")
            .warn_exit_codes(vec![2])
            .build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Fail);
    }

    #[test]
    fn script_no_output_fail_feedback() {
        let v = ValidatorBuilder::script("test", "exit 1").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("no output"));
    }

    #[test]
    fn script_with_stderr_feedback() {
        let v = ValidatorBuilder::script("test", "echo 'error detail' >&2 && exit 1").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("error detail"));
    }

    #[test]
    fn script_placeholder_resolution() {
        let dir = TempDir::new().unwrap();
        let art_path = dir.path().join("test.txt");
        std::fs::write(&art_path, "hello").unwrap();

        let cmd = if cfg!(windows) {
            "type {artifact}"
        } else {
            "cat {artifact}"
        };
        let v = ValidatorBuilder::script("test", cmd).build();
        let mut art = Artifact::from_file(&art_path).unwrap();
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Pass);
    }

    // ─── Human validator tests ───────────────────────

    #[test]
    fn human_validator_fails_with_prompt() {
        let v = ValidatorBuilder::human("human", "Please review this change.").build();

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1").build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let gate = th::gate(
            "test",
            vec![ValidatorBuilder::script("check", "exit 2")
                .warn_exit_codes(vec![2])
                .build()],
        );
        let config = th::config_for_gate(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
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
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fast", "exit 0")
                    .tags(vec!["quick"])
                    .build(),
                ValidatorBuilder::script("slow", "exit 0")
                    .tags(vec!["deep"])
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
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
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                // This will fail, not error, but let's test with a validator that errors
                ValidatorBuilder::script("b", "exit 1").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
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
        let mut gate = th::gate(
            "test",
            vec![ValidatorBuilder::script("a", "exit 0").build()],
        );
        gate.context.insert(
            "spec".into(),
            ContextSlot {
                description: None,
                required: true,
            },
        );
        let config = th::config_for_gate(gate.clone());
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;
        opts.suppressed_statuses = vec![Status::Error, Status::Fail, Status::Warn];

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
    }

    // ─── LLM completion validator tests ─────────────────

    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Start a mock HTTP server that returns a fixed response body.
    /// Returns (port, join_handle). The server handles exactly one request.
    fn start_mock_server(
        status_code: u16,
        response_body: &str,
    ) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = response_body.to_string();

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();

            let response = format!(
                "HTTP/1.1 {status_code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
            request
        });

        (port, handle)
    }

    #[test]
    fn llm_completion_pass_verdict() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "PASS — code looks good"
                }
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 20
            }
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this code").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        assert!(result.cost.is_some());
        let cost = result.cost.unwrap();
        assert_eq!(cost.input_tokens, Some(100));
        assert_eq!(cost.output_tokens, Some(20));
        assert_eq!(cost.model, Some("test-model".into()));

        // Verify the request was sent
        let request = handle.join().unwrap();
        assert!(request.contains("POST"));
        assert!(request.contains("/v1/chat/completions"));
    }

    #[test]
    fn llm_completion_fail_verdict() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "FAIL — missing error handling in function parse()"
                }
            }],
            "usage": {
                "prompt_tokens": 150,
                "completion_tokens": 30
            }
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this code").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Fail);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("missing error handling"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_warn_verdict() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "WARN minor style issue"
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Warn);

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_unparseable_verdict() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "I reviewed the code but I'm not sure what to say about it."
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Could not parse verdict"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_empty_response() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": ""
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("empty or malformed"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_http_401() {
        let (port, handle) = start_mock_server(401, r#"{"error": "unauthorized"}"#);
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Authentication failed"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_http_404() {
        let (port, handle) = start_mock_server(404, r#"{"error": "model not found"}"#);
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("Model"));
        assert!(result.feedback.as_ref().unwrap().contains("not found"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_http_429() {
        let (port, handle) = start_mock_server(429, r#"{"error": "rate limited"}"#);
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("Rate limited"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_http_500() {
        let (port, handle) = start_mock_server(500, r#"{"error": "internal error"}"#);
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("HTTP 500"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_unreachable_provider() {
        let config = th::config_with_provider("http://127.0.0.1:1");
        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Cannot reach provider"));
    }

    #[test]
    fn llm_completion_missing_provider() {
        let config = th::config_with_provider("http://localhost");
        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .provider("nonexistent")
            .build();

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("not defined"));
    }

    #[test]
    fn llm_completion_no_config() {
        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("requires config"));
    }

    #[test]
    fn llm_completion_freeform_returns_warn() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "The code could use better variable names."
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .response_format(ResponseFormat::Freeform)
            .build();

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Warn);
        assert!(result.feedback.as_ref().unwrap().contains("variable names"));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_with_system_prompt() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "PASS"
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .system_prompt("You are a code reviewer.")
            .build();

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);

        // Verify system message was sent
        let request = handle.join().unwrap();
        assert!(request.contains("system"));
        assert!(request.contains("code reviewer"));
    }

    #[test]
    fn llm_completion_with_placeholders() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "PASS"
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review: {artifact_content}").build();

        let mut art = Artifact::from_string("def hello(): pass");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);

        // Verify placeholder was resolved in the request
        let request = handle.join().unwrap();
        assert!(request.contains("def hello()"));
    }

    #[test]
    fn llm_completion_cost_tracking() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "PASS"
                }
            }],
            "usage": {
                "prompt_tokens": 500,
                "completion_tokens": 100
            }
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert!(result.cost.is_some());
        let cost = result.cost.unwrap();
        assert_eq!(cost.input_tokens, Some(500));
        assert_eq!(cost.output_tokens, Some(100));
        assert_eq!(cost.model, Some("test-model".into()));

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_no_usage_in_response() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "PASS"
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        assert!(result.cost.is_none());

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_uses_default_model() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "PASS"
                }
            }],
            "usage": {
                "prompt_tokens": 50,
                "completion_tokens": 10
            }
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this")
            .no_model() // Should use provider default
            .build();

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Pass);
        // Cost should reflect the provider's default model
        let cost = result.cost.unwrap();
        assert_eq!(cost.model, Some("test-model".into()));

        let request = handle.join().unwrap();
        assert!(request.contains("test-model"));
    }

    #[test]
    fn llm_completion_in_gate_run() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "PASS"
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let api_base = format!("http://127.0.0.1:{port}");

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let gate = th::gate("test", vec![v]);

        let mut config = th::config_with_provider(&api_base);
        config.gates.insert("test".into(), gate.clone());

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.history.len(), 1);
        assert_eq!(verdict.history[0].status, Status::Pass);

        handle.join().unwrap();
    }

    #[test]
    fn llm_completion_fail_blocks_gate() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "content": "FAIL — missing tests"
                }
            }]
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let api_base = format!("http://127.0.0.1:{port}");

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let gate = th::gate(
            "test",
            vec![v, ValidatorBuilder::script("after", "exit 0").build()],
        );

        let mut config = th::config_with_provider(&api_base);
        config.gates.insert("test".into(), gate.clone());

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        assert_eq!(verdict.failed_at, Some("llm-check".into()));
        // After validator should not have run (blocking)
        assert_eq!(verdict.history.len(), 1);

        handle.join().unwrap();
    }

    // ─── LLM session validator tests ────────────────────

    #[test]
    fn llm_session_no_config() {
        let v = ValidatorBuilder::llm("session-check", "Review this")
            .mode(LlmMode::Session)
            .runtime("openhands")
            .build();

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
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

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
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
        let mut artifact = Artifact::from_string("hello");
        let mut context = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut artifact, &mut context, &prior, None);
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
        let mut artifact = Artifact::from_string("hello");
        let mut context = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut artifact, &mut context, &prior, None);
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
        let mut artifact = Artifact::from_string("hello");
        let mut context = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut artifact, &mut context, &prior, None);
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
        let result2 = execute_validator(&v2, &mut artifact, &mut context, &prior, None);
        assert_eq!(
            result2.status,
            Status::Pass,
            "env var should be set correctly; feedback: {:?}",
            result2.feedback
        );
    }

    #[test]
    fn run_gate_artifact_not_found() {
        let v = ValidatorBuilder::script("dummy", "true").build();
        let gate = th::gate("test-gate", vec![v]);
        let config = th::config_for_gate(gate.clone());
        let mut artifact = Artifact::from_string("placeholder");
        artifact.path = Some(std::path::PathBuf::from("/nonexistent/file.txt"));
        let mut context = Context::new();
        let options = RunOptions::new();

        let err = run_gate(&gate, &config, &mut artifact, &mut context, &options).unwrap_err();
        match err {
            BatonError::ArtifactNotFound(p) => {
                assert!(p.contains("nonexistent"), "path was: {p}");
            }
            other => panic!("expected ArtifactNotFound, got: {other}"),
        }
    }

    #[test]
    fn run_gate_artifact_is_directory() {
        let dir = TempDir::new().unwrap();
        let v = ValidatorBuilder::script("dummy", "true").build();
        let gate = th::gate("test-gate", vec![v]);
        let config = th::config_for_gate(gate.clone());
        let mut artifact = Artifact::from_string("placeholder");
        artifact.path = Some(dir.path().to_path_buf());
        let mut context = Context::new();
        let options = RunOptions::new();

        let err = run_gate(&gate, &config, &mut artifact, &mut context, &options).unwrap_err();
        match err {
            BatonError::ArtifactIsDirectory(p) => {
                assert!(p.contains(dir.path().to_str().unwrap()), "path was: {p}");
            }
            other => panic!("expected ArtifactIsDirectory, got: {other}"),
        }
    }

    #[test]
    fn run_gate_context_not_found() {
        let tmp = TempDir::new().unwrap();
        let artifact_path = tmp.path().join("artifact.txt");
        std::fs::write(&artifact_path, "hello").unwrap();
        let mut artifact = Artifact::from_file(&artifact_path).unwrap();

        let v = ValidatorBuilder::script("dummy", "true").build();
        let gate = th::gate("test-gate", vec![v]);
        let config = th::config_for_gate(gate.clone());

        let mut context = Context::new();
        context.add_string("bogus".into(), "temp".into());
        // Mutate the path to a nonexistent file
        context.items.get_mut("bogus").unwrap().path =
            Some(std::path::PathBuf::from("/nonexistent/context.txt"));

        let options = RunOptions::new();
        let err = run_gate(&gate, &config, &mut artifact, &mut context, &options).unwrap_err();
        match err {
            BatonError::ContextNotFound { name, path } => {
                assert_eq!(name, "bogus");
                assert!(path.contains("nonexistent"), "path was: {path}");
            }
            other => panic!("expected ContextNotFound, got: {other}"),
        }
    }

    #[test]
    fn run_gate_context_is_directory() {
        let tmp = TempDir::new().unwrap();
        let artifact_path = tmp.path().join("artifact.txt");
        std::fs::write(&artifact_path, "hello").unwrap();
        let mut artifact = Artifact::from_file(&artifact_path).unwrap();

        let context_dir = TempDir::new().unwrap();
        let v = ValidatorBuilder::script("dummy", "true").build();
        let gate = th::gate("test-gate", vec![v]);
        let config = th::config_for_gate(gate.clone());

        let mut context = Context::new();
        context.add_string("dirctx".into(), "temp".into());
        context.items.get_mut("dirctx").unwrap().path = Some(context_dir.path().to_path_buf());

        let options = RunOptions::new();
        let err = run_gate(&gate, &config, &mut artifact, &mut context, &options).unwrap_err();
        match err {
            BatonError::ContextIsDirectory { name, path } => {
                assert_eq!(name, "dirctx");
                assert!(
                    path.contains(context_dir.path().to_str().unwrap()),
                    "path was: {path}"
                );
            }
            other => panic!("expected ContextIsDirectory, got: {other}"),
        }
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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
        let v = ValidatorBuilder::human("human-ph", "Review {artifact_content} please").build();
        let mut art = Artifact::from_string("fn main() {}");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.warnings, vec!["w1", "w2"]);
    }

    // ─── run_gate: required context error message format ─

    #[test]
    fn gate_required_context_error_includes_gate_name() {
        let mut gate = th::gate(
            "my-gate",
            vec![ValidatorBuilder::script("a", "exit 0").build()],
        );
        gate.context.insert(
            "design-doc".into(),
            ContextSlot {
                description: None,
                required: true,
            },
        );
        let config = th::config_for_gate(gate.clone());
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let err = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("design-doc"),
            "expected context name in error: {msg}"
        );
        assert!(
            msg.contains("my-gate"),
            "expected gate name in error: {msg}"
        );
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Warn, Status::Error, Status::Fail];

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
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
        let response = serde_json::json!({
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5
            }
        });

        let (port, handle) = start_mock_server(200, &response.to_string());
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("empty or malformed"));
        // Cost should still be extracted even on empty content
        assert!(result.cost.is_some());

        handle.join().unwrap();
    }

    // ─── LLM completion: generic HTTP error (e.g. 503) ───

    #[test]
    fn llm_completion_http_503() {
        let (port, handle) = start_mock_server(503, r#"{"error": "service unavailable"}"#);
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result.feedback.as_ref().unwrap().contains("HTTP 503"));

        handle.join().unwrap();
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

        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Warn, Status::Fail];

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, &mut art, &mut ctx, &opts).unwrap();
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
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
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut art, &mut ctx, &prior, None);
        assert_eq!(result.status, Status::Pass);
    }

    // ─── LLM completion: HTTP 403 ────────────────────────

    #[test]
    fn llm_completion_http_403() {
        let (port, handle) = start_mock_server(403, r#"{"error": "forbidden"}"#);
        let config = th::config_with_provider(&format!("http://127.0.0.1:{port}"));

        let v = ValidatorBuilder::llm("llm-check", "Review this").build();
        let mut art = Artifact::from_string("hello");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();

        let result = execute_validator(&v, &mut art, &mut ctx, &prior, Some(&config));
        assert_eq!(result.status, Status::Error);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("Authentication failed"));

        handle.join().unwrap();
    }
}
