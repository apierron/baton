//! LLM and session validator execution.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::config::{BatonConfig, LlmMode, ResponseFormat};
use crate::error::BatonError;
use crate::placeholder::{resolve_placeholders, ResolutionWarnings};
use crate::prompt::{is_file_reference, resolve_prompt_value};
use crate::runtime::{self, CompletionRequest, SessionConfig, SessionStatus};
use crate::types::*;
use crate::verdict_parser::parse_verdict;

pub(super) fn execute_llm_validator(
    validator: &crate::config::ValidatorConfig,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::exec::execute_validator;
    use crate::test_helpers::{self as th, MockRuntimeAdapter, ValidatorBuilder};

    fn test_session_config() -> crate::runtime::SessionConfig {
        MockRuntimeAdapter::dummy_session_config()
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

        let verdict = crate::exec::run_gate(&gate, &config, vec![], &opts).unwrap();
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

        let verdict = crate::exec::run_gate(&gate, &config, vec![], &opts).unwrap();
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
}
