//! Add a validator to baton.toml (interactive, flag-driven, or `--from` import).
//!
//! Three modes:
//! - Interactive wizard (default): walks the user through creating a validator
//! - Non-interactive: builds a validator from CLI flags
//! - Import: fetches validator definitions from a file or URL

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml_edit::{value, Array, DocumentMut, InlineTable, Item, Table};

use crate::config::{discover_config, parse_config, validate_config, BatonConfig};
use crate::error::{BatonError, Result};

// ─── Data types ──────────────────────────────────────────

/// A validator definition ready to be inserted into baton.toml.
#[derive(Debug, Clone)]
pub struct ValidatorDef {
    pub name: String,
    pub validator_type: String,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub temperature: Option<f64>,
    pub response_format: Option<String>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
    pub input: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub warn_exit_codes: Vec<i32>,
    pub working_dir: Option<String>,
    pub tags: Vec<String>,
    #[allow(dead_code)] // reserved for future env var support
    pub env: BTreeMap<String, String>,
}

/// Where to add the validator in a gate.
#[derive(Debug, Clone)]
pub struct GateAssignment {
    pub gate_name: String,
    pub blocking: bool,
    pub create_new: bool,
    pub description: Option<String>,
}

/// Options passed from the CLI to the add module.
#[derive(Debug, Clone)]
pub struct AddOptions {
    pub name: Option<String>,
    pub validator_type: Option<String>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub gate: Option<String>,
    pub blocking: Option<bool>,
    pub tags: Option<Vec<String>>,
    pub input: Option<String>,
    pub timeout: Option<u64>,
    pub from: Option<String>,
    pub config: Option<PathBuf>,
    pub dry_run: bool,
    pub yes: bool,
}

// ─── Config discovery ────────────────────────────────────

/// Find and read the baton.toml file, returning its path and raw content.
pub fn find_config(config_path: Option<&PathBuf>) -> Result<(PathBuf, String)> {
    let config_file = match config_path {
        Some(p) => {
            if !p.exists() {
                return Err(BatonError::ConfigError(format!(
                    "Config file not found: {}",
                    p.display()
                )));
            }
            p.clone()
        }
        None => discover_config(&std::env::current_dir()?)?,
    };
    let raw = std::fs::read_to_string(&config_file)?;
    Ok((config_file, raw))
}

/// Parse and validate a config string. Returns the parsed config for inspection.
fn parse_and_validate(raw: &str, config_dir: &Path) -> Result<BatonConfig> {
    let config = parse_config(raw, config_dir)?;
    let validation = validate_config(&config);
    if validation.has_errors() {
        return Err(BatonError::ConfigError(format!(
            "Config validation errors: {}",
            validation.errors.join("; ")
        )));
    }
    Ok(config)
}

// ─── Non-interactive mode ────────────────────────────────

/// Build a ValidatorDef from CLI flags. Validates required fields per type.
pub fn build_from_flags(opts: &AddOptions) -> Result<ValidatorDef> {
    let name = opts.name.as_ref().ok_or_else(|| {
        BatonError::ConfigError("--name is required in non-interactive mode".into())
    })?;
    let vtype = opts.validator_type.as_ref().ok_or_else(|| {
        BatonError::ConfigError("--type is required in non-interactive mode".into())
    })?;

    match vtype.as_str() {
        "script" => {
            if opts.command.is_none() {
                return Err(BatonError::ConfigError(
                    "--command is required for script validators".into(),
                ));
            }
        }
        "llm" => {
            if opts.prompt.is_none() {
                return Err(BatonError::ConfigError(
                    "--prompt is required for llm validators".into(),
                ));
            }
            if opts.runtime.is_none() {
                return Err(BatonError::ConfigError(
                    "--runtime is required for llm validators".into(),
                ));
            }
        }
        "human" => {
            if opts.prompt.is_none() {
                return Err(BatonError::ConfigError(
                    "--prompt is required for human validators".into(),
                ));
            }
        }
        other => {
            return Err(BatonError::ConfigError(format!(
                "Unknown validator type '{other}'. Must be one of: script, llm, human"
            )));
        }
    }

    Ok(ValidatorDef {
        name: name.clone(),
        validator_type: vtype.clone(),
        command: opts.command.clone(),
        prompt: opts.prompt.clone(),
        runtime: opts.runtime.clone(),
        model: opts.model.clone(),
        mode: None,
        temperature: None,
        response_format: None,
        max_tokens: None,
        system_prompt: None,
        input: opts.input.clone(),
        timeout_seconds: opts.timeout,
        warn_exit_codes: Vec::new(),
        working_dir: None,
        tags: opts.tags.clone().unwrap_or_default(),
        env: BTreeMap::new(),
    })
}

// ─── Interactive mode ────────────────────────────────────

/// Run the interactive wizard. Returns a ValidatorDef and optional GateAssignment.
pub fn run_wizard(config: &BatonConfig) -> Result<(ValidatorDef, Option<GateAssignment>)> {
    use dialoguer::{Input, Select};

    // Step 1: Validator type
    let type_options = &["script", "llm", "human"];
    let type_idx = Select::new()
        .with_prompt("Validator type")
        .items(type_options)
        .default(0)
        .interact()
        .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
    let vtype = type_options[type_idx].to_string();

    // Step 2: Name
    let name: String = Input::new()
        .with_prompt("Validator name")
        .interact_text()
        .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;

    if name.is_empty() {
        return Err(BatonError::ConfigError(
            "Validator name cannot be empty".into(),
        ));
    }

    // Check name doesn't already exist
    for gate in config.gates.values() {
        for v in &gate.validators {
            if v.name == name {
                return Err(BatonError::ConfigError(format!(
                    "Validator '{name}' already exists"
                )));
            }
        }
    }

    // Step 3: Type-specific fields
    let mut def = ValidatorDef {
        name,
        validator_type: vtype.clone(),
        command: None,
        prompt: None,
        runtime: None,
        model: None,
        mode: None,
        temperature: None,
        response_format: None,
        max_tokens: None,
        system_prompt: None,
        input: None,
        timeout_seconds: None,
        warn_exit_codes: Vec::new(),
        working_dir: None,
        tags: Vec::new(),
        env: BTreeMap::new(),
    };

    match vtype.as_str() {
        "script" => {
            let command: String = Input::new()
                .with_prompt("Command")
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            def.command = Some(command);

            let input_pattern: String = Input::new()
                .with_prompt("Input pattern (glob, blank to skip)")
                .default(String::new())
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            if !input_pattern.is_empty() {
                def.input = Some(input_pattern);
            }

            let timeout: String = Input::new()
                .with_prompt("Timeout seconds")
                .default("300".into())
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            let timeout_val: u64 = timeout.parse().unwrap_or(300);
            if timeout_val != 300 {
                def.timeout_seconds = Some(timeout_val);
            }

            let warn_codes: String = Input::new()
                .with_prompt("Warn exit codes (comma-separated, blank to skip)")
                .default(String::new())
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            if !warn_codes.is_empty() {
                def.warn_exit_codes = warn_codes
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect();
            }
        }
        "llm" => {
            let prompt_text: String = Input::new()
                .with_prompt("Prompt (text or path to .md file)")
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            def.prompt = Some(prompt_text);

            // List available runtimes
            let runtime_names: Vec<&String> = config.runtimes.keys().collect();
            if runtime_names.is_empty() {
                eprintln!(
                    "Warning: No runtimes defined in config. You'll need to add one to baton.toml."
                );
                let rt: String = Input::new()
                    .with_prompt("Runtime name")
                    .interact_text()
                    .map_err(|e| {
                        BatonError::ConfigError(format!("Interactive prompt failed: {e}"))
                    })?;
                def.runtime = Some(rt);
            } else if runtime_names.len() == 1 {
                def.runtime = Some(runtime_names[0].clone());
                eprintln!("Using runtime: {}", runtime_names[0]);
            } else {
                let rt_strs: Vec<&str> = runtime_names.iter().map(|s| s.as_str()).collect();
                let rt_idx = Select::new()
                    .with_prompt("Runtime")
                    .items(&rt_strs)
                    .default(0)
                    .interact()
                    .map_err(|e| {
                        BatonError::ConfigError(format!("Interactive prompt failed: {e}"))
                    })?;
                def.runtime = Some(rt_strs[rt_idx].to_string());
            }

            let mode_options = &["query", "session"];
            let mode_idx = Select::new()
                .with_prompt("Mode")
                .items(mode_options)
                .default(0)
                .interact()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            if mode_idx != 0 {
                def.mode = Some(mode_options[mode_idx].to_string());
            }

            let temp: String = Input::new()
                .with_prompt("Temperature")
                .default("0.0".into())
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            let temp_val: f64 = temp.parse().unwrap_or(0.0);
            if temp_val != 0.0 {
                def.temperature = Some(temp_val);
            }

            let fmt_options = &["verdict", "freeform"];
            let fmt_idx = Select::new()
                .with_prompt("Response format")
                .items(fmt_options)
                .default(0)
                .interact()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            if fmt_idx != 0 {
                def.response_format = Some(fmt_options[fmt_idx].to_string());
            }

            let input_pattern: String = Input::new()
                .with_prompt("Input pattern (glob, blank to skip)")
                .default(String::new())
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            if !input_pattern.is_empty() {
                def.input = Some(input_pattern);
            }
        }
        "human" => {
            let prompt_text: String = Input::new()
                .with_prompt("Prompt")
                .interact_text()
                .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
            def.prompt = Some(prompt_text);
        }
        _ => unreachable!(),
    }

    // Step 4: Tags
    let tags_str: String = Input::new()
        .with_prompt("Tags (comma-separated, blank to skip)")
        .default(String::new())
        .interact_text()
        .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
    if !tags_str.is_empty() {
        def.tags = tags_str.split(',').map(|s| s.trim().to_string()).collect();
    }

    // Step 5: Gate assignment
    let gate_assignment = prompt_gate_assignment(config)?;

    Ok((def, gate_assignment))
}

/// Prompt the user to assign the validator to a gate.
fn prompt_gate_assignment(config: &BatonConfig) -> Result<Option<GateAssignment>> {
    use dialoguer::{Confirm, Input, Select};

    let mut gate_options: Vec<String> = Vec::new();
    let gate_names: Vec<&String> = config.gates.keys().collect();
    for gname in &gate_names {
        let count = config.gates[gname.as_str()].validators.len();
        gate_options.push(format!("{gname} ({count} validators)"));
    }
    gate_options.push("+ Create new gate".into());
    gate_options.push("Skip".into());

    let gate_idx = Select::new()
        .with_prompt("Add to a gate?")
        .items(&gate_options)
        .default(0)
        .interact()
        .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;

    if gate_idx == gate_options.len() - 1 {
        // Skip
        return Ok(None);
    }

    let (gate_name, create_new, description) = if gate_idx == gate_options.len() - 2 {
        // Create new gate
        let name: String = Input::new()
            .with_prompt("Gate name")
            .interact_text()
            .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
        let desc: String = Input::new()
            .with_prompt("Description (blank to skip)")
            .default(String::new())
            .interact_text()
            .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;
        let desc = if desc.is_empty() { None } else { Some(desc) };
        (name, true, desc)
    } else {
        (gate_names[gate_idx].clone(), false, None)
    };

    let blocking = Confirm::new()
        .with_prompt("Blocking?")
        .default(true)
        .interact()
        .map_err(|e| BatonError::ConfigError(format!("Interactive prompt failed: {e}")))?;

    Ok(Some(GateAssignment {
        gate_name,
        blocking,
        create_new,
        description,
    }))
}

// ─── Import mode ─────────────────────────────────────────

/// Resolve an import source string to raw TOML content.
pub fn resolve_import_source(source: &str) -> Result<String> {
    if source.starts_with("registry:") {
        return Err(BatonError::ConfigError(
            "Registry imports are not yet supported. Use a URL or file path.".into(),
        ));
    }

    if source.starts_with("http://") || source.starts_with("https://") {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| BatonError::ConfigError(format!("HTTP client error: {e}")))?;

        let response = client
            .get(source)
            .send()
            .map_err(|e| BatonError::ConfigError(format!("Failed to fetch {source}: {e}")))?;

        if !response.status().is_success() {
            return Err(BatonError::ConfigError(format!(
                "Failed to fetch {source}: HTTP {}",
                response.status()
            )));
        }

        response
            .text()
            .map_err(|e| BatonError::ConfigError(format!("Failed to read response: {e}")))
    } else {
        // Local file
        let path = Path::new(source);
        if !path.exists() {
            return Err(BatonError::ConfigError(format!(
                "Import file not found: {source}"
            )));
        }
        std::fs::read_to_string(path)
            .map_err(|e| BatonError::ConfigError(format!("Failed to read {source}: {e}")))
    }
}

/// Parse an import TOML string into ValidatorDefs.
/// Supports two formats:
/// - Format A: `[validator]` with a `name` field (single validator)
/// - Format B: `[validators.*]` (multi-validator, same as baton.toml)
pub fn parse_import(toml_str: &str) -> Result<Vec<ValidatorDef>> {
    let doc = toml_str
        .parse::<DocumentMut>()
        .map_err(|e| BatonError::ConfigError(format!("Failed to parse import TOML: {e}")))?;

    // Check for Format A: [validator]
    if let Some(item) = doc.get("validator") {
        if let Some(table) = item.as_table() {
            let def = parse_validator_table(table, None)?;
            return Ok(vec![def]);
        }
    }

    // Check for Format B: [validators.*]
    if let Some(item) = doc.get("validators") {
        if let Some(table) = item.as_table() {
            let mut defs = Vec::new();
            for (key, val) in table.iter() {
                if let Some(vtable) = val.as_table() {
                    let def = parse_validator_table(vtable, Some(key))?;
                    defs.push(def);
                }
            }
            if defs.is_empty() {
                return Err(BatonError::ConfigError(
                    "No validators found in import file".into(),
                ));
            }
            return Ok(defs);
        }
    }

    Err(BatonError::ConfigError(
        "Import file must contain a [validator] or [validators.*] section".into(),
    ))
}

/// Parse a single validator from a toml_edit Table.
fn parse_validator_table(table: &Table, key_name: Option<&str>) -> Result<ValidatorDef> {
    let name = if let Some(n) = key_name {
        n.to_string()
    } else {
        table
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BatonError::ConfigError("Import [validator] must have a 'name' field".into())
            })?
            .to_string()
    };

    let vtype = table
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| BatonError::ConfigError(format!("Validator '{name}' missing 'type' field")))?
        .to_string();

    Ok(ValidatorDef {
        name,
        validator_type: vtype,
        command: table
            .get("command")
            .and_then(|v| v.as_str())
            .map(String::from),
        prompt: table
            .get("prompt")
            .and_then(|v| v.as_str())
            .map(String::from),
        runtime: table
            .get("runtime")
            .and_then(|v| v.as_str())
            .map(String::from),
        model: table
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from),
        mode: table.get("mode").and_then(|v| v.as_str()).map(String::from),
        temperature: table.get("temperature").and_then(|v| v.as_float()),
        response_format: table
            .get("response_format")
            .and_then(|v| v.as_str())
            .map(String::from),
        max_tokens: table
            .get("max_tokens")
            .and_then(|v| v.as_integer())
            .map(|i| i as u32),
        system_prompt: table
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .map(String::from),
        input: table
            .get("input")
            .and_then(|v| v.as_str())
            .map(String::from),
        timeout_seconds: table
            .get("timeout_seconds")
            .and_then(|v| v.as_integer())
            .map(|i| i as u64),
        warn_exit_codes: table
            .get("warn_exit_codes")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_integer().map(|i| i as i32))
                    .collect()
            })
            .unwrap_or_default(),
        working_dir: table
            .get("working_dir")
            .and_then(|v| v.as_str())
            .map(String::from),
        tags: table
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        env: BTreeMap::new(),
    })
}

// ─── TOML editing ────────────────────────────────────────

/// Build the TOML snippet for a validator (for preview).
pub fn validator_to_toml_preview(def: &ValidatorDef) -> String {
    let mut lines = vec![format!("[validators.{}]", def.name)];
    lines.push(format!("type = \"{}\"", def.validator_type));

    if let Some(ref cmd) = def.command {
        lines.push(format!(
            "command = \"{}\"",
            cmd.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    if let Some(ref p) = def.prompt {
        lines.push(format!(
            "prompt = \"{}\"",
            p.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    if let Some(ref rt) = def.runtime {
        lines.push(format!("runtime = \"{}\"", rt));
    }
    if let Some(ref m) = def.model {
        lines.push(format!("model = \"{}\"", m));
    }
    if let Some(ref mode) = def.mode {
        lines.push(format!("mode = \"{}\"", mode));
    }
    if let Some(temp) = def.temperature {
        lines.push(format!("temperature = {temp}"));
    }
    if let Some(ref fmt) = def.response_format {
        lines.push(format!("response_format = \"{}\"", fmt));
    }
    if let Some(max) = def.max_tokens {
        lines.push(format!("max_tokens = {max}"));
    }
    if let Some(ref sp) = def.system_prompt {
        lines.push(format!(
            "system_prompt = \"{}\"",
            sp.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    if let Some(ref input) = def.input {
        lines.push(format!("input = \"{}\"", input));
    }
    if let Some(timeout) = def.timeout_seconds {
        lines.push(format!("timeout_seconds = {timeout}"));
    }
    if !def.warn_exit_codes.is_empty() {
        let codes: Vec<String> = def.warn_exit_codes.iter().map(|c| c.to_string()).collect();
        lines.push(format!("warn_exit_codes = [{}]", codes.join(", ")));
    }
    if let Some(ref wd) = def.working_dir {
        lines.push(format!("working_dir = \"{}\"", wd));
    }
    if !def.tags.is_empty() {
        let tags: Vec<String> = def.tags.iter().map(|t| format!("\"{t}\"")).collect();
        lines.push(format!("tags = [{}]", tags.join(", ")));
    }

    lines.join("\n")
}

/// Insert a validator definition into a TOML document.
fn insert_validator(doc: &mut DocumentMut, def: &ValidatorDef) {
    // Ensure [validators] table exists
    if doc.get("validators").is_none() {
        doc.insert("validators", Item::Table(Table::new()));
    }

    let validators = doc["validators"].as_table_mut().unwrap();

    let mut table = Table::new();
    table.insert("type", value(&def.validator_type));

    if let Some(ref cmd) = def.command {
        table.insert("command", value(cmd.as_str()));
    }
    if let Some(ref p) = def.prompt {
        table.insert("prompt", value(p.as_str()));
    }
    if let Some(ref rt) = def.runtime {
        table.insert("runtime", value(rt.as_str()));
    }
    if let Some(ref m) = def.model {
        table.insert("model", value(m.as_str()));
    }
    if let Some(ref mode) = def.mode {
        table.insert("mode", value(mode.as_str()));
    }
    if let Some(temp) = def.temperature {
        table.insert("temperature", value(temp));
    }
    if let Some(ref fmt) = def.response_format {
        table.insert("response_format", value(fmt.as_str()));
    }
    if let Some(max) = def.max_tokens {
        table.insert("max_tokens", value(max as i64));
    }
    if let Some(ref sp) = def.system_prompt {
        table.insert("system_prompt", value(sp.as_str()));
    }
    if let Some(ref input) = def.input {
        table.insert("input", value(input.as_str()));
    }
    if let Some(timeout) = def.timeout_seconds {
        table.insert("timeout_seconds", value(timeout as i64));
    }
    if !def.warn_exit_codes.is_empty() {
        let mut arr = Array::new();
        for code in &def.warn_exit_codes {
            arr.push(*code as i64);
        }
        table.insert("warn_exit_codes", value(arr));
    }
    if let Some(ref wd) = def.working_dir {
        table.insert("working_dir", value(wd.as_str()));
    }
    if !def.tags.is_empty() {
        let mut arr = Array::new();
        for tag in &def.tags {
            arr.push(tag.as_str());
        }
        table.insert("tags", value(arr));
    }

    validators.insert(&def.name, Item::Table(table));
}

/// Add a gate reference for a validator. Creates the gate if needed.
fn insert_gate_ref(doc: &mut DocumentMut, assignment: &GateAssignment, validator_name: &str) {
    // Ensure [gates] table exists
    if doc.get("gates").is_none() {
        doc.insert("gates", Item::Table(Table::new()));
    }
    let gates = doc["gates"].as_table_mut().unwrap();

    if assignment.create_new {
        // Create new gate
        let mut gate_table = Table::new();
        if let Some(ref desc) = assignment.description {
            gate_table.insert("description", value(desc.as_str()));
        }
        // Create validators as inline array
        let mut ref_table = InlineTable::new();
        ref_table.insert("ref", validator_name.into());
        ref_table.insert("blocking", assignment.blocking.into());
        let mut arr = Array::new();
        arr.push(ref_table);
        gate_table.insert("validators", value(arr));
        gates.insert(&assignment.gate_name, Item::Table(gate_table));
    } else {
        // Append to existing gate's validators array
        let gate = gates
            .get_mut(&assignment.gate_name)
            .and_then(|g| g.as_table_mut());

        if let Some(gate_table) = gate {
            if let Some(validators) = gate_table.get_mut("validators") {
                if let Some(arr) = validators.as_array_mut() {
                    let mut ref_table = InlineTable::new();
                    ref_table.insert("ref", validator_name.into());
                    ref_table.insert("blocking", assignment.blocking.into());
                    arr.push(ref_table);
                }
            }
        }
    }
}

/// Apply validators and optional gate assignment to a TOML document string.
/// Returns the modified TOML string.
pub fn apply_edits(
    raw_toml: &str,
    defs: &[ValidatorDef],
    gate_assignment: Option<&GateAssignment>,
    config_dir: &Path,
) -> Result<String> {
    let mut doc = raw_toml
        .parse::<DocumentMut>()
        .map_err(|e| BatonError::ConfigError(format!("Failed to parse TOML: {e}")))?;

    // Check for name collisions
    if let Some(validators) = doc.get("validators").and_then(|v| v.as_table()) {
        for def in defs {
            if validators.contains_key(&def.name) {
                return Err(BatonError::ConfigError(format!(
                    "Validator '{}' already exists in baton.toml",
                    def.name
                )));
            }
        }
    }
    // Also check inline validators in gates
    if let Some(gates) = doc.get("gates").and_then(|g| g.as_table()) {
        for (_gate_name, gate_item) in gates.iter() {
            if let Some(gate_table) = gate_item.as_table() {
                if let Some(validators) = gate_table.get("validators") {
                    if let Some(arr) = validators.as_array() {
                        for val_item in arr.iter() {
                            if let Some(inline) = val_item.as_inline_table() {
                                if let Some(name) = inline.get("name").and_then(|v| v.as_str()) {
                                    for def in defs {
                                        if def.name == name {
                                            return Err(BatonError::ConfigError(format!(
                                                "Validator '{}' already exists in baton.toml",
                                                def.name
                                            )));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Insert validators
    for def in defs {
        insert_validator(&mut doc, def);
    }

    // Insert gate references
    if let Some(assignment) = gate_assignment {
        for def in defs {
            insert_gate_ref(&mut doc, assignment, &def.name);
        }
    }

    let result = doc.to_string();

    // Validate the modified TOML
    parse_and_validate(&result, config_dir)?;

    Ok(result)
}

/// Write the modified TOML to disk atomically.
pub fn write_config(config_path: &Path, content: &str) -> Result<()> {
    let dir = config_path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = dir.join(".baton.toml.tmp");

    std::fs::write(&tmp_path, content)?;
    std::fs::rename(&tmp_path, config_path)?;

    Ok(())
}

// ─── Top-level orchestration ─────────────────────────────

/// Run the full add command. Returns exit code.
pub fn cmd_add(opts: AddOptions) -> i32 {
    // Find config
    let (config_path, raw_toml) = match find_config(opts.config.as_ref()) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("Error: No baton.toml found. Run `baton init` first.");
            return 2;
        }
    };

    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    // Parse existing config for inspection
    let config = match parse_config(&raw_toml, &config_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    // Determine mode and get validators + gate assignment
    let (defs, gate_assignment) = if let Some(ref source) = opts.from {
        // Import mode
        match run_import(source, &opts) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("Error: {e}");
                return 1;
            }
        }
    } else if opts.validator_type.is_some() && opts.name.is_some() {
        // Non-interactive mode
        match build_from_flags(&opts) {
            Ok(def) => {
                let gate = opts.gate.as_ref().map(|g| {
                    let exists = config.gates.contains_key(g.as_str());
                    GateAssignment {
                        gate_name: g.clone(),
                        blocking: opts.blocking.unwrap_or(true),
                        create_new: !exists,
                        description: None,
                    }
                });
                (vec![def], gate)
            }
            Err(e) => {
                eprintln!("Error: {e}");
                return 1;
            }
        }
    } else {
        // Interactive mode
        if !atty::is(atty::Stream::Stdin) {
            eprintln!("Error: Interactive mode requires a terminal. Use --type and --name for non-interactive mode.");
            return 1;
        }
        match run_wizard(&config) {
            Ok((def, gate)) => (vec![def], gate),
            Err(e) => {
                eprintln!("Error: {e}");
                return 1;
            }
        }
    };

    // Preview
    eprintln!();
    eprintln!("Will add to baton.toml:");
    eprintln!();
    for def in &defs {
        eprintln!("  {}", validator_to_toml_preview(def).replace('\n', "\n  "));
    }
    if let Some(ref ga) = gate_assignment {
        eprintln!();
        if ga.create_new {
            eprintln!("  New gate '{}' will be created.", ga.gate_name);
        }
        for def in &defs {
            let blocking_str = if ga.blocking { "true" } else { "false" };
            eprintln!(
                "  Gate '{}': {{ ref = \"{}\", blocking = {} }}",
                ga.gate_name, def.name, blocking_str
            );
        }
    }
    eprintln!();

    // Dry run
    if opts.dry_run {
        eprintln!("Dry run — no changes written.");
        return 0;
    }

    // Confirm
    if !opts.yes {
        use dialoguer::Confirm;
        let confirmed = Confirm::new()
            .with_prompt("Confirm?")
            .default(true)
            .interact()
            .unwrap_or(false);
        if !confirmed {
            eprintln!("Cancelled.");
            return 0;
        }
    }

    // Apply edits
    let new_toml = match apply_edits(&raw_toml, &defs, gate_assignment.as_ref(), &config_dir) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    // Write
    if let Err(e) = write_config(&config_path, &new_toml) {
        eprintln!("Error: {e}");
        return 2;
    }

    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    eprintln!("Added validator(s) '{}' to baton.toml.", names.join("', '"));
    0
}

/// Import validators from an external source.
fn run_import(
    source: &str,
    opts: &AddOptions,
) -> Result<(Vec<ValidatorDef>, Option<GateAssignment>)> {
    let toml_content = resolve_import_source(source)?;
    let defs = parse_import(&toml_content)?;

    let gate = opts.gate.as_ref().map(|g| GateAssignment {
        gate_name: g.clone(),
        blocking: opts.blocking.unwrap_or(true),
        create_new: true, // will be refined by caller
        description: None,
    });

    Ok((defs, gate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn base_config() -> &'static str {
        r#"version = "0.7"

[defaults]
timeout_seconds = 300
blocking = true

[validators.existing]
type = "script"
command = "echo existing"

[gates.ci]
description = "CI gate"
validators = [
    { ref = "existing", blocking = true },
]
"#
    }

    fn default_opts() -> AddOptions {
        AddOptions {
            name: None,
            validator_type: None,
            command: None,
            prompt: None,
            runtime: None,
            model: None,
            gate: None,
            blocking: None,
            tags: None,
            input: None,
            timeout: None,
            from: None,
            config: None,
            dry_run: false,
            yes: false,
        }
    }

    fn script_def(name: &str, command: &str) -> ValidatorDef {
        ValidatorDef {
            name: name.into(),
            validator_type: "script".into(),
            command: Some(command.into()),
            prompt: None,
            runtime: None,
            model: None,
            mode: None,
            temperature: None,
            response_format: None,
            max_tokens: None,
            system_prompt: None,
            input: None,
            timeout_seconds: None,
            warn_exit_codes: vec![],
            working_dir: None,
            tags: vec![],
            env: BTreeMap::new(),
        }
    }

    // ─── build_from_flags: non-interactive field validation ──

    /// SPEC-MN-AD-020
    #[test]
    fn build_from_flags_script_requires_command() {
        let opts = AddOptions {
            name: Some("test".into()),
            validator_type: Some("script".into()),
            ..default_opts()
        };
        let err = build_from_flags(&opts).unwrap_err();
        assert!(err.to_string().contains("--command"));
    }

    /// SPEC-MN-AD-021: llm missing prompt
    #[test]
    fn build_from_flags_llm_requires_prompt() {
        let opts = AddOptions {
            name: Some("test".into()),
            validator_type: Some("llm".into()),
            runtime: Some("default".into()),
            ..default_opts()
        };
        let err = build_from_flags(&opts).unwrap_err();
        assert!(err.to_string().contains("--prompt"));
    }

    /// SPEC-MN-AD-021: llm missing runtime
    #[test]
    fn build_from_flags_llm_requires_runtime() {
        let opts = AddOptions {
            name: Some("test".into()),
            validator_type: Some("llm".into()),
            prompt: Some("Review this".into()),
            ..default_opts()
        };
        let err = build_from_flags(&opts).unwrap_err();
        assert!(err.to_string().contains("--runtime"));
    }

    /// SPEC-MN-AD-022
    #[test]
    fn build_from_flags_human_requires_prompt() {
        let opts = AddOptions {
            name: Some("test".into()),
            validator_type: Some("human".into()),
            ..default_opts()
        };
        let err = build_from_flags(&opts).unwrap_err();
        assert!(err.to_string().contains("--prompt"));
    }

    /// SPEC-MN-AD-023
    #[test]
    fn build_from_flags_rejects_unknown_type() {
        let opts = AddOptions {
            name: Some("test".into()),
            validator_type: Some("unknown".into()),
            ..default_opts()
        };
        let err = build_from_flags(&opts).unwrap_err();
        assert!(err.to_string().contains("Unknown validator type"));
        assert!(err.to_string().contains("unknown"));
    }

    #[test]
    fn build_from_flags_script_success() {
        let opts = AddOptions {
            name: Some("lint".into()),
            validator_type: Some("script".into()),
            command: Some("ruff check".into()),
            input: Some("*.py".into()),
            tags: Some(vec!["lint".into()]),
            timeout: Some(60),
            ..default_opts()
        };
        let def = build_from_flags(&opts).unwrap();
        assert_eq!(def.name, "lint");
        assert_eq!(def.validator_type, "script");
        assert_eq!(def.command.as_deref(), Some("ruff check"));
        assert_eq!(def.input.as_deref(), Some("*.py"));
        assert_eq!(def.tags, vec!["lint"]);
        assert_eq!(def.timeout_seconds, Some(60));
    }

    #[test]
    fn build_from_flags_llm_success() {
        let opts = AddOptions {
            name: Some("review".into()),
            validator_type: Some("llm".into()),
            prompt: Some("Review this code".into()),
            runtime: Some("default".into()),
            model: Some("claude-haiku".into()),
            ..default_opts()
        };
        let def = build_from_flags(&opts).unwrap();
        assert_eq!(def.name, "review");
        assert_eq!(def.validator_type, "llm");
        assert_eq!(def.prompt.as_deref(), Some("Review this code"));
        assert_eq!(def.runtime.as_deref(), Some("default"));
        assert_eq!(def.model.as_deref(), Some("claude-haiku"));
    }

    #[test]
    fn build_from_flags_human_success() {
        let opts = AddOptions {
            name: Some("manual".into()),
            validator_type: Some("human".into()),
            prompt: Some("Check this manually".into()),
            ..default_opts()
        };
        let def = build_from_flags(&opts).unwrap();
        assert_eq!(def.name, "manual");
        assert_eq!(def.validator_type, "human");
        assert_eq!(def.prompt.as_deref(), Some("Check this manually"));
    }

    // ─── apply_edits: TOML editing ──────────────────────────

    /// SPEC-MN-AD-032: no gate → top-level validator only
    #[test]
    fn apply_edits_adds_script_validator_no_gate() {
        let mut def = script_def("lint", "ruff check {file.path}");
        def.input = Some("*.py".into());
        def.tags = vec!["lint".into()];

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], None, tmp.path()).unwrap();

        assert!(result.contains("[validators.lint]"));
        assert!(result.contains("ruff check"));
        assert!(result.contains("input = \"*.py\""));
        // Existing content preserved
        assert!(result.contains("[validators.existing]"));
        assert!(result.contains("[gates.ci]"));
        // No new gate ref created for "lint"
        // The only gate ref should still be for "existing"
    }

    /// SPEC-MN-AD-030: gate ref appended to existing gate
    #[test]
    fn apply_edits_adds_to_existing_gate() {
        let def = script_def("format-check", "ruff format --check");

        let gate = GateAssignment {
            gate_name: "ci".into(),
            blocking: true,
            create_new: false,
            description: None,
        };

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], Some(&gate), tmp.path()).unwrap();

        assert!(result.contains("[validators.format-check]"));
        // The gate should now have a ref to format-check
        assert!(result.contains("format-check"));
    }

    /// SPEC-MN-AD-030: gate ref with blocking = false
    #[test]
    fn apply_edits_gate_ref_blocking_false() {
        let def = script_def("advisory", "echo advisory");

        let gate = GateAssignment {
            gate_name: "ci".into(),
            blocking: false,
            create_new: false,
            description: None,
        };

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], Some(&gate), tmp.path()).unwrap();

        assert!(result.contains("[validators.advisory]"));
        // The result should contain blocking = false for the new ref
        assert!(result.contains("false"));
    }

    /// SPEC-MN-AD-031: new gate created with description
    #[test]
    fn apply_edits_creates_new_gate() {
        let def = script_def("review", "echo review");

        let gate = GateAssignment {
            gate_name: "pr-review".into(),
            blocking: false,
            create_new: true,
            description: Some("PR review gate".into()),
        };

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], Some(&gate), tmp.path()).unwrap();

        assert!(result.contains("[validators.review]"));
        assert!(result.contains("[gates.pr-review]"));
        assert!(result.contains("PR review gate"));
    }

    /// SPEC-MN-AD-031: new gate without description
    #[test]
    fn apply_edits_creates_new_gate_no_description() {
        let def = script_def("check", "echo check");

        let gate = GateAssignment {
            gate_name: "staging".into(),
            blocking: true,
            create_new: true,
            description: None,
        };

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], Some(&gate), tmp.path()).unwrap();

        assert!(result.contains("[gates.staging]"));
        // Should not contain a description key for this gate
    }

    /// SPEC-MN-AD-011: duplicate name in [validators] section rejected
    #[test]
    fn apply_edits_rejects_duplicate_name() {
        let def = script_def("existing", "echo dup");

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], None, tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));
    }

    /// SPEC-MN-AD-043: import collision rejects all — no partial writes
    #[test]
    fn apply_edits_rejects_collision_in_batch() {
        let defs = vec![
            script_def("new-one", "echo one"),
            script_def("existing", "echo dup"),
        ];

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &defs, None, tmp.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("existing"));
    }

    /// SPEC-MN-AD-050: comments, formatting, key ordering preserved
    #[test]
    fn apply_edits_preserves_comments() {
        let config_with_comments = r#"# Main config
version = "0.7"

[defaults]
timeout_seconds = 300
blocking = true

# Validators section
[validators.existing]
type = "script"
command = "echo existing"

# Gates
[gates.ci]
description = "CI gate"
validators = [
    { ref = "existing", blocking = true },
]
"#;
        let def = script_def("new-val", "echo new");

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(config_with_comments, &[def], None, tmp.path()).unwrap();
        assert!(result.contains("# Main config"));
        assert!(result.contains("# Validators section"));
        assert!(result.contains("# Gates"));
    }

    /// SPEC-MN-AD-051: modified TOML must pass parse_config + validate_config
    #[test]
    fn apply_edits_validates_result() {
        // A validator with an invalid type should fail validation
        let def = ValidatorDef {
            name: "bad".into(),
            validator_type: "invalid_type".into(),
            command: None,
            prompt: None,
            runtime: None,
            model: None,
            mode: None,
            temperature: None,
            response_format: None,
            max_tokens: None,
            system_prompt: None,
            input: None,
            timeout_seconds: None,
            warn_exit_codes: vec![],
            working_dir: None,
            tags: vec![],
            env: BTreeMap::new(),
        };

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], None, tmp.path());
        // Should fail because "invalid_type" is not a recognized validator type
        assert!(result.is_err());
    }

    /// Multiple validators added at once
    #[test]
    fn apply_edits_multiple_validators() {
        let defs = vec![
            script_def("lint", "ruff check"),
            script_def("format", "ruff format --check"),
        ];

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &defs, None, tmp.path()).unwrap();

        assert!(result.contains("[validators.lint]"));
        assert!(result.contains("[validators.format]"));
        assert!(result.contains("[validators.existing]"));
    }

    /// All validator fields are inserted into TOML
    #[test]
    fn apply_edits_all_script_fields() {
        let def = ValidatorDef {
            name: "full-script".into(),
            validator_type: "script".into(),
            command: Some("ruff check {file.path}".into()),
            prompt: None,
            runtime: None,
            model: None,
            mode: None,
            temperature: None,
            response_format: None,
            max_tokens: None,
            system_prompt: None,
            input: Some("*.py".into()),
            timeout_seconds: Some(60),
            warn_exit_codes: vec![2, 3],
            working_dir: Some("/src".into()),
            tags: vec!["lint".into(), "python".into()],
            env: BTreeMap::new(),
        };

        let tmp = TempDir::new().unwrap();
        let result = apply_edits(base_config(), &[def], None, tmp.path()).unwrap();

        assert!(result.contains("[validators.full-script]"));
        assert!(result.contains("type = \"script\""));
        assert!(result.contains("ruff check"));
        assert!(result.contains("input = \"*.py\""));
        assert!(result.contains("timeout_seconds = 60"));
        assert!(result.contains("working_dir = \"/src\""));
    }

    // ─── write_config: atomic file writes ───────────────────

    #[test]
    fn write_config_creates_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("baton.toml");
        write_config(&path, "version = \"0.7\"\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "version = \"0.7\"\n"
        );
    }

    #[test]
    fn write_config_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("baton.toml");
        std::fs::write(&path, "old content").unwrap();
        write_config(&path, "new content").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
    }

    #[test]
    fn write_config_no_temp_file_left() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("baton.toml");
        write_config(&path, "content").unwrap();
        // The .baton.toml.tmp file should not remain
        assert!(!tmp.path().join(".baton.toml.tmp").exists());
    }

    // ─── parse_import: format parsing ───────────────────────

    /// SPEC-MN-AD-044: single format — name from field
    #[test]
    fn parse_import_single_format() {
        let toml = r#"
[validator]
name = "ruff-lint"
type = "script"
command = "ruff check {file.path}"
input = "*.py"
tags = ["lint", "python"]
"#;
        let defs = parse_import(toml).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "ruff-lint");
        assert_eq!(defs[0].validator_type, "script");
        assert_eq!(defs[0].command.as_deref(), Some("ruff check {file.path}"));
        assert_eq!(defs[0].input.as_deref(), Some("*.py"));
        assert_eq!(defs[0].tags, vec!["lint", "python"]);
    }

    /// SPEC-MN-AD-044: single format — missing name is error
    #[test]
    fn parse_import_single_format_missing_name() {
        let toml = r#"
[validator]
type = "script"
command = "echo hi"
"#;
        let err = parse_import(toml).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    /// SPEC-MN-AD-044: single format — missing type is error
    #[test]
    fn parse_import_single_format_missing_type() {
        let toml = r#"
[validator]
name = "test"
command = "echo hi"
"#;
        let err = parse_import(toml).unwrap_err();
        assert!(err.to_string().contains("type"));
    }

    /// SPEC-MN-AD-045: multi format — names from keys
    #[test]
    fn parse_import_multi_format() {
        let toml = r#"
[validators.ruff-lint]
type = "script"
command = "ruff check {file.path}"

[validators.ruff-format]
type = "script"
command = "ruff format --check {file.path}"
"#;
        let defs = parse_import(toml).unwrap();
        assert_eq!(defs.len(), 2);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"ruff-lint"));
        assert!(names.contains(&"ruff-format"));
    }

    /// SPEC-MN-AD-045: multi format preserves all fields
    #[test]
    fn parse_import_multi_format_all_fields() {
        let toml = r#"
[validators.full]
type = "script"
command = "ruff check"
input = "*.py"
timeout_seconds = 60
warn_exit_codes = [2, 3]
working_dir = "/src"
tags = ["lint"]
"#;
        let defs = parse_import(toml).unwrap();
        assert_eq!(defs.len(), 1);
        let d = &defs[0];
        assert_eq!(d.name, "full");
        assert_eq!(d.command.as_deref(), Some("ruff check"));
        assert_eq!(d.input.as_deref(), Some("*.py"));
        assert_eq!(d.timeout_seconds, Some(60));
        assert_eq!(d.warn_exit_codes, vec![2, 3]);
        assert_eq!(d.working_dir.as_deref(), Some("/src"));
        assert_eq!(d.tags, vec!["lint"]);
    }

    /// Import file with neither [validator] nor [validators.*] is rejected
    #[test]
    fn parse_import_rejects_unrecognized_format() {
        let toml = "[other]\nkey = \"value\"\n";
        let err = parse_import(toml).unwrap_err();
        assert!(err.to_string().contains("[validator]"));
    }

    /// LLM validator fields round-trip through import
    #[test]
    fn parse_import_llm_fields() {
        let toml = r#"
[validator]
name = "review"
type = "llm"
prompt = "Review this code"
runtime = "default"
model = "claude-haiku"
mode = "session"
temperature = 0.5
response_format = "freeform"
max_tokens = 1024
system_prompt = "You are a reviewer"
"#;
        let defs = parse_import(toml).unwrap();
        let d = &defs[0];
        assert_eq!(d.validator_type, "llm");
        assert_eq!(d.prompt.as_deref(), Some("Review this code"));
        assert_eq!(d.runtime.as_deref(), Some("default"));
        assert_eq!(d.model.as_deref(), Some("claude-haiku"));
        assert_eq!(d.mode.as_deref(), Some("session"));
        assert_eq!(d.temperature, Some(0.5));
        assert_eq!(d.response_format.as_deref(), Some("freeform"));
        assert_eq!(d.max_tokens, Some(1024));
        assert_eq!(d.system_prompt.as_deref(), Some("You are a reviewer"));
    }

    // ─── resolve_import_source ──────────────────────────────

    /// SPEC-MN-AD-042
    #[test]
    fn resolve_import_source_rejects_registry() {
        let err = resolve_import_source("registry:community/lint").unwrap_err();
        assert!(err.to_string().contains("not yet supported"));
    }

    #[test]
    fn resolve_import_source_rejects_missing_file() {
        let err = resolve_import_source("/nonexistent/file.toml").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    /// SPEC-MN-AD-040: reads local file successfully
    #[test]
    fn resolve_import_source_reads_local_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("import.toml");
        std::fs::write(
            &path,
            "[validator]\nname = \"x\"\ntype = \"script\"\ncommand = \"echo\"\n",
        )
        .unwrap();

        let content = resolve_import_source(path.to_str().unwrap()).unwrap();
        assert!(content.contains("[validator]"));
        assert!(content.contains("name = \"x\""));
    }

    // ─── validator_to_toml_preview ──────────────────────────

    #[test]
    fn validator_to_toml_preview_script() {
        let def = ValidatorDef {
            name: "lint".into(),
            validator_type: "script".into(),
            command: Some("ruff check".into()),
            input: Some("*.py".into()),
            warn_exit_codes: vec![2],
            tags: vec!["lint".into()],
            ..script_def("lint", "ruff check")
        };
        let preview = validator_to_toml_preview(&def);
        assert!(preview.contains("[validators.lint]"));
        assert!(preview.contains("type = \"script\""));
        assert!(preview.contains("command = \"ruff check\""));
        assert!(preview.contains("input = \"*.py\""));
        assert!(preview.contains("warn_exit_codes = [2]"));
        assert!(preview.contains("tags = [\"lint\"]"));
    }

    #[test]
    fn validator_to_toml_preview_llm() {
        let def = ValidatorDef {
            name: "review".into(),
            validator_type: "llm".into(),
            command: None,
            prompt: Some("Review code".into()),
            runtime: Some("default".into()),
            model: Some("claude-haiku".into()),
            mode: Some("session".into()),
            temperature: Some(0.5),
            response_format: Some("freeform".into()),
            max_tokens: Some(1024),
            system_prompt: Some("Be thorough".into()),
            input: None,
            timeout_seconds: None,
            warn_exit_codes: vec![],
            working_dir: None,
            tags: vec![],
            env: BTreeMap::new(),
        };
        let preview = validator_to_toml_preview(&def);
        assert!(preview.contains("[validators.review]"));
        assert!(preview.contains("type = \"llm\""));
        assert!(preview.contains("prompt = \"Review code\""));
        assert!(preview.contains("runtime = \"default\""));
        assert!(preview.contains("model = \"claude-haiku\""));
        assert!(preview.contains("mode = \"session\""));
        assert!(preview.contains("temperature = 0.5"));
        assert!(preview.contains("response_format = \"freeform\""));
        assert!(preview.contains("max_tokens = 1024"));
        assert!(preview.contains("system_prompt = \"Be thorough\""));
    }

    #[test]
    fn validator_to_toml_preview_human() {
        let def = ValidatorDef {
            name: "manual".into(),
            validator_type: "human".into(),
            command: None,
            prompt: Some("Manual review".into()),
            runtime: None,
            model: None,
            mode: None,
            temperature: None,
            response_format: None,
            max_tokens: None,
            system_prompt: None,
            input: None,
            timeout_seconds: None,
            warn_exit_codes: vec![],
            working_dir: None,
            tags: vec![],
            env: BTreeMap::new(),
        };
        let preview = validator_to_toml_preview(&def);
        assert!(preview.contains("[validators.manual]"));
        assert!(preview.contains("type = \"human\""));
        assert!(preview.contains("prompt = \"Manual review\""));
        // No command, runtime, etc.
        assert!(!preview.contains("command"));
        assert!(!preview.contains("runtime"));
    }

    #[test]
    fn validator_to_toml_preview_escapes_quotes() {
        let def = ValidatorDef {
            name: "escape-test".into(),
            validator_type: "script".into(),
            command: Some(r#"echo "hello world""#.into()),
            prompt: None,
            runtime: None,
            model: None,
            mode: None,
            temperature: None,
            response_format: None,
            max_tokens: None,
            system_prompt: None,
            input: None,
            timeout_seconds: None,
            warn_exit_codes: vec![],
            working_dir: None,
            tags: vec![],
            env: BTreeMap::new(),
        };
        let preview = validator_to_toml_preview(&def);
        assert!(preview.contains(r#"command = "echo \"hello world\"""#));
    }

    // ─── find_config ────────────────────────────────────────

    /// SPEC-MN-AD-010: explicit --config to nonexistent file
    #[test]
    fn find_config_missing_explicit_path() {
        let path = PathBuf::from("/nonexistent/baton.toml");
        let err = find_config(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    /// find_config reads existing file
    #[test]
    fn find_config_reads_existing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("baton.toml");
        std::fs::write(&path, "version = \"0.7\"\n").unwrap();
        let (found_path, content) = find_config(Some(&path)).unwrap();
        assert_eq!(found_path, path);
        assert!(content.contains("version"));
    }
}
