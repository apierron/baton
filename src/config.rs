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

// ─── StringOrList deserialization ────────────────────────

/// Deserializes either a single string or a list of strings into `Vec<String>`.
///
/// Accepts both `runtime = "name"` and `runtime = ["name1", "name2"]` in TOML.
#[derive(Debug, Clone)]
pub struct StringOrList(pub Vec<String>);

impl<'de> Deserialize<'de> for StringOrList {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct Visitor;

        impl<'de> de::Visitor<'de> for Visitor {
            type Value = StringOrList;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or list of strings")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<StringOrList, E> {
                Ok(StringOrList(vec![v.to_string()]))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<StringOrList, A::Error> {
                let mut v = Vec::new();
                while let Some(s) = seq.next_element::<String>()? {
                    v.push(s);
                }
                Ok(StringOrList(v))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

// ─── Raw TOML structures ────────────────────────────────

/// Raw deserialized baton.toml before validation.
#[derive(Debug, Deserialize)]
pub struct RawConfig {
    pub version: String,
    #[serde(default)]
    pub defaults: RawDefaults,
    #[serde(default)]
    pub runtimes: BTreeMap<String, RawRuntime>,
    #[serde(default)]
    pub sources: BTreeMap<String, RawSource>,
    #[serde(default)]
    pub validators: BTreeMap<String, RawValidatorDef>,
    pub gates: BTreeMap<String, RawGate>,
}

/// Raw source entry from `[sources.<name>]`.
#[derive(Debug, Deserialize, Clone)]
pub struct RawSource {
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub files: Option<Vec<String>>,
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
}

/// Raw top-level validator definition from `[validators.<name>]`.
#[derive(Debug, Deserialize, Clone)]
pub struct RawValidatorDef {
    #[serde(rename = "type")]
    pub validator_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,

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
    pub model: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub response_format: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub system_prompt: Option<String>,

    // Runtime (string or list of strings for fallback)
    #[serde(default)]
    pub runtime: Option<StringOrList>,
    #[serde(default)]
    pub sandbox: Option<bool>,
    #[serde(default)]
    pub max_iterations: Option<u32>,

    // Input declarations
    #[serde(default)]
    pub input: Option<toml::Value>,

    // Deprecated — must produce error if present
    #[serde(default)]
    pub context_refs: Option<Vec<String>>,
}

/// Raw gate reference: `{ ref = "name", blocking = true, ... }`.
#[derive(Debug, Deserialize, Clone)]
pub struct RawGateRef {
    #[serde(rename = "ref")]
    pub validator_ref: String,
    #[serde(default)]
    pub blocking: Option<bool>,
    #[serde(default)]
    pub run_if: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
}

/// A gate entry: either a reference to a top-level validator or an inline definition.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum RawGateEntry {
    Ref(RawGateRef),
    Inline(Box<RawValidator>),
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

/// Raw runtime entry from `[runtimes.<name>]`.
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
    pub validators: Vec<RawGateEntry>,
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

    // Runtime (string or list of strings for fallback)
    #[serde(default)]
    pub runtime: Option<StringOrList>,
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
    pub runtimes: BTreeMap<String, Runtime>,
    pub sources: BTreeMap<String, SourceConfig>,
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

/// Runtime configuration (API, OpenHands, OpenCode).
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

/// LLM interaction mode: one-shot query or multi-step session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmMode {
    Query,
    Session,
}

/// Expected response format from an LLM validator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseFormat {
    Verdict,
    Freeform,
}

/// Validated source configuration.
#[derive(Debug, Clone)]
pub struct SourceConfig {
    pub name: String,
    pub source_type: SourceType,
}

/// The type of a source: directory, single file, or file list.
#[derive(Debug, Clone)]
pub enum SourceType {
    Directory {
        root: String,
        include: Vec<String>,
        exclude: Vec<String>,
    },
    File {
        path: String,
    },
    FileList {
        files: Vec<String>,
    },
}

/// Input declaration on a validator.
#[derive(Debug, Clone, Default)]
pub enum InputDecl {
    /// No input — validator runs once with no files.
    #[default]
    None,
    /// Per-file — validator runs once per matching file.
    PerFile { pattern: String },
    /// Batch — all matching files at once.
    Batch { pattern: String },
    /// Named inputs — multiple named slots.
    Named(BTreeMap<String, InputSlot>),
}

/// A single named input slot in a multi-input validator.
#[derive(Debug, Clone)]
pub struct InputSlot {
    pub match_pattern: Option<String>,
    pub path: Option<String>,
    pub key: Option<String>,
    pub collect: bool,
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
    pub runtimes: Vec<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub context_refs: Vec<String>,
    pub temperature: f64,
    pub response_format: ResponseFormat,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,

    // Session
    pub sandbox: Option<bool>,
    pub max_iterations: Option<u32>,

    // Input declarations (v2)
    pub input: InputDecl,
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

    if raw.version != "0.4" && raw.version != "0.5" && raw.version != "0.6" {
        return Err(BatonError::ConfigError(format!(
            "Unsupported version '{}'. Expected '0.4', '0.5', or '0.6'.",
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

    let mut runtimes = BTreeMap::new();
    for (name, raw_r) in &raw.runtimes {
        let mut base_url = if raw_r.runtime_type == "api" {
            resolve_env_vars(&raw_r.base_url)
                .map_err(|e| BatonError::ConfigError(format!("Runtime '{name}': {e}")))?
        } else {
            raw_r.base_url.clone()
        };
        // Strip trailing slash
        if base_url.ends_with('/') {
            base_url.pop();
        }
        runtimes.insert(
            name.clone(),
            Runtime {
                runtime_type: raw_r.runtime_type.clone(),
                base_url,
                api_key_env: raw_r.api_key_env.clone(),
                default_model: raw_r.default_model.clone(),
                sandbox: raw_r.sandbox,
                timeout_seconds: raw_r.timeout_seconds,
                max_iterations: raw_r.max_iterations,
            },
        );
    }

    // ── Parse sources ──────────────────────────────────
    let mut sources = BTreeMap::new();
    for (name, raw_s) in &raw.sources {
        // Validate source name
        let valid_name = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !valid_name {
            return Err(BatonError::ConfigError(format!(
                "Source name '{name}' is invalid. Must match [a-zA-Z0-9_-]+ (no dots)."
            )));
        }

        // Mutual exclusion: only one of root, path, files
        let has_root = raw_s.root.is_some();
        let has_path = raw_s.path.is_some();
        let has_files = raw_s.files.is_some();
        let count = has_root as u8 + has_path as u8 + has_files as u8;
        if count > 1 {
            return Err(BatonError::ConfigError(format!(
                "Source '{name}': only one of 'root', 'path', or 'files' may be set."
            )));
        }
        if count == 0 {
            return Err(BatonError::ConfigError(format!(
                "Source '{name}': one of 'root', 'path', or 'files' must be set."
            )));
        }

        let source_type = if let Some(ref root) = raw_s.root {
            SourceType::Directory {
                root: root.clone(),
                include: raw_s.include.clone().unwrap_or_else(|| vec!["**/*".into()]),
                exclude: raw_s.exclude.clone().unwrap_or_default(),
            }
        } else if let Some(ref path) = raw_s.path {
            SourceType::File { path: path.clone() }
        } else if let Some(ref files) = raw_s.files {
            if files.is_empty() {
                return Err(BatonError::ConfigError(format!(
                    "Source '{name}': 'files' must not be empty."
                )));
            }
            SourceType::FileList {
                files: files.clone(),
            }
        } else {
            unreachable!()
        };

        sources.insert(
            name.clone(),
            SourceConfig {
                name: name.clone(),
                source_type,
            },
        );
    }

    // ── Parse top-level validators ────────────────────
    let mut top_validators: BTreeMap<String, ValidatorConfig> = BTreeMap::new();
    for (name, raw_v) in &raw.validators {
        let valid_name = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !valid_name {
            return Err(BatonError::ConfigError(format!(
                "Validator name '{name}' contains invalid characters. Must match [A-Za-z0-9_-]+."
            )));
        }

        // Reject context_refs
        if let Some(ref crefs) = raw_v.context_refs {
            if !crefs.is_empty() {
                return Err(BatonError::ConfigError(format!(
                    "Validator '{name}': 'context_refs' is no longer supported. Use 'input' declarations instead."
                )));
            }
        }

        let vc = parse_validator_def(name, raw_v, &defaults)?;
        top_validators.insert(name.clone(), vc);
    }

    // ── Parse gates ───────────────────────────────────
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

        for entry in &raw_gate.validators {
            match entry {
                RawGateEntry::Ref(gate_ref) => {
                    // Look up in top-level validators
                    let base = top_validators.get(&gate_ref.validator_ref).ok_or_else(|| {
                        BatonError::ConfigError(format!(
                            "Gate '{gate_name}': ref '{}' not found in [validators].",
                            gate_ref.validator_ref
                        ))
                    })?;
                    let mut vc = base.clone();
                    // Apply gate-level overrides
                    if let Some(blocking) = gate_ref.blocking {
                        vc.blocking = blocking;
                    }
                    if gate_ref.run_if.is_some() {
                        vc.run_if = gate_ref.run_if.clone();
                    }
                    if let Some(timeout) = gate_ref.timeout_seconds {
                        vc.timeout_seconds = timeout;
                    }
                    validators.push(vc);
                }
                RawGateEntry::Inline(raw_v) => {
                    // Old inline format
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

                    let vc = parse_inline_validator(raw_v, &defaults)?;
                    validators.push(vc);
                }
            }
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
        runtimes,
        sources,
        gates,
        config_dir: config_dir.to_path_buf(),
    })
}

/// Parse a top-level validator definition into a ValidatorConfig.
fn parse_validator_def(
    name: &str,
    raw_v: &RawValidatorDef,
    defaults: &Defaults,
) -> Result<ValidatorConfig> {
    let vtype = match raw_v.validator_type.as_str() {
        "script" => ValidatorType::Script,
        "llm" => ValidatorType::Llm,
        "human" => ValidatorType::Human,
        other => {
            return Err(BatonError::ConfigError(format!(
                "Validator '{name}': unknown type '{other}'. Expected 'script', 'llm', or 'human'."
            )));
        }
    };

    match &vtype {
        ValidatorType::Script => {
            if raw_v.command.is_none() {
                return Err(BatonError::ConfigError(format!(
                    "Validator '{name}': missing required field 'command'."
                )));
            }
        }
        ValidatorType::Llm | ValidatorType::Human => {
            if raw_v.prompt.is_none() {
                return Err(BatonError::ConfigError(format!(
                    "Validator '{name}': missing required field 'prompt'."
                )));
            }
        }
    }

    let mode = match raw_v.mode.as_deref() {
        Some("session") => LlmMode::Session,
        Some("query") | Some("completion") | None => LlmMode::Query,
        Some(other) => {
            return Err(BatonError::ConfigError(format!(
                "Validator '{name}': invalid mode '{other}'. Expected 'query', 'completion', or 'session'."
            )));
        }
    };

    let response_format = match raw_v.response_format.as_deref() {
        Some("freeform") => ResponseFormat::Freeform,
        Some("verdict") | None => ResponseFormat::Verdict,
        Some(other) => {
            return Err(BatonError::ConfigError(format!(
                "Validator '{name}': invalid response_format '{other}'."
            )));
        }
    };

    if raw_v.warn_exit_codes.contains(&0) {
        return Err(BatonError::ConfigError(format!(
            "Validator '{name}': warn_exit_codes must not contain 0 (exit code 0 is always pass)."
        )));
    }

    let runtimes = raw_v
        .runtime
        .as_ref()
        .map(|sol| sol.0.clone())
        .unwrap_or_default();

    let input = parse_input_decl(name, &raw_v.input)?;

    Ok(ValidatorConfig {
        name: name.to_string(),
        validator_type: vtype,
        blocking: defaults.blocking,
        run_if: None,
        timeout_seconds: raw_v.timeout_seconds.unwrap_or(defaults.timeout_seconds),
        tags: raw_v.tags.clone(),
        command: raw_v.command.clone(),
        warn_exit_codes: raw_v.warn_exit_codes.clone(),
        working_dir: raw_v.working_dir.clone(),
        env: raw_v.env.clone(),
        mode,
        runtimes,
        model: raw_v.model.clone(),
        prompt: raw_v.prompt.clone(),
        context_refs: vec![],
        temperature: raw_v.temperature.unwrap_or(0.0),
        response_format,
        max_tokens: raw_v.max_tokens,
        system_prompt: raw_v.system_prompt.clone(),
        sandbox: raw_v.sandbox,
        max_iterations: raw_v.max_iterations,
        input,
    })
}

/// Parse an inline validator (old format) into a ValidatorConfig.
fn parse_inline_validator(raw_v: &RawValidator, defaults: &Defaults) -> Result<ValidatorConfig> {
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

    match &vtype {
        ValidatorType::Script => {
            if raw_v.command.is_none() {
                return Err(BatonError::ConfigError(format!(
                    "Validator '{}': missing required field 'command'.",
                    raw_v.name
                )));
            }
        }
        ValidatorType::Llm | ValidatorType::Human => {
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
        Some("query") | Some("completion") | None => LlmMode::Query,
        Some(other) => {
            return Err(BatonError::ConfigError(format!(
                "Validator '{}': invalid mode '{other}'. Expected 'query', 'completion', or 'session'.",
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

    if raw_v.warn_exit_codes.contains(&0) {
        return Err(BatonError::ConfigError(format!(
            "Validator '{}': warn_exit_codes must not contain 0 (exit code 0 is always pass).",
            raw_v.name
        )));
    }

    let runtimes = raw_v
        .runtime
        .as_ref()
        .map(|sol| sol.0.clone())
        .unwrap_or_default();

    Ok(ValidatorConfig {
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
        runtimes,
        model: raw_v.model.clone(),
        prompt: raw_v.prompt.clone(),
        context_refs: raw_v.context_refs.clone(),
        temperature: raw_v.temperature.unwrap_or(0.0),
        response_format,
        max_tokens: raw_v.max_tokens,
        system_prompt: raw_v.system_prompt.clone(),
        sandbox: raw_v.sandbox,
        max_iterations: raw_v.max_iterations,
        input: InputDecl::None,
    })
}

/// Parse input declarations from a TOML value.
fn parse_input_decl(name: &str, input: &Option<toml::Value>) -> Result<InputDecl> {
    let input = match input {
        None => return Ok(InputDecl::None),
        Some(v) => v,
    };

    match input {
        toml::Value::String(pattern) => Ok(InputDecl::PerFile {
            pattern: pattern.clone(),
        }),
        toml::Value::Table(table) => {
            // If it has "match", it's a batch or single unnamed input
            if table.contains_key("match") {
                let pattern = table
                    .get("match")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        BatonError::ConfigError(format!(
                            "Validator '{name}': input.match must be a string."
                        ))
                    })?
                    .to_string();
                let collect = table
                    .get("collect")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if collect {
                    Ok(InputDecl::Batch { pattern })
                } else {
                    Ok(InputDecl::PerFile { pattern })
                }
            } else {
                // Named inputs: each key is an input slot
                let mut slots = BTreeMap::new();
                for (slot_name, slot_val) in table {
                    let slot_table = slot_val.as_table().ok_or_else(|| {
                        BatonError::ConfigError(format!(
                            "Validator '{name}': input.{slot_name} must be a table."
                        ))
                    })?;
                    let match_pattern = slot_table
                        .get("match")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let path = slot_table
                        .get("path")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let key = slot_table
                        .get("key")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let collect = slot_table
                        .get("collect")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    // Validate key expression if present
                    if let Some(ref key_expr) = key {
                        validate_key_expression(name, slot_name, key_expr)?;
                    }

                    slots.insert(
                        slot_name.clone(),
                        InputSlot {
                            match_pattern,
                            path,
                            key,
                            collect,
                        },
                    );
                }
                Ok(InputDecl::Named(slots))
            }
        }
        _ => Err(BatonError::ConfigError(format!(
            "Validator '{name}': 'input' must be a string or table."
        ))),
    }
}

/// Validate a key expression.
fn validate_key_expression(validator_name: &str, slot_name: &str, expr: &str) -> Result<()> {
    let valid = expr == "{stem}"
        || expr == "{name}"
        || expr == "{parent}"
        || expr.starts_with("{relative:") && expr.ends_with('}')
        || expr.starts_with("{regex:") && expr.ends_with('}');
    if !valid {
        return Err(BatonError::ConfigError(format!(
            "Validator '{validator_name}': input.{slot_name} has invalid key expression '{expr}'. \
             Expected one of: {{stem}}, {{name}}, {{parent}}, {{relative:prefix/}}, {{regex:pattern}}."
        )));
    }
    Ok(())
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

            // Check runtime references (LLM validators)
            if val.validator_type == ValidatorType::Llm {
                // LLM validators must have at least one runtime
                if val.runtimes.is_empty() {
                    v.errors.push(format!(
                        "Validator '{}': LLM validators require a 'runtime' field.",
                        val.name
                    ));
                }

                // Check each runtime reference exists
                for rt in &val.runtimes {
                    if !config.runtimes.contains_key(rt) {
                        v.errors.push(format!(
                            "Validator '{}': runtime '{rt}' is not defined in [runtimes].",
                            val.name
                        ));
                    }
                }

                // Session mode: check for api-only runtimes
                if val.mode == LlmMode::Session && !val.runtimes.is_empty() {
                    let api_runtimes: Vec<&str> = val
                        .runtimes
                        .iter()
                        .filter(|rt| {
                            config
                                .runtimes
                                .get(*rt)
                                .map(|r| r.runtime_type == "api")
                                .unwrap_or(false)
                        })
                        .map(|s| s.as_str())
                        .collect();

                    if api_runtimes.len() == val.runtimes.len() && !val.runtimes.is_empty() {
                        v.errors.push(format!(
                            "Validator '{}': mode 'session' but all listed runtimes are type 'api' (no session-capable runtimes).",
                            val.name
                        ));
                    } else {
                        for rt in &api_runtimes {
                            v.warnings.push(format!(
                                "Validator '{}': api runtime '{rt}' will be skipped for session mode.",
                                val.name
                            ));
                        }
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

    // Check sources
    for (name, source) in &config.sources {
        if let SourceType::Directory { ref root, .. } = source.source_type {
            let root_path = config.config_dir.join(root);
            if !root_path.exists() {
                v.warnings.push(format!(
                    "Source '{name}': root directory '{}' does not exist.",
                    root_path.display()
                ));
            }
        }
    }

    // Check fixed input paths
    for gate in config.gates.values() {
        for val in &gate.validators {
            if let InputDecl::Named(ref slots) = val.input {
                for (slot_name, slot) in slots {
                    if let Some(ref path) = slot.path {
                        let resolved = config.config_dir.join(path);
                        if !resolved.exists() && !std::path::Path::new(path).exists() {
                            v.errors.push(format!(
                                "Validator '{}': input.{slot_name} path '{}' does not exist.",
                                val.name, path
                            ));
                        }
                    }
                }
            }
        }
    }

    // Check runtime API key env vars (for api-type runtimes)
    for (name, runtime) in &config.runtimes {
        if let Some(ref env_var) = runtime.api_key_env {
            if !env_var.is_empty() && std::env::var(env_var).is_err() {
                v.errors
                    .push(format!("Runtime '{name}': env var '{env_var}' is not set.",));
            }
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

    // ─── Runtime parsing ─────────────────────────────

    #[test]
    fn api_runtime_trailing_slash_stripped() {
        let toml = r#"
version = "0.6"
[runtimes.default]
type = "api"
base_url = "https://api.example.com/"
default_model = "test-model"

[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(
            config.runtimes["default"].base_url,
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
    fn validate_session_mode_api_runtime_warns() {
        let toml = r#"
version = "0.6"
[runtimes.api-rt]
type = "api"
base_url = "http://localhost"

[runtimes.agent-rt]
type = "openhands"
base_url = "http://localhost:3000"

[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
mode = "session"
prompt = "Review this"
runtime = ["api-rt", "agent-rt"]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            validation
                .warnings
                .iter()
                .any(|w| w.contains("api-rt") && w.contains("skipped")),
            "Warnings: {:?}",
            validation.warnings
        );
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
version = "0.6"

[defaults]
timeout_seconds = 300
blocking = true
prompts_dir = "./prompts"

[runtimes.default]
type = "api"
base_url = "https://api.example.com"
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
mode = "query"
runtime = "default"
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
    fn llm_validator_no_runtime_has_empty_runtimes() {
        let toml = r#"
version = "0.6"
[gates.gate]
[[gates.gate.validators]]
name = "check"
type = "llm"
prompt = "Review"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert!(config.gates["gate"].validators[0].runtimes.is_empty());
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
    fn undefined_non_default_runtime() {
        let toml = r#"
version = "0.6"
[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
prompt = "Review"
runtime = "nonexistent"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(validation.has_errors());
        assert!(
            validation.errors.iter().any(|e| e.contains("nonexistent")),
            "Errors: {:?}",
            validation.errors
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
version = "0.6"
[runtimes.myruntime]
type = "api"
base_url = "https://api.example.com"
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
        assert!(err.contains("myruntime"), "Error: {err}");
        assert!(
            err.contains("BATON_TEST_NONEXISTENT_VAR_XYZ"),
            "Error: {err}"
        );
    }

    #[test]
    fn multiple_simultaneous_validation_errors() {
        let toml = r#"
version = "0.6"
[gates.test]
[[gates.test.validators]]
name = "a"
type = "llm"
prompt = "Review"
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

    // ═══════════════════════════════════════════════════════════════
    // v2 migration: Source parsing (SPEC-CF-SC-*)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn source_directory_with_root() {
        // SPEC-CF-SC-001: directory source requires root, include defaults to ["**/*"]
        let toml = r#"
version = "0.4"
[sources.code]
root = "src"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn source_file_with_path() {
        // SPEC-CF-SC-002: file source requires path
        let toml = r#"
version = "0.4"
[sources.readme]
path = "README.md"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn source_file_list() {
        // SPEC-CF-SC-003: file list source requires non-empty files array
        let toml = r#"
version = "0.4"
[sources.docs]
files = ["README.md", "CHANGELOG.md"]

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn source_empty_files_list_rejected() {
        // SPEC-CF-SC-003: empty files list is an error
        let toml = r#"
version = "0.4"
[sources.docs]
files = []

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
    }

    #[test]
    fn source_type_mutual_exclusion() {
        // SPEC-CF-SC-004: only one of root, path, or files may be set
        let toml = r#"
version = "0.4"
[sources.mixed]
root = "src"
path = "README.md"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
    }

    #[test]
    fn source_name_pattern_rejects_dots() {
        // SPEC-CF-SC-005: source names must match [a-zA-Z0-9_-]+, no dots
        let toml = r#"
version = "0.4"
[sources."my.source"]
root = "src"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
    }

    #[test]
    fn source_missing_root_warns() {
        // SPEC-CF-SC-006: nonexistent root directory emits validation warning
        let toml = r#"
version = "0.4"
[sources.code]
root = "/nonexistent/path/that/does/not/exist"

[gates.review]
[[gates.review.validators]]
name = "lint"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            !validation.warnings.is_empty(),
            "Expected a warning about nonexistent root"
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 migration: Top-level validator parsing (SPEC-CF-VP-*)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn validator_name_from_toml_key() {
        // SPEC-CF-VP-001: validator name comes from the TOML key under [validators]
        let toml = r#"
version = "0.4"
[validators.my-lint]
type = "script"
command = "echo ok"

[gates.review]
validators = [{ ref = "my-lint" }]
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn validator_name_rejects_invalid_chars() {
        // SPEC-CF-VP-001: validator name must match [A-Za-z0-9_-]+
        let toml = r#"
version = "0.4"
[validators."bad name!"]
type = "script"
command = "echo ok"

[gates.review]
validators = [{ ref = "bad name!" }]
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
    }

    #[test]
    fn context_refs_field_rejected() {
        // SPEC-CF-VP-002: context_refs field produces an error
        let toml = r#"
version = "0.4"
[validators.check]
type = "script"
command = "echo ok"
context_refs = ["spec"]

[gates.review]
validators = [{ ref = "check" }]
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("input"),
            "Expected error about input declarations, got: {err}"
        );
    }

    #[test]
    fn input_form_no_input() {
        // SPEC-CF-VP-003: absent input means validator runs once with no files
        let toml = r#"
version = "0.4"
[validators.check]
type = "script"
command = "echo ok"

[gates.review]
validators = [{ ref = "check" }]
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn input_form_per_file_glob() {
        // SPEC-CF-VP-004: input as string is a glob pattern
        let toml = r#"
version = "0.4"
[validators.lint]
type = "script"
command = "ruff check {file}"
input = "*.py"

[gates.review]
validators = [{ ref = "lint" }]
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn input_form_batch() {
        // SPEC-CF-VP-005: input with match + collect = true
        let toml = r#"
version = "0.4"
[validators.lint]
type = "script"
command = "echo ok"
input = { match = "*.py", collect = true }

[gates.review]
validators = [{ ref = "lint" }]
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn input_form_named() {
        // SPEC-CF-VP-006: input with named sub-keys
        let toml = r#"
version = "0.4"
[validators.check]
type = "llm"
prompt = "check code"
model = "test"
[validators.check.input]
code = { match = "*.py" }
spec = { path = "spec.md" }

[gates.review]
validators = [{ ref = "check" }]
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
    }

    #[test]
    fn fixed_input_path_must_exist() {
        // SPEC-CF-VP-007: named input with path must reference existing file
        let toml = r#"
version = "0.4"
[validators.check]
type = "script"
command = "echo ok"
[validators.check.input]
spec = { path = "/nonexistent/spec.md" }

[gates.review]
validators = [{ ref = "check" }]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            validation.has_errors(),
            "Expected error about nonexistent fixed input path"
        );
    }

    #[test]
    fn key_expression_must_be_valid() {
        // SPEC-CF-VP-008: unknown key expressions are config errors
        let toml = r#"
version = "0.4"
[validators.check]
type = "script"
command = "echo ok"
[validators.check.input]
code = { match = "*.py", key = "{unknown_expr}" }

[gates.review]
validators = [{ ref = "check" }]
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
    }

    #[test]
    fn key_expression_valid_forms() {
        // SPEC-CF-VP-008: valid key expressions
        for expr in &[
            "{stem}",
            "{name}",
            "{parent}",
            "{relative:src/}",
            "{regex:[a-z]+}",
        ] {
            let toml = format!(
                "version = \"0.4\"\n\
                 [validators.check]\n\
                 type = \"script\"\n\
                 command = \"echo ok\"\n\
                 [validators.check.input]\n\
                 code = {{ match = \"*.py\", key = \"{}\" }}\n\
                 \n\
                 [gates.review]\n\
                 validators = [{{ ref = \"check\" }}]\n",
                expr
            );
            let result = parse_config(&toml, &config_dir());
            assert!(
                result.is_ok(),
                "Key expression {} should be valid, got: {:?}",
                expr,
                result.err()
            );
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 migration: Gate reference parsing (SPEC-CF-GR-*)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn gate_ref_must_name_existing_validator() {
        // SPEC-CF-GR-001: ref must match a key in [validators]
        let toml = r#"
version = "0.4"
[validators.lint]
type = "script"
command = "echo ok"

[gates.review]
validators = [{ ref = "nonexistent" }]
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("nonexistent"),
            "Error should name the missing validator: {err}"
        );
    }

    #[test]
    fn gate_ref_blocking_defaults_from_defaults() {
        // SPEC-CF-GR-002: blocking inherits from [defaults].blocking when not set
        let toml = r#"
version = "0.4"
[defaults]
blocking = false

[validators.lint]
type = "script"
command = "echo ok"

[gates.review]
validators = [{ ref = "lint" }]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let gate = &config.gates["review"];
        // The lint validator within this gate should inherit blocking=false
        assert!(!gate.validators[0].blocking);
    }

    #[test]
    fn gate_ref_blocking_overrides_defaults() {
        // SPEC-CF-GR-002: explicit blocking on gate ref takes precedence
        let toml = r#"
version = "0.4"
[defaults]
blocking = false

[validators.lint]
type = "script"
command = "echo ok"

[gates.review]
validators = [{ ref = "lint", blocking = true }]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let gate = &config.gates["review"];
        assert!(gate.validators[0].blocking);
    }

    #[test]
    fn gate_ref_run_if_validated() {
        // SPEC-CF-GR-003: run_if must reference earlier validators in same gate
        let toml = r#"
version = "0.4"
[validators.lint]
type = "script"
command = "echo ok"
[validators.format]
type = "script"
command = "echo ok"

[gates.review]
validators = [
    { ref = "lint" },
    { ref = "format", run_if = "lint.status == pass" },
]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            !validation.has_errors(),
            "Valid run_if should not produce errors: {:?}",
            validation.errors
        );
    }

    #[test]
    fn gate_ref_run_if_forward_reference_rejected() {
        // SPEC-CF-GR-003: run_if referencing a later validator is an error
        let toml = r#"
version = "0.4"
[validators.lint]
type = "script"
command = "echo ok"
[validators.format]
type = "script"
command = "echo ok"

[gates.review]
validators = [
    { ref = "lint", run_if = "format.status == pass" },
    { ref = "format" },
]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            validation.has_errors(),
            "Forward reference in run_if should produce an error"
        );
    }

    #[test]
    fn validator_reuse_across_gates() {
        // SPEC-CF-GR-004: same validator in multiple gates with different settings
        let toml = r#"
version = "0.4"
[validators.lint]
type = "script"
command = "echo ok"

[gates.fast]
validators = [{ ref = "lint", blocking = false }]

[gates.strict]
validators = [{ ref = "lint", blocking = true }]
"#;
        let config = parse_config(toml, &config_dir());
        assert!(config.is_ok(), "Error: {:?}", config.err());
        let config = config.unwrap();
        assert!(!config.gates["fast"].validators[0].blocking);
        assert!(config.gates["strict"].validators[0].blocking);
    }

    #[test]
    fn gate_empty_validators_rejected() {
        // SPEC-CF-PC-030: gate validators array must have at least one ref
        let toml = r#"
version = "0.4"
[validators.lint]
type = "script"
command = "echo ok"

[gates.review]
validators = []
"#;
        let result = parse_config(toml, &config_dir());
        assert!(result.is_err());
    }

    // ─── Session / API Runtime Validation ────────────

    #[test]
    fn validate_session_mode_all_api_runtimes_errors() {
        // SPEC-CF-VC-025: session mode with ALL api runtimes → error
        let toml = r#"
version = "0.6"
[runtimes.rt1]
type = "api"
base_url = "http://localhost:8001"

[runtimes.rt2]
type = "api"
base_url = "http://localhost:8002"

[gates.test]
[[gates.test.validators]]
name = "check"
type = "llm"
mode = "session"
prompt = "Review this"
runtime = ["rt1", "rt2"]
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        let validation = validate_config(&config);
        assert!(
            validation.has_errors(),
            "Expected error when all runtimes are api type for session mode. Errors: {:?}, Warnings: {:?}",
            validation.errors,
            validation.warnings
        );
    }

    #[test]
    fn api_runtime_double_trailing_slash_only_one_stripped() {
        // SPEC-CF-PC-028: only a single trailing slash is stripped
        let toml = r#"
version = "0.6"
[runtimes.default]
type = "api"
base_url = "https://api.example.com//"
default_model = "test-model"

[gates.test]
[[gates.test.validators]]
name = "check"
type = "script"
command = "echo ok"
"#;
        let config = parse_config(toml, &config_dir()).unwrap();
        assert_eq!(
            config.runtimes["default"].base_url,
            "https://api.example.com/"
        );
    }
}
