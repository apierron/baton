//! Configuration parsing and validation for baton.toml files.
//!
//! Two-stage design: [`parse_config`] deserializes TOML into validated structures,
//! [`validate_config`] checks semantic correctness (e.g., forward references,
//! missing providers, undefined context slots).

use crate::error::{BatonError, Result};
use crate::placeholder::resolve_env_vars;
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

// ─── Raw TOML structures ────────────────────────────────

/// Raw deserialized baton.toml before validation.
#[derive(Debug, Deserialize)]
pub struct RawConfig {
    pub version: String,
    #[serde(default)]
    pub defaults: RawDefaults,
    #[serde(default)]
    pub providers: BTreeMap<String, RawProvider>,
    #[serde(default)]
    pub runtimes: BTreeMap<String, RawRuntime>,
    pub gates: BTreeMap<String, RawGate>,
}

/// Raw default settings from `[defaults]`.
#[derive(Debug, Deserialize, Default)]
pub struct RawDefaults {
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_blocking")]
    pub blocking: bool,
    #[serde(default = "default_prompts_dir")]
    pub prompts_dir: String,
    #[serde(default = "default_log_dir")]
    pub log_dir: String,
    #[serde(default = "default_history_db")]
    pub history_db: String,
    #[serde(default = "default_tmp_dir")]
    pub tmp_dir: String,
}

fn default_timeout() -> u64 {
    300
}
fn default_blocking() -> bool {
    true
}
fn default_prompts_dir() -> String {
    "./prompts".into()
}
fn default_log_dir() -> String {
    "./.baton/logs".into()
}
fn default_history_db() -> String {
    "./.baton/history.db".into()
}
fn default_tmp_dir() -> String {
    "./.baton/tmp".into()
}

/// Raw LLM provider entry from `[providers.<name>]`.
#[derive(Debug, Deserialize, Clone)]
pub struct RawProvider {
    pub api_base: String,
    pub api_key_env: String,
    pub default_model: String,
}

/// Raw agent runtime entry from `[runtimes.<name>]`.
#[derive(Debug, Deserialize, Clone)]
pub struct RawRuntime {
    #[serde(rename = "type")]
    pub runtime_type: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default = "default_sandbox")]
    pub sandbox: bool,
    #[serde(default = "default_runtime_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

fn default_sandbox() -> bool {
    true
}
fn default_runtime_timeout() -> u64 {
    600
}
fn default_max_iterations() -> u32 {
    30
}

/// Raw gate entry from `[gates.<name>]`.
#[derive(Debug, Deserialize)]
pub struct RawGate {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub context: BTreeMap<String, RawContextSlot>,
    pub validators: Vec<RawValidator>,
}

/// Raw context slot declaration on a gate.
#[derive(Debug, Deserialize, Clone)]
pub struct RawContextSlot {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// Raw validator entry from `[[gates.<name>.validators]]`.
#[derive(Debug, Deserialize, Clone)]
pub struct RawValidator {
    pub name: String,
    #[serde(rename = "type")]
    pub validator_type: String,
    #[serde(default)]
    pub blocking: Option<bool>,
    #[serde(default)]
    pub run_if: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub tags: Vec<String>,

    // Script fields
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub warn_exit_codes: Vec<i32>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,

    // LLM fields
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub context_refs: Vec<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub response_format: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,

    // Session fields
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub sandbox: Option<bool>,
    #[serde(default)]
    pub max_iterations: Option<u32>,
}

// ─── Validated config structures ─────────────────────────

/// Fully validated baton configuration, ready for execution.
#[derive(Debug, Clone)]
pub struct BatonConfig {
    pub version: String,
    pub defaults: Defaults,
    pub providers: BTreeMap<String, Provider>,
    pub runtimes: BTreeMap<String, Runtime>,
    pub gates: BTreeMap<String, GateConfig>,
    pub config_dir: PathBuf,
}

/// Resolved default settings with absolute paths.
#[derive(Debug, Clone)]
pub struct Defaults {
    pub timeout_seconds: u64,
    pub blocking: bool,
    pub prompts_dir: PathBuf,
    pub log_dir: PathBuf,
    pub history_db: PathBuf,
    pub tmp_dir: PathBuf,
}

/// LLM API provider configuration.
#[derive(Debug, Clone)]
pub struct Provider {
    pub api_base: String,
    pub api_key_env: String,
    pub default_model: String,
}

/// Agent runtime configuration (e.g., OpenHands).
#[derive(Debug, Clone)]
pub struct Runtime {
    pub runtime_type: String,
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub default_model: Option<String>,
    pub sandbox: bool,
    pub timeout_seconds: u64,
    pub max_iterations: u32,
}

/// A single validation gate with its validators.
#[derive(Debug, Clone)]
pub struct GateConfig {
    pub name: String,
    pub description: Option<String>,
    pub context: BTreeMap<String, ContextSlot>,
    pub validators: Vec<ValidatorConfig>,
}

/// Declared context slot on a gate.
#[derive(Debug, Clone)]
pub struct ContextSlot {
    pub description: Option<String>,
    pub required: bool,
}

/// The type of a validator: script, LLM, or human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidatorType {
    Script,
    Llm,
    Human,
}

/// LLM interaction mode: single completion or multi-step session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmMode {
    Completion,
    Session,
}

/// Expected response format from an LLM validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseFormat {
    Verdict,
    Freeform,
}

/// Fully resolved validator configuration with defaults applied.
#[derive(Debug, Clone)]
pub struct ValidatorConfig {
    pub name: String,
    pub validator_type: ValidatorType,
    pub blocking: bool,
    pub run_if: Option<String>,
    pub timeout_seconds: u64,
    pub tags: Vec<String>,

    // Script
    pub command: Option<String>,
    pub warn_exit_codes: Vec<i32>,
    pub working_dir: Option<String>,
    pub env: BTreeMap<String, String>,

    // LLM
    pub mode: LlmMode,
    pub provider: String,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub context_refs: Vec<String>,
    pub temperature: f64,
    pub response_format: ResponseFormat,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,

    // Session
    pub runtime: Option<String>,
    pub sandbox: Option<bool>,
    pub max_iterations: Option<u32>,
}

// ─── Validation result ───────────────────────────────────

/// Collection of validation errors and warnings from [`validate_config`].
#[derive(Debug, Clone, Default)]
pub struct ConfigValidation {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ConfigValidation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if any validation errors were recorded.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

// ─── Parsing ─────────────────────────────────────────────

/// Parse a baton.toml from a string, with `config_dir` as the base for relative paths.
pub fn parse_config(toml_str: &str, config_dir: &Path) -> Result<BatonConfig> {
    let raw: RawConfig = toml::from_str(toml_str)?;

    if raw.version != "0.4" {
        return Err(BatonError::ConfigError(format!(
            "Unsupported version '{}'. Expected '0.4'.",
            raw.version
        )));
    }

    if raw.gates.is_empty() {
        return Err(BatonError::ConfigError(
            "No gates defined. At least one gate is required.".into(),
        ));
    }

    let defaults = Defaults {
        timeout_seconds: raw.defaults.timeout_seconds,
        blocking: raw.defaults.blocking,
        prompts_dir: config_dir.join(&raw.defaults.prompts_dir),
        log_dir: config_dir.join(&raw.defaults.log_dir),
        history_db: config_dir.join(&raw.defaults.history_db),
        tmp_dir: config_dir.join(&raw.defaults.tmp_dir),
    };

    let mut providers = BTreeMap::new();
    for (name, raw_p) in &raw.providers {
        let mut api_base = resolve_env_vars(&raw_p.api_base)
            .map_err(|e| BatonError::ConfigError(format!("Provider '{name}': {e}")))?;
        // Strip trailing slash
        if api_base.ends_with('/') {
            api_base.pop();
        }
        providers.insert(
            name.clone(),
            Provider {
                api_base,
                api_key_env: raw_p.api_key_env.clone(),
                default_model: raw_p.default_model.clone(),
            },
        );
    }

    let mut runtimes = BTreeMap::new();
    for (name, raw_r) in &raw.runtimes {
        runtimes.insert(
            name.clone(),
            Runtime {
                runtime_type: raw_r.runtime_type.clone(),
                base_url: raw_r.base_url.clone(),
                api_key_env: raw_r.api_key_env.clone(),
                default_model: raw_r.default_model.clone(),
                sandbox: raw_r.sandbox,
                timeout_seconds: raw_r.timeout_seconds,
                max_iterations: raw_r.max_iterations,
            },
        );
    }

    let mut gates = BTreeMap::new();
    for (gate_name, raw_gate) in &raw.gates {
        if raw_gate.validators.is_empty() {
            return Err(BatonError::ConfigError(format!(
                "Gate '{gate_name}' has no validators."
            )));
        }

        let context: BTreeMap<String, ContextSlot> = raw_gate
            .context
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    ContextSlot {
                        description: v.description.clone(),
                        required: v.required,
                    },
                )
            })
            .collect();

        let mut validators = Vec::new();
        let mut seen_names: HashSet<String> = HashSet::new();

        for raw_v in &raw_gate.validators {
            // Validate name
            let valid_name = raw_v
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && !raw_v.name.is_empty();
            if !valid_name {
                return Err(BatonError::ConfigError(format!(
                    "Validator name '{}' contains invalid characters. Must match [A-Za-z0-9_-]+.",
                    raw_v.name
                )));
            }
            if !seen_names.insert(raw_v.name.clone()) {
                return Err(BatonError::ConfigError(format!(
                    "Gate '{gate_name}': duplicate validator name '{}'.",
                    raw_v.name
                )));
            }

            let vtype = match raw_v.validator_type.as_str() {
                "script" => ValidatorType::Script,
                "llm" => ValidatorType::Llm,
                "human" => ValidatorType::Human,
                other => {
                    return Err(BatonError::ConfigError(format!(
                        "Validator '{}': unknown type '{other}'. Expected 'script', 'llm', or 'human'.",
                        raw_v.name
                    )));
                }
            };

            // Validate required fields per type
            match &vtype {
                ValidatorType::Script => {
                    if raw_v.command.is_none() {
                        return Err(BatonError::ConfigError(format!(
                            "Validator '{}': missing required field 'command'.",
                            raw_v.name
                        )));
                    }
                }
                ValidatorType::Llm => {
                    if raw_v.prompt.is_none() {
                        return Err(BatonError::ConfigError(format!(
                            "Validator '{}': missing required field 'prompt'.",
                            raw_v.name
                        )));
                    }
                }
                ValidatorType::Human => {
                    if raw_v.prompt.is_none() {
                        return Err(BatonError::ConfigError(format!(
                            "Validator '{}': missing required field 'prompt'.",
                            raw_v.name
                        )));
                    }
                }
            }

            let mode = match raw_v.mode.as_deref() {
                Some("session") => LlmMode::Session,
                Some("completion") | None => LlmMode::Completion,
                Some(other) => {
                    return Err(BatonError::ConfigError(format!(
                        "Validator '{}': invalid mode '{other}'. Expected 'completion' or 'session'.",
                        raw_v.name
                    )));
                }
            };

            let response_format = match raw_v.response_format.as_deref() {
                Some("freeform") => ResponseFormat::Freeform,
                Some("verdict") | None => ResponseFormat::Verdict,
                Some(other) => {
                    return Err(BatonError::ConfigError(format!(
                        "Validator '{}': invalid response_format '{other}'.",
                        raw_v.name
                    )));
                }
            };

            // warn_exit_codes must not contain 0
            if raw_v.warn_exit_codes.contains(&0) {
                return Err(BatonError::ConfigError(format!(
                    "Validator '{}': warn_exit_codes must not contain 0 (exit code 0 is always pass).",
                    raw_v.name
                )));
            }

            validators.push(ValidatorConfig {
                name: raw_v.name.clone(),
                validator_type: vtype,
                blocking: raw_v.blocking.unwrap_or(defaults.blocking),
                run_if: raw_v.run_if.clone(),
                timeout_seconds: raw_v.timeout_seconds.unwrap_or(defaults.timeout_seconds),
                tags: raw_v.tags.clone(),
                command: raw_v.command.clone(),
                warn_exit_codes: raw_v.warn_exit_codes.clone(),
                working_dir: raw_v.working_dir.clone(),
                env: raw_v.env.clone(),
                mode,
                provider: raw_v.provider.clone().unwrap_or("default".into()),
                model: raw_v.model.clone(),
                prompt: raw_v.prompt.clone(),
                context_refs: raw_v.context_refs.clone(),
                temperature: raw_v.temperature.unwrap_or(0.0),
                response_format,
                max_tokens: raw_v.max_tokens,
                system_prompt: raw_v.system_prompt.clone(),
                runtime: raw_v.runtime.clone(),
                sandbox: raw_v.sandbox,
                max_iterations: raw_v.max_iterations,
            });
        }

        gates.insert(
            gate_name.clone(),
            GateConfig {
                name: gate_name.clone(),
                description: raw_gate.description.clone(),
                context,
                validators,
            },
        );
    }

    Ok(BatonConfig {
        version: raw.version,
        defaults,
        providers,
        runtimes,
        gates,
        config_dir: config_dir.to_path_buf(),
    })
}

/// Validate a config and return errors/warnings without aborting.
pub fn validate_config(config: &BatonConfig) -> ConfigValidation {
    let mut v = ConfigValidation::new();

    for (gate_name, gate) in &config.gates {
        let validator_names: Vec<&str> = gate.validators.iter().map(|v| v.name.as_str()).collect();

        for (idx, val) in gate.validators.iter().enumerate() {
            // Check run_if references
            if let Some(ref run_if) = val.run_if {
                validate_run_if_expr(run_if, &validator_names, idx, &val.name, gate_name, &mut v);
            }

            // Check context_refs reference defined context slots
            for cref in &val.context_refs {
                if !gate.context.contains_key(cref) {
                    v.errors.push(format!(
                        "Validator '{}': context_refs includes '{cref}' which is not defined on gate '{gate_name}'.",
                        val.name
                    ));
                }
            }

            // Check provider references (LLM validators)
            if val.validator_type == ValidatorType::Llm {
                if !config.providers.contains_key(&val.provider) && val.provider != "default" {
                    v.errors.push(format!(
                        "Validator '{}': provider '{}' is not defined in [providers].",
                        val.name, val.provider
                    ));
                }

                // Session mode requires runtime
                if val.mode == LlmMode::Session && val.runtime.is_none() {
                    v.errors.push(format!(
                        "Validator '{}': mode 'session' requires a 'runtime' field.",
                        val.name
                    ));
                }

                // Completion mode with runtime is a warning
                if val.mode == LlmMode::Completion && val.runtime.is_some() {
                    v.warnings.push(format!(
                        "Validator '{}': runtime field ignored in completion mode.",
                        val.name
                    ));
                }

                // Check runtime reference
                if let Some(ref rt) = val.runtime {
                    if !config.runtimes.contains_key(rt) {
                        v.errors.push(format!(
                            "Validator '{}': runtime '{rt}' is not defined in [runtimes].",
                            val.name
                        ));
                    }
                }

                // Blocking + freeform warning
                if val.response_format == ResponseFormat::Freeform && val.blocking {
                    v.warnings.push(format!(
                        "Validator '{}': blocking has no effect with response_format 'freeform' (freeform always returns warn).",
                        val.name
                    ));
                }
            }
        }
    }

    // Check provider API key env vars
    for (name, provider) in &config.providers {
        if !provider.api_key_env.is_empty() && std::env::var(&provider.api_key_env).is_err() {
            v.errors.push(format!(
                "Provider '{name}': env var '{}' is not set.",
                provider.api_key_env
            ));
        }
    }

    v
}

fn validate_run_if_expr(
    expr: &str,
    validator_names: &[&str],
    current_idx: usize,
    current_name: &str,
    _gate_name: &str,
    v: &mut ConfigValidation,
) {
    // Tokenize on " and " / " or "
    let tokens = split_run_if(expr);

    for token in &tokens {
        if *token == "and" || *token == "or" {
            continue;
        }
        // Validate atom: "<name>.status == <value>"
        let parts: Vec<&str> = token.split(".status == ").collect();
        if parts.len() != 2 {
            v.errors.push(format!(
                "Validator '{current_name}': invalid run_if expression: '{expr}'."
            ));
            return;
        }
        let ref_name = parts[0].trim();
        let expected = parts[1].trim();

        if !["pass", "fail", "warn", "error", "skip"].contains(&expected) {
            v.errors.push(format!(
                "Validator '{current_name}': invalid run_if expression: '{expr}'."
            ));
            return;
        }

        // Check referenced validator exists
        if let Some(ref_idx) = validator_names.iter().position(|&n| n == ref_name) {
            if ref_idx >= current_idx {
                v.errors.push(format!(
                    "Validator '{current_name}': run_if references '{ref_name}' which appears later in the pipeline."
                ));
            }
        } else {
            v.errors.push(format!(
                "Validator '{current_name}': run_if references unknown validator '{ref_name}'."
            ));
        }
    }
}

/// Split run_if expression into tokens: atoms and operators.
pub fn split_run_if(expr: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut remaining = expr.trim();

    while !remaining.is_empty() {
        if let Some(pos) = remaining.find(" and ") {
            let before_or = remaining.find(" or ");
            if let Some(or_pos) = before_or {
                if or_pos < pos {
                    tokens.push(remaining[..or_pos].trim().to_string());
                    tokens.push("or".to_string());
                    remaining = &remaining[or_pos + 4..];
                    continue;
                }
            }
            tokens.push(remaining[..pos].trim().to_string());
            tokens.push("and".to_string());
            remaining = &remaining[pos + 5..];
        } else if let Some(pos) = remaining.find(" or ") {
            tokens.push(remaining[..pos].trim().to_string());
            tokens.push("or".to_string());
            remaining = &remaining[pos + 4..];
        } else {
            tokens.push(remaining.trim().to_string());
            break;
        }
    }

    tokens
}

/// Discover baton.toml by searching upward from `start_dir`.
pub fn discover_config(start_dir: &Path) -> Result<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join("baton.toml");
        if candidate.exists() {
            return Ok(candidate);
        }
        // Stop at .git
        if dir.join(".git").exists() {
            break;
        }
        // Go up
        if !dir.pop() {
            break;
        }
    }

    Err(BatonError::ConfigError(format!(
        "No baton.toml found (searched from {})",
        start_dir.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn config_dir() -> PathBuf {
        std::env::temp_dir().join("baton-test")
    }

    // ═══════════════════════════════════════════════════════════════
    // Internal implementation tests
    // ═══════════════════════════════════════════════════════════════

    // ─── split_run_if ────────────────────────────────

    #[test]
    fn split_run_if_simple() {
        let tokens = split_run_if("lint.status == pass");
        assert_eq!(tokens, vec!["lint.status == pass"]);
    }

    #[test]
    fn split_run_if_and() {
        let tokens = split_run_if("lint.status == pass and typecheck.status == pass");
        assert_eq!(
            tokens,
            vec!["lint.status == pass", "and", "typecheck.status == pass"]
        );
    }

    #[test]
    fn split_run_if_or() {
        let tokens = split_run_if("lint.status == fail or typecheck.status == fail");
        assert_eq!(
            tokens,
            vec!["lint.status == fail", "or", "typecheck.status == fail"]
        );
    }

    #[test]
    fn split_run_if_mixed() {
        let tokens = split_run_if("a.status == pass and b.status == pass or c.status == pass");
        assert_eq!(
            tokens,
            vec![
                "a.status == pass",
                "and",
                "b.status == pass",
                "or",
                "c.status == pass"
            ]
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // ═══════════════════════════════════════════════════════════════

    // ─── Basic parsing ───────────────────────────────

    #[test]
    fn parse_minimal_config() {
        let toml = r#"
version = "0.4"
[gates.test]
description = "Test gate"
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(config.version, "0.4");
        assert!(config.gates.contains_key("test"));
        assert_eq!(config.gates["test"].validators.len(), 1);
        assert_eq!(config.gates["test"].validators[0].name, "check");
    }

    #[test]
    fn parse_wrong_version() {
        let toml = r#"
version = "0.3"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("0.3"), "Error: {err}");
    }

    #[test]
    fn parse_no_gates() {
        let toml = r#"
version = "0.4"
[gates]
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No gates"), "Error: {err}");
    }

    #[test]
    fn parse_gate_no_validators() {
        let toml = r#"
version = "0.4"
[gates.test]
description = "Empty gate"
validators = []
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no validators"), "Error: {err}");
    }

    // ─── Validator name validation ───────────────────

    #[test]
    fn invalid_validator_name() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "bad name!"
type = "script"
command = "echo ok"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid characters"), "Error: {err}");
    }

    #[test]
    fn duplicate_validator_name() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok2"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("duplicate"), "Error: {err}");
    }

    // ─── Validator type validation ───────────────────

    #[test]
    fn unknown_validator_type() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "unknown"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unknown type"), "Error: {err}");
    }

    #[test]
    fn script_missing_command() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("command"), "Error: {err}");
    }

    #[test]
    fn llm_missing_prompt() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("prompt"), "Error: {err}");
    }

    #[test]
    fn human_missing_prompt() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "human"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("prompt"), "Error: {err}");
    }

    // ─── warn_exit_codes ─────────────────────────────

    #[test]
    fn warn_exit_codes_contains_zero() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
warn_exit_codes = [0, 2]
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("warn_exit_codes must not contain 0"),
            "Error: {err}"
        );
    }

    // ─── Defaults ────────────────────────────────────

    #[test]
    fn defaults_applied() {
        let toml = r#"
version = "0.4"
[defaults]
timeout_seconds = 600
blocking = false

[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(config.defaults.timeout_seconds, 600);
        assert!(!config.defaults.blocking);
        assert_eq!(config.gates["test"].validators[0].timeout_seconds, 600);
        assert!(!config.gates["test"].validators[0].blocking);
    }

    #[test]
    fn validator_overrides_defaults() {
        let toml = r#"
version = "0.4"
[defaults]
timeout_seconds = 600
blocking = false

[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
timeout_seconds = 30
blocking = true
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(config.gates["test"].validators[0].timeout_seconds, 30);
        assert!(config.gates["test"].validators[0].blocking);
    }

    // ─── Provider parsing ────────────────────────────

    #[test]
    fn provider_trailing_slash_stripped() {
        let toml = r#"
version = "0.4"
[providers.default]
api_base = "https://api.example.com/"
api_key_env = ""
default_model = "test-model"

[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(
            config.providers["default"].api_base,
            "https://api.example.com"
        );
    }

    // ─── Validation ──────────────────────────────────

    #[test]
    fn validate_run_if_references_nonexistent() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "a"
type = "script"
command = "echo ok"
[[gates.test.validators]]
name = "b"
type = "script"
command = "echo ok"
run_if = "nonexistent.status == pass"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        assert!(validation.errors[0].contains("unknown validator"));
    }

    #[test]
    fn validate_run_if_forward_reference() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "a"
type = "script"
command = "echo ok"
run_if = "b.status == pass"
[[gates.test.validators]]
name = "b"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        assert!(validation.errors[0].contains("later in the pipeline"));
    }

    #[test]
    fn validate_context_refs_undefined() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
prompt = "Review this"
context_refs = ["nonexistent"]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        assert!(validation.errors[0].contains("nonexistent"));
    }

    #[test]
    fn validate_session_without_runtime() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
mode = "session"
prompt = "Review this"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        assert!(validation.errors[0].contains("runtime"));
    }

    #[test]
    fn validate_completion_with_runtime_warning() {
        let toml = r#"
version = "0.4"
[runtimes.test]
type = "test"
base_url = "http://localhost"

[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
mode = "completion"
prompt = "Review this"
runtime = "test"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(!validation.warnings.is_empty());
        assert!(validation.warnings[0].contains("ignored in completion mode"));
    }

    #[test]
    fn validate_freeform_blocking_warning() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
prompt = "Review this"
response_format = "freeform"
blocking = true
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(!validation.warnings.is_empty());
        assert!(validation.warnings[0].contains("blocking has no effect"));
    }

    // ─── Config discovery ────────────────────────────

    #[test]
    fn discover_config_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = discover_config(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn discover_config_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("baton.toml"), "version = \"0.4\"").unwrap();
        let result = discover_config(dir.path());
        assert!(result.is_ok());
        assert!(result.unwrap().ends_with("baton.toml"));
    }

    #[test]
    fn discover_config_in_parent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("baton.toml"), "version = \"0.4\"").unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        let result = discover_config(&subdir);
        assert!(result.is_ok());
    }

    #[test]
    fn discover_config_stops_at_git_boundary() {
        let dir = tempfile::tempdir().unwrap();
        // baton.toml above the .git boundary
        std::fs::write(dir.path().join("baton.toml"), "version = \"0.4\"").unwrap();
        // .git in a subdirectory creates a boundary
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        let nested = repo.join("src");
        std::fs::create_dir(&nested).unwrap();
        // Search from repo/src should stop at repo/.git and NOT find the parent baton.toml
        let result = discover_config(&nested);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No baton.toml found"));
    }

    #[test]
    fn discover_config_git_boundary_with_config_inside() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        std::fs::write(repo.join("baton.toml"), "version = \"0.4\"").unwrap();
        let nested = repo.join("src/deep");
        std::fs::create_dir_all(&nested).unwrap();
        // Config is inside the .git boundary, should be found
        let result = discover_config(&nested);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), repo.join("baton.toml"));
    }

    #[test]
    fn discover_config_deeply_nested() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("baton.toml"), "version = \"0.4\"").unwrap();
        let deep = dir.path().join("a/b/c/d/e");
        std::fs::create_dir_all(&deep).unwrap();
        let result = discover_config(&deep);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), dir.path().join("baton.toml"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_config_through_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("baton.toml"), "version = \"0.4\"").unwrap();
        let real_sub = dir.path().join("real");
        std::fs::create_dir(&real_sub).unwrap();
        let link = dir.path().join("linked");
        std::os::unix::fs::symlink(&real_sub, &link).unwrap();
        // Search from the symlinked path should still find baton.toml
        let result = discover_config(&link);
        assert!(result.is_ok());
    }

    #[test]
    fn discover_config_error_message_includes_start_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let result = discover_config(dir.path());
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains(&dir.path().display().to_string()));
    }

    // ─── Full config with all features ───────────────

    #[test]
    fn parse_full_config() {
        let toml = r#"
version = "0.4"

[defaults]
timeout_seconds = 300
blocking = true
prompts_dir = "./prompts"

[providers.default]
api_base = "https://api.example.com"
api_key_env = ""
default_model = "test-model"

[gates.code-review]
description = "Code review gate"

[gates.code-review.context.spec]
description = "The specification"
required = true

[[gates.code-review.validators]]
name = "lint"
type = "script"
command = "echo {artifact}"
blocking = true
tags = ["fast"]
warn_exit_codes = [2]

[[gates.code-review.validators]]
name = "llm-check"
type = "llm"
mode = "completion"
provider = "default"
prompt = "Check this code"
context_refs = ["spec"]
temperature = 0.0
response_format = "verdict"
blocking = true

[[gates.code-review.validators]]
name = "human-gate"
type = "human"
prompt = "Please review"
run_if = "llm-check.status == pass"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let gate = &config.gates["code-review"];
        assert_eq!(gate.validators.len(), 3);
        assert_eq!(gate.validators[0].warn_exit_codes, vec![2]);
        assert_eq!(gate.validators[0].tags, vec!["fast"]);
        assert_eq!(gate.validators[1].temperature, 0.0);
        assert_eq!(gate.validators[1].context_refs, vec!["spec"]);
        assert!(gate.context["spec"].required);
        assert_eq!(
            gate.validators[2].run_if,
            Some("llm-check.status == pass".into())
        );

        let validation = validate_config(&config);
        assert!(!validation.has_errors(), "Errors: {:?}", validation.errors);
    }

    // ─── Spec coverage (UNTESTED) ──────────────────────

    #[test]
    fn malformed_toml_returns_error() {
        let result = parse_config("not valid {toml", Path::new("."));
        assert!(result.is_err());
    }

    #[test]
    fn prompts_dir_default() {
        let toml = r#"
version = "0.4"
[defaults]
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let prompts_dir = config.defaults.prompts_dir.to_string_lossy();
        assert!(prompts_dir.contains("/prompts"), "Got: {prompts_dir}");
        assert!(
            config.defaults.prompts_dir.is_absolute(),
            "Expected absolute path, got: {prompts_dir}"
        );
    }

    #[test]
    fn log_dir_default() {
        let toml = r#"
version = "0.4"
[defaults]
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let log_dir = config.defaults.log_dir.to_string_lossy();
        assert!(log_dir.contains(".baton/logs"), "Got: {log_dir}");
    }

    #[test]
    fn history_db_default() {
        let toml = r#"
version = "0.4"
[defaults]
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let history_db = config.defaults.history_db.to_string_lossy();
        assert!(
            history_db.contains(".baton/history.db"),
            "Got: {history_db}"
        );
    }

    #[test]
    fn tmp_dir_default() {
        let toml = r#"
version = "0.4"
[defaults]
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let tmp_dir = config.defaults.tmp_dir.to_string_lossy();
        assert!(tmp_dir.contains(".baton/tmp"), "Got: {tmp_dir}");
    }

    #[test]
    fn path_resolution_with_config_dir() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, Path::new("/custom/dir")).unwrap();
        assert!(
            config.defaults.prompts_dir.starts_with("/custom/dir"),
            "prompts_dir: {:?}",
            config.defaults.prompts_dir
        );
        assert!(
            config.defaults.log_dir.starts_with("/custom/dir"),
            "log_dir: {:?}",
            config.defaults.log_dir
        );
        assert!(
            config.defaults.history_db.starts_with("/custom/dir"),
            "history_db: {:?}",
            config.defaults.history_db
        );
        assert!(
            config.defaults.tmp_dir.starts_with("/custom/dir"),
            "tmp_dir: {:?}",
            config.defaults.tmp_dir
        );
    }

    #[test]
    fn runtime_defaults() {
        let toml = r#"
version = "0.4"
[runtimes.test]
type = "openhands"
base_url = "http://localhost"

[gates.g]
[[gates.g.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let rt = &config.runtimes["test"];
        assert!(rt.sandbox);
        assert_eq!(rt.timeout_seconds, 600);
        assert_eq!(rt.max_iterations, 30);
    }

    #[test]
    fn runtime_fields_stored_verbatim() {
        let toml = r#"
version = "0.4"
[runtimes.custom]
type = "openhands"
base_url = "http://example.com:3000"
sandbox = false
timeout_seconds = 1200
max_iterations = 50
api_key_env = "MY_KEY"
default_model = "gpt-4"

[gates.g]
[[gates.g.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let rt = &config.runtimes["custom"];
        assert_eq!(rt.runtime_type, "openhands");
        assert_eq!(rt.base_url, "http://example.com:3000");
        assert!(!rt.sandbox);
        assert_eq!(rt.timeout_seconds, 1200);
        assert_eq!(rt.max_iterations, 50);
        assert_eq!(rt.api_key_env, Some("MY_KEY".into()));
        assert_eq!(rt.default_model, Some("gpt-4".into()));
    }

    #[test]
    fn invalid_mode_string() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
prompt = "Review"
mode = "invalid"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid mode"), "Error: {err}");
    }

    #[test]
    fn invalid_response_format() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
prompt = "Review"
response_format = "invalid"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid response_format"), "Error: {err}");
    }

    #[test]
    fn default_provider() {
        let toml = r#"
version = "0.4"
[gates.gate]
[[gates.gate.validators]]
name = "check"
type = "llm"
prompt = "Review"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(config.gates["gate"].validators[0].provider, "default");
    }

    #[test]
    fn config_dir_stored() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, Path::new("/my/project")).unwrap();
        assert_eq!(config.config_dir, PathBuf::from("/my/project"));
    }

    #[test]
    fn duplicate_name_across_gates_is_ok() {
        let toml = r#"
version = "0.4"
[gates.alpha]
[[gates.alpha.validators]]
name = "lint"
type = "script"
command = "echo ok"

[gates.beta]
[[gates.beta.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(config.gates["alpha"].validators[0].name, "lint");
        assert_eq!(config.gates["beta"].validators[0].name, "lint");
    }

    #[test]
    fn empty_validator_name() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = ""
type = "script"
command = "echo ok"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid characters"), "Error: {err}");
    }

    #[test]
    fn self_referencing_run_if() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "a"
type = "script"
command = "echo ok"
run_if = "a.status == pass"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        assert!(
            validation.errors[0].contains("later in the pipeline"),
            "Error: {}",
            validation.errors[0]
        );
    }

    #[test]
    fn undefined_non_default_provider() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
prompt = "Review"
provider = "nonexistent"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        assert!(
            validation.errors[0].contains("nonexistent"),
            "Error: {}",
            validation.errors[0]
        );
    }

    #[test]
    fn undefined_runtime_reference() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
prompt = "Review"
mode = "session"
runtime = "nonexistent"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        let has_runtime_err = validation
            .errors
            .iter()
            .any(|e| e.contains("not defined in [runtimes]"));
        assert!(has_runtime_err, "Errors: {:?}", validation.errors);
    }

    #[test]
    fn script_validator_with_provider_not_flagged() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
provider = "nonexistent"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            !validation.has_errors(),
            "Unexpected errors: {:?}",
            validation.errors
        );
    }

    #[test]
    fn api_key_env_validation() {
        let toml = r#"
version = "0.4"
[providers.myprovider]
api_base = "https://api.example.com"
api_key_env = "BATON_TEST_NONEXISTENT_VAR_XYZ"
default_model = "test-model"

[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        let err = validation.errors.join(" ");
        assert!(err.contains("myprovider"), "Error: {err}");
        assert!(
            err.contains("BATON_TEST_NONEXISTENT_VAR_XYZ"),
            "Error: {err}"
        );
    }

    #[test]
    fn multiple_simultaneous_validation_errors() {
        let toml = r#"
version = "0.4"
[gates.test]
[[gates.test.validators]]
name = "a"
type = "llm"
prompt = "Review"
provider = "nonexistent"
mode = "session"
runtime = "also-nonexistent"
context_refs = ["undefined-ctx"]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            validation.errors.len() > 1,
            "Expected multiple errors, got: {:?}",
            validation.errors
        );
    }

    #[test]
    fn whitespace_in_run_if() {
        let tokens = split_run_if("  a.status == pass  and  b.status == fail  ");
        assert_eq!(tokens, vec!["a.status == pass", "and", "b.status == fail"]);
    }

    #[test]
    fn names_containing_and_or() {
        let tokens = split_run_if("command.status == pass and mentor.status == fail");
        assert_eq!(
            tokens,
            vec!["command.status == pass", "and", "mentor.status == fail"]
        );
    }
}
