//! Shared test helpers for constructing common types.
//!
//! This module is only compiled in test builds. It provides factory functions
//! and a builder for `ValidatorConfig` to reduce boilerplate across test modules.

use crate::config::*;
use crate::types::*;
use chrono::Utc;
use std::collections::BTreeMap;

// ─── ValidatorResult factories ──────────────────────────

/// Creates a `ValidatorResult` with the given name and status, zero duration, no feedback/cost.
pub fn result(name: &str, status: Status) -> ValidatorResult {
    ValidatorResult {
        name: name.into(),
        status,
        feedback: None,
        duration_ms: 0,
        cost: None,
    }
}

/// Creates a `ValidatorResult` with feedback.
pub fn result_with_feedback(name: &str, status: Status, feedback: &str) -> ValidatorResult {
    ValidatorResult {
        name: name.into(),
        status,
        feedback: Some(feedback.into()),
        duration_ms: 0,
        cost: None,
    }
}

/// Creates a map with "lint" (Pass) and "typecheck" (Fail) results.
/// This is the standard prior-results fixture used across run_if and placeholder tests.
pub fn prior_results() -> BTreeMap<String, ValidatorResult> {
    let mut map = BTreeMap::new();
    map.insert("lint".into(), result("lint", Status::Pass));
    map.insert(
        "typecheck".into(),
        result_with_feedback("typecheck", Status::Fail, "error"),
    );
    map
}

/// Like `prior_results` but with richer feedback for placeholder tests.
pub fn prior_results_detailed() -> BTreeMap<String, ValidatorResult> {
    let mut map = BTreeMap::new();
    map.insert(
        "lint".into(),
        ValidatorResult {
            name: "lint".into(),
            status: Status::Pass,
            feedback: None,
            duration_ms: 50,
            cost: None,
        },
    );
    map.insert(
        "typecheck".into(),
        ValidatorResult {
            name: "typecheck".into(),
            status: Status::Fail,
            feedback: Some("type error on line 5".into()),
            duration_ms: 200,
            cost: None,
        },
    );
    map
}

// ─── ValidatorConfig builder ────────────────────────────

/// Builder for `ValidatorConfig` with sensible defaults.
///
/// ```ignore
/// let v = ValidatorBuilder::script("lint", "echo PASS").blocking(false).build();
/// let v = ValidatorBuilder::llm("check", "Review this").model("gpt-4").build();
/// ```
pub struct ValidatorBuilder {
    config: ValidatorConfig,
}

impl ValidatorBuilder {
    fn base(name: &str, vtype: ValidatorType) -> Self {
        Self {
            config: ValidatorConfig {
                name: name.into(),
                validator_type: vtype,
                blocking: true,
                run_if: None,
                timeout_seconds: 300,
                tags: vec![],
                command: None,
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
            },
        }
    }

    /// Script validator with a command.
    pub fn script(name: &str, command: &str) -> Self {
        let mut b = Self::base(name, ValidatorType::Script);
        b.config.command = Some(command.into());
        b
    }

    /// LLM validator with a prompt.
    pub fn llm(name: &str, prompt: &str) -> Self {
        let mut b = Self::base(name, ValidatorType::Llm);
        b.config.prompt = Some(prompt.into());
        b.config.timeout_seconds = 30;
        b.config.max_tokens = Some(4096);
        b.config.model = Some("test-model".into());
        b
    }

    /// Human validator with a prompt.
    pub fn human(name: &str, prompt: &str) -> Self {
        let mut b = Self::base(name, ValidatorType::Human);
        b.config.prompt = Some(prompt.into());
        b
    }

    pub fn blocking(mut self, blocking: bool) -> Self {
        self.config.blocking = blocking;
        self
    }

    pub fn run_if(mut self, expr: &str) -> Self {
        self.config.run_if = Some(expr.into());
        self
    }

    pub fn tags(mut self, tags: Vec<&str>) -> Self {
        self.config.tags = tags.into_iter().map(String::from).collect();
        self
    }

    pub fn warn_exit_codes(mut self, codes: Vec<i32>) -> Self {
        self.config.warn_exit_codes = codes;
        self
    }

    pub fn model(mut self, model: &str) -> Self {
        self.config.model = Some(model.into());
        self
    }

    pub fn mode(mut self, mode: LlmMode) -> Self {
        self.config.mode = mode;
        self
    }

    pub fn response_format(mut self, format: ResponseFormat) -> Self {
        self.config.response_format = format;
        self
    }

    pub fn provider(mut self, provider: &str) -> Self {
        self.config.provider = provider.into();
        self
    }

    pub fn system_prompt(mut self, prompt: &str) -> Self {
        self.config.system_prompt = Some(prompt.into());
        self
    }

    pub fn no_model(mut self) -> Self {
        self.config.model = None;
        self
    }

    pub fn runtime(mut self, runtime: &str) -> Self {
        self.config.runtime = Some(runtime.into());
        self
    }

    pub fn sandbox(mut self, sandbox: bool) -> Self {
        self.config.sandbox = Some(sandbox);
        self
    }

    pub fn max_iterations(mut self, n: u32) -> Self {
        self.config.max_iterations = Some(n);
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.config.env.insert(key.into(), value.into());
        self
    }

    pub fn working_dir(mut self, dir: &str) -> Self {
        self.config.working_dir = Some(dir.into());
        self
    }

    pub fn context_refs(mut self, refs: Vec<&str>) -> Self {
        self.config.context_refs = refs.into_iter().map(String::from).collect();
        self
    }

    pub fn build(self) -> ValidatorConfig {
        self.config
    }
}

// ─── GateConfig factory ─────────────────────────────────

/// Creates a `GateConfig` with no context slots.
pub fn gate(name: &str, validators: Vec<ValidatorConfig>) -> GateConfig {
    GateConfig {
        name: name.into(),
        description: None,
        context: BTreeMap::new(),
        validators,
    }
}

// ─── BatonConfig factory ────────────────────────────────

/// Creates a minimal `BatonConfig` wrapping a single gate, with `/tmp` paths.
pub fn config_for_gate(g: GateConfig) -> BatonConfig {
    let mut gates = BTreeMap::new();
    gates.insert(g.name.clone(), g);
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

/// Creates a `BatonConfig` with a custom provider (for LLM mock tests).
pub fn config_with_provider(api_base: &str) -> BatonConfig {
    let mut providers = BTreeMap::new();
    providers.insert(
        "default".into(),
        Provider {
            api_base: api_base.into(),
            api_key_env: "".into(),
            default_model: "test-model".into(),
        },
    );

    let mut gates = BTreeMap::new();
    gates.insert(
        "test".into(),
        GateConfig {
            name: "test".into(),
            description: None,
            context: BTreeMap::new(),
            validators: vec![],
        },
    );

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
        providers,
        runtimes: BTreeMap::new(),
        gates,
        config_dir: "/tmp".into(),
    }
}

// ─── Verdict factory ────────────────────────────────────

/// Creates a `Verdict` with one validator result and optional cost metadata.
pub fn verdict(status: VerdictStatus) -> Verdict {
    Verdict {
        status,
        gate: "test-gate".into(),
        failed_at: if status != VerdictStatus::Pass {
            Some("lint".into())
        } else {
            None
        },
        feedback: if status != VerdictStatus::Pass {
            Some("something failed".into())
        } else {
            None
        },
        duration_ms: 100,
        timestamp: Utc::now(),
        artifact_hash: "abc123".into(),
        context_hash: "def456".into(),
        warnings: vec![],
        suppressed: vec![],
        history: vec![ValidatorResult {
            name: "lint".into(),
            status: if status == VerdictStatus::Pass {
                Status::Pass
            } else {
                Status::Fail
            },
            feedback: None,
            duration_ms: 50,
            cost: Some(Cost {
                input_tokens: Some(100),
                output_tokens: Some(50),
                model: Some("test-model".into()),
                estimated_usd: Some(0.001),
            }),
        }],
    }
}
