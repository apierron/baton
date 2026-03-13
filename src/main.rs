//! CLI entry point for baton.
//!
//! Provides subcommands for running gates (`check`), project setup (`init`),
//! inspecting configuration (`list`, `validate-config`), querying history,
//! checking provider/runtime connectivity, and managing the installation
//! (`update`, `uninstall`, `clean`, `version`).

use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;
use std::process;

use baton::config::{discover_config, parse_config, validate_config};
use baton::exec::run_gate;
use baton::history;
use baton::types::*;

#[derive(Parser)]
#[command(name = "baton", version = env!("CARGO_PKG_VERSION"), about = "A composable validation gate for AI agent outputs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a gate against an artifact
    Check {
        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,

        /// Gate to run
        #[arg(long)]
        gate: String,

        /// Path to the artifact (use '-' for stdin)
        #[arg(long)]
        artifact: String,

        /// Context items (name=path, repeatable)
        #[arg(long, value_parser = parse_context_arg)]
        context: Vec<(String, String)>,

        /// Run all validators even if a blocking one fails
        #[arg(long)]
        all: bool,

        /// Run only named validators (comma-separated)
        #[arg(long, value_delimiter = ',')]
        only: Option<Vec<String>>,

        /// Skip named validators (comma-separated)
        #[arg(long, value_delimiter = ',')]
        skip: Option<Vec<String>>,

        /// Run only validators with these tags (comma-separated)
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        /// Override default timeout for all validators
        #[arg(long)]
        timeout: Option<u64>,

        /// Output format
        #[arg(long, default_value = "json")]
        format: String,

        /// Print validators that would run and exit
        #[arg(long)]
        dry_run: bool,

        /// Don't write to history database or log files
        #[arg(long)]
        no_log: bool,

        /// Print each validator's result as it completes
        #[arg(short, long)]
        verbose: bool,

        /// Treat warn statuses as pass
        #[arg(long)]
        suppress_warnings: bool,

        /// Treat error statuses as pass
        #[arg(long)]
        suppress_errors: bool,

        /// Suppress both warnings and errors
        #[arg(long)]
        suppress_all: bool,
    },

    /// Initialize a new baton project
    Init {
        /// Only create baton.toml and .baton/ directory
        #[arg(long)]
        minimal: bool,

        /// Only create the prompts/ directory with starter templates
        #[arg(long)]
        prompts_only: bool,
    },

    /// List available gates and validators
    List {
        /// Show validators for a specific gate
        #[arg(long)]
        gate: Option<String>,

        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Query verdict history
    History {
        /// Filter by gate name
        #[arg(long)]
        gate: Option<String>,

        /// Filter by status
        #[arg(long)]
        status: Option<String>,

        /// Filter by artifact hash
        #[arg(long)]
        artifact_hash: Option<String>,

        /// Number of results
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Validate baton.toml configuration
    ValidateConfig {
        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Remove stale temporary files
    Clean {
        /// Show what would be cleaned without deleting
        #[arg(long)]
        dry_run: bool,

        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Check provider connectivity and model availability
    CheckProvider {
        /// Provider name (omit to check the first configured provider)
        name: Option<String>,

        /// Check all configured providers
        #[arg(long)]
        all: bool,

        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Check runtime connectivity and health
    CheckRuntime {
        /// Runtime name (omit to check the first configured runtime)
        name: Option<String>,

        /// Check all configured runtimes
        #[arg(long)]
        all: bool,

        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Print version information
    Version {
        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Update baton to the latest version
    Update {
        /// Install a specific version (e.g. "0.4.2" or "v0.4.2")
        #[arg(long)]
        version: Option<String>,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Uninstall baton from this system
    Uninstall {
        /// Remove all baton installations, not just the one in PATH
        #[arg(long)]
        all: bool,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

/// Parses a `name=path` context argument from the CLI.
fn parse_context_arg(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid context format: '{s}'. Expected name=path"));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Loads and parses baton.toml from an explicit path or by discovery.
fn load_config(config_path: Option<&PathBuf>) -> baton::error::Result<(baton::config::BatonConfig, PathBuf)> {
    let config_file = match config_path {
        Some(p) => {
            if !p.exists() {
                return Err(baton::error::BatonError::ConfigError(format!(
                    "Config file not found: {}",
                    p.display()
                )));
            }
            p.clone()
        }
        None => discover_config(&std::env::current_dir()?)?,
    };

    let config_dir = config_file.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf();
    let toml_str = std::fs::read_to_string(&config_file)?;
    let config = parse_config(&toml_str, &config_dir)?;
    Ok((config, config_file))
}

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Check {
            config,
            gate,
            artifact,
            context,
            all,
            only,
            skip,
            tags,
            timeout,
            format,
            dry_run,
            no_log,
            verbose: _,
            suppress_warnings,
            suppress_errors,
            suppress_all,
        } => cmd_check(
            config.as_ref(),
            &gate,
            &artifact,
            &context,
            all,
            only,
            skip,
            tags,
            timeout,
            &format,
            dry_run,
            no_log,
            suppress_warnings,
            suppress_errors,
            suppress_all,
        ),
        Commands::Init { minimal, prompts_only } => cmd_init(minimal, prompts_only),
        Commands::List { gate, config } => cmd_list(config.as_ref(), gate.as_deref()),
        Commands::History {
            gate,
            status,
            artifact_hash,
            limit,
            config,
        } => cmd_history(config.as_ref(), gate.as_deref(), status.as_deref(), artifact_hash.as_deref(), limit),
        Commands::ValidateConfig { config } => cmd_validate_config(config.as_ref()),
        Commands::CheckProvider { name, all, config } => cmd_check_provider(config.as_ref(), name.as_deref(), all),
        Commands::CheckRuntime { name, all, config } => cmd_check_runtime(config.as_ref(), name.as_deref(), all),
        Commands::Clean { dry_run, config } => cmd_clean(config.as_ref(), dry_run),
        Commands::Version { config } => cmd_version(config.as_ref()),
        Commands::Update { version, yes } => cmd_update(version, yes),
        Commands::Uninstall { all, yes } => cmd_uninstall(all, yes),
    };

    process::exit(exit_code);
}

/// Executes the `check` subcommand: loads config, builds artifact/context,
/// runs the gate pipeline, stores the verdict in history, and outputs the result.
#[allow(clippy::too_many_arguments)]
fn cmd_check(
    config_path: Option<&PathBuf>,
    gate_name: &str,
    artifact_path: &str,
    context_args: &[(String, String)],
    run_all: bool,
    only: Option<Vec<String>>,
    skip: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    timeout: Option<u64>,
    format: &str,
    dry_run: bool,
    no_log: bool,
    suppress_warnings: bool,
    suppress_errors: bool,
    suppress_all: bool,
) -> i32 {
    let (config, _config_file) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    let gate = match config.gates.get(gate_name) {
        Some(g) => g,
        None => {
            let available: Vec<&String> = config.gates.keys().collect();
            eprintln!(
                "Error: Gate '{gate_name}' not found. Available gates: {}",
                available.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            );
            return 2;
        }
    };

    // Validate --only references
    if let Some(ref only_list) = only {
        let validator_names: Vec<&str> = gate.validators.iter().map(|v| v.name.as_str()).collect();
        for name in only_list {
            if !validator_names.contains(&name.as_str()) {
                eprintln!("Error: --only references unknown validator '{name}'");
                return 2;
            }
        }
    }

    // Validate --skip references
    if let Some(ref skip_list) = skip {
        let validator_names: Vec<&str> = gate.validators.iter().map(|v| v.name.as_str()).collect();
        for name in skip_list {
            if !validator_names.contains(&name.as_str()) {
                eprintln!("Warning: --skip references unknown validator '{name}'");
            }
        }
    }

    // Build artifact
    let mut artifact = if artifact_path == "-" {
        let mut content = Vec::new();
        if let Err(e) = std::io::stdin().read_to_end(&mut content) {
            eprintln!("Error reading stdin: {e}");
            return 2;
        }
        // Write to temp file
        let tmp_dir = &config.defaults.tmp_dir;
        if let Err(e) = std::fs::create_dir_all(tmp_dir) {
            eprintln!("Error creating tmp dir: {e}");
            return 2;
        }
        let tmp_path = tmp_dir.join(format!("stdin-{}.tmp", uuid::Uuid::new_v4()));
        if let Err(e) = std::fs::write(&tmp_path, &content) {
            eprintln!("Error writing temp file: {e}");
            return 2;
        }
        match Artifact::from_file(&tmp_path) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Error: {e}");
                return 2;
            }
        }
    } else {
        match Artifact::from_file(artifact_path) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Error: {e}");
                return 2;
            }
        }
    };

    // Build context
    let mut context = Context::new();
    for (name, path) in context_args {
        if path == "-" {
            eprintln!("Error: stdin context not supported in this version");
            return 2;
        }
        let p = std::path::Path::new(path);
        if p.exists() {
            if let Err(e) = context.add_file(name.clone(), p) {
                eprintln!("Error: {e}");
                return 2;
            }
        } else {
            context.add_string(name.clone(), path.clone());
        }
    }

    // Dry run
    if dry_run {
        eprintln!("Dry run: validators that would execute for gate '{gate_name}':");
        for v in &gate.validators {
            let skip_reason = if let Some(ref o) = only {
                if !o.contains(&v.name) { Some("--only") } else { None }
            } else { None };

            let skip_reason = skip_reason.or_else(|| {
                if let Some(ref s) = skip {
                    if s.contains(&v.name) { Some("--skip") } else { None }
                } else { None }
            });

            let skip_reason = skip_reason.or_else(|| {
                if let Some(ref t) = tags {
                    if !v.tags.iter().any(|vt| t.contains(vt)) { Some("--tags") } else { None }
                } else { None }
            });

            match skip_reason {
                Some(reason) => eprintln!("  — {} (skipped by {reason})", v.name),
                None => {
                    let run_if_note = match &v.run_if {
                        Some(expr) if run_all => format!(" (run_if: {expr}, depends on runtime)"),
                        Some(expr) => format!(" (run_if: {expr})"),
                        None => String::new(),
                    };
                    eprintln!("  ✓ {} [{}]{run_if_note}", v.name, v.validator_type_str());
                }
            }
        }
        return 0;
    }

    // Build run options
    let mut suppressed_statuses = Vec::new();
    if suppress_all || suppress_warnings {
        suppressed_statuses.push(Status::Warn);
    }
    if suppress_all || suppress_errors {
        suppressed_statuses.push(Status::Error);
    }
    if suppress_all {
        suppressed_statuses.push(Status::Fail);
    }

    let options = RunOptions {
        run_all,
        only,
        skip,
        tags,
        timeout,
        log: !no_log,
        suppressed_statuses,
    };

    // Run the gate
    let verdict = match run_gate(gate, &config, &mut artifact, &mut context, &options) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    // Store in history
    if options.log {
        let db_path = &config.defaults.history_db;
        if let Err(e) = std::fs::create_dir_all(db_path.parent().unwrap_or_else(|| std::path::Path::new("."))) {
            eprintln!("Warning: could not create history directory: {e}");
        } else {
            match history::init_db(db_path) {
                Ok(conn) => {
                    if let Err(e) = history::store_verdict(&conn, &verdict) {
                        eprintln!("Warning: could not store verdict: {e}");
                    }
                }
                Err(e) => eprintln!("Warning: could not open history database: {e}"),
            }
        }
    }

    // Output
    match format {
        "json" => println!("{}", verdict.to_json()),
        "human" => eprintln!("{}", verdict.to_human()),
        "summary" => eprintln!("{}", verdict.to_summary()),
        other => {
            eprintln!("Unknown format: {other}. Using json.");
            println!("{}", verdict.to_json());
        }
    }

    // Clean up stdin temp file
    if artifact_path == "-" {
        if let Some(ref path) = artifact.path {
            let _ = std::fs::remove_file(path);
        }
    }

    verdict.status.exit_code()
}

/// Initializes a new baton project: creates `baton.toml`, `.baton/` directory,
/// and optionally starter prompt templates in `prompts/`.
fn cmd_init(minimal: bool, prompts_only: bool) -> i32 {
    if !prompts_only {
        // Check if baton.toml already exists
        if std::path::Path::new("baton.toml").exists() {
            eprintln!("Error: baton.toml already exists. Will not overwrite.");
            return 1;
        }

        // Create .baton directory structure
        let baton_dir = std::path::Path::new(".baton");
        if baton_dir.exists() {
            eprintln!("Warning: .baton/ already exists. Creating missing subdirectories.");
        }
        for subdir in &["logs", "tmp"] {
            let dir = baton_dir.join(subdir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("Error creating {}: {e}", dir.display());
                return 1;
            }
        }

        // Write starter baton.toml
        let starter_config = r#"version = "0.4"

[defaults]
timeout_seconds = 300
blocking = true
prompts_dir = "./prompts"
log_dir = "./.baton/logs"
history_db = "./.baton/history.db"
tmp_dir = "./.baton/tmp"

# [providers.default]
# api_base = "https://api.anthropic.com"
# api_key_env = "ANTHROPIC_API_KEY"
# default_model = "claude-haiku"

[gates.example]
description = "Example validation gate"

[[gates.example.validators]]
name = "lint"
type = "script"
command = "echo 'Replace with your lint command' && exit 0"
blocking = true
"#;
        if let Err(e) = std::fs::write("baton.toml", starter_config) {
            eprintln!("Error writing baton.toml: {e}");
            return 1;
        }
        eprintln!("Created baton.toml");
        eprintln!("Created .baton/");
    }

    if !minimal {
        // Create prompts directory with starter templates
        let prompts_dir = std::path::Path::new("prompts");
        if let Err(e) = std::fs::create_dir_all(prompts_dir) {
            eprintln!("Error creating prompts/: {e}");
            return 1;
        }

        let templates = [
            ("spec-compliance.md", include_str!("../templates/spec-compliance.md")),
            ("adversarial-review.md", include_str!("../templates/adversarial-review.md")),
            ("doc-completeness.md", include_str!("../templates/doc-completeness.md")),
        ];

        for (name, content) in &templates {
            let path = prompts_dir.join(name);
            if !path.exists() {
                if let Err(e) = std::fs::write(&path, content) {
                    eprintln!("Error writing {}: {e}", path.display());
                    return 1;
                }
                eprintln!("Created prompts/{name}");
            }
        }
    }

    eprintln!("baton project initialized.");
    0
}

/// Lists available gates, or shows validators for a specific gate.
fn cmd_list(config_path: Option<&PathBuf>, gate_name: Option<&str>) -> i32 {
    let (config, _) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    match gate_name {
        Some(name) => {
            let gate = match config.gates.get(name) {
                Some(g) => g,
                None => {
                    eprintln!("Error: Gate '{name}' not found.");
                    return 1;
                }
            };
            println!("Gate: {name}");
            if let Some(ref desc) = gate.description {
                println!("Description: {desc}");
            }
            println!("Validators:");
            for v in &gate.validators {
                let blocking = if v.blocking { "blocking" } else { "non-blocking" };
                let run_if = v
                    .run_if
                    .as_ref()
                    .map(|e| format!(" (run_if: {e})"))
                    .unwrap_or_default();
                let tags = if v.tags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", v.tags.join(", "))
                };
                println!("  - {} ({}, {blocking}){run_if}{tags}", v.name, v.validator_type_str());
            }
        }
        None => {
            println!("Available gates:");
            for (name, gate) in &config.gates {
                let desc = gate
                    .description
                    .as_deref()
                    .unwrap_or("(no description)");
                let count = gate.validators.len();
                println!("  {name} — {desc} ({count} validators)");
            }
        }
    }
    0
}

/// Queries and displays verdict history, optionally filtered by gate, status, or artifact hash.
fn cmd_history(
    config_path: Option<&PathBuf>,
    gate: Option<&str>,
    status: Option<&str>,
    artifact_hash: Option<&str>,
    limit: usize,
) -> i32 {
    let (config, _) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    let conn = match history::init_db(&config.defaults.history_db) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    let results = if let Some(hash) = artifact_hash {
        match history::query_by_artifact(&conn, hash) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: {e}");
                return 2;
            }
        }
    } else {
        match history::query_recent(&conn, limit, gate, status) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: {e}");
                return 2;
            }
        }
    };

    if results.is_empty() {
        println!("No verdicts found.");
        return 0;
    }

    for r in &results {
        let failed_info = r
            .failed_at
            .as_ref()
            .map(|f| format!(" (failed at: {f})"))
            .unwrap_or_default();
        println!(
            "{} {} {} {}ms{}",
            r.timestamp, r.gate, r.status, r.duration_ms, failed_info
        );
    }

    0
}

/// Validates baton.toml and reports any errors or warnings.
fn cmd_validate_config(config_path: Option<&PathBuf>) -> i32 {
    let (config, config_file) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 1;
        }
    };

    let validation = validate_config(&config);

    if validation.errors.is_empty() && validation.warnings.is_empty() {
        eprintln!("Config OK: {}", config_file.display());
        return 0;
    }

    for w in &validation.warnings {
        eprintln!("Warning: {w}");
    }
    for e in &validation.errors {
        eprintln!("Error: {e}");
    }

    if validation.has_errors() { 1 } else { 0 }
}

/// Tests connectivity to a single LLM provider: checks API key, tries `/v1/models`,
/// and falls back to a minimal test completion if the model list is unavailable.
fn check_single_provider(name: &str, provider: &baton::config::Provider) -> bool {
    // 1. Check API key
    let api_key = if provider.api_key_env.is_empty() {
        None
    } else {
        match std::env::var(&provider.api_key_env) {
            Ok(key) => Some(key),
            Err(_) => {
                eprintln!("  ERROR: API key env var '{}' is not set", provider.api_key_env);
                return false;
            }
        }
    };

    // 2. Build HTTP client with short timeout
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  ERROR: Failed to create HTTP client: {e}");
            return false;
        }
    };

    // Build auth headers
    let mut headers = reqwest::header::HeaderMap::new();
    if let Some(ref key) = api_key {
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
    }

    // 3. Try /v1/models endpoint
    let models_url = format!("{}/v1/models", provider.api_base);
    let models_response = client.get(&models_url).headers(headers.clone()).send();

    match models_response {
        Err(e) => {
            if e.is_timeout() {
                eprintln!("  ERROR: Provider '{name}': connection timed out to {}", provider.api_base);
            } else {
                eprintln!("  ERROR: Cannot reach {}: {e}", provider.api_base);
            }
            return false;
        }
        Ok(resp) => {
            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                eprintln!("  ERROR: Authentication failed for provider '{name}'. Check {}.", provider.api_key_env);
                return false;
            }
            if status.is_success() {
                // Parse model list and check for default_model
                let body: serde_json::Value = resp.json().unwrap_or_default();
                let models: Vec<String> = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("id").and_then(|v| v.as_str()))
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .unwrap_or_default();

                if models.iter().any(|m| m == &provider.default_model) {
                    eprintln!("  OK: Provider '{name}': reachable, model '{}' available", provider.default_model);
                    return true;
                } else if models.is_empty() {
                    // Model list came back empty — fall through to test completion
                } else {
                    eprintln!("  WARN: Provider '{name}': reachable, but model '{}' not found", provider.default_model);
                    let display: Vec<&str> = models.iter().take(10).map(|s| s.as_str()).collect();
                    eprintln!("  Available models: {}", display.join(", "));
                    return true; // reachable, just model not found
                }
            }
            // /v1/models not available or empty — try test completion
        }
    }

    // 4. Fallback: minimal test completion
    eprintln!("  WARN: Model list not available. Attempting test completion...");
    let completions_url = format!("{}/v1/chat/completions", provider.api_base);
    let test_body = serde_json::json!({
        "model": provider.default_model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
    });

    let test_client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  ERROR: Failed to create HTTP client: {e}");
            return false;
        }
    };

    match test_client.post(&completions_url).headers(headers).json(&test_body).send() {
        Ok(resp) if resp.status().is_success() => {
            eprintln!("  OK: Provider '{name}': reachable, model '{}' responds", provider.default_model);
            true
        }
        Ok(resp) => {
            eprintln!("  ERROR: Provider '{name}': HTTP {}", resp.status());
            false
        }
        Err(e) => {
            eprintln!("  ERROR: Provider '{name}': test completion failed: {e}");
            false
        }
    }
}

/// Checks connectivity for one or all configured LLM providers.
fn cmd_check_provider(config_path: Option<&PathBuf>, name: Option<&str>, all: bool) -> i32 {
    let (config, _) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    if config.providers.is_empty() {
        eprintln!("No providers configured in baton.toml.");
        return 1;
    }

    let providers_to_check: Vec<(&String, &baton::config::Provider)> = if all {
        config.providers.iter().collect()
    } else if let Some(name) = name {
        match config.providers.get_key_value(name) {
            Some((k, p)) => vec![(k, p)],
            None => {
                let available: Vec<&String> = config.providers.keys().collect();
                eprintln!(
                    "Error: Provider '{name}' not found. Available providers: {}",
                    available.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                );
                return 1;
            }
        }
    } else {
        // Default: check the first provider
        config.providers.iter().take(1).collect()
    };

    let mut any_failed = false;
    for (pname, provider) in &providers_to_check {
        eprintln!("Checking provider '{pname}'...");
        if !check_single_provider(pname, provider) {
            any_failed = true;
        }
    }

    if any_failed { 1 } else { 0 }
}

/// Checks health for one or all configured agent runtimes.
fn cmd_check_runtime(config_path: Option<&PathBuf>, name: Option<&str>, all: bool) -> i32 {
    let (config, _) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    if config.runtimes.is_empty() {
        eprintln!("No runtimes configured in baton.toml.");
        return 1;
    }

    let runtimes_to_check: Vec<(&String, &baton::config::Runtime)> = if all {
        config.runtimes.iter().collect()
    } else if let Some(name) = name {
        match config.runtimes.get_key_value(name) {
            Some((k, r)) => vec![(k, r)],
            None => {
                let available: Vec<&String> = config.runtimes.keys().collect();
                eprintln!(
                    "Error: Runtime '{name}' not found. Available runtimes: {}",
                    available.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                );
                return 1;
            }
        }
    } else {
        // Default: check the first runtime
        config.runtimes.iter().take(1).collect()
    };

    let mut any_failed = false;
    for (rname, runtime_config) in &runtimes_to_check {
        eprintln!("Checking runtime '{rname}'...");

        // Create the adapter
        let adapter = match baton::runtime::create_adapter(rname, runtime_config) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("  ERROR: Failed to create adapter for runtime '{rname}': {e}");
                any_failed = true;
                continue;
            }
        };

        // Run health check
        match adapter.health_check() {
            Ok(health) => {
                if health.reachable {
                    let version_info = health
                        .version
                        .as_ref()
                        .map(|v| format!(", version {v}"))
                        .unwrap_or_default();
                    eprintln!("  OK: Runtime '{rname}': reachable{version_info}");
                } else {
                    let msg = health.message.as_deref().unwrap_or("unknown error");
                    eprintln!("  ERROR: Runtime '{rname}': not reachable ({msg})");
                    any_failed = true;
                }
            }
            Err(e) => {
                eprintln!("  ERROR: Runtime '{rname}': health check failed: {e}");
                any_failed = true;
            }
        }
    }

    if any_failed { 1 } else { 0 }
}

/// Removes stale temporary files (older than 1 hour) from the `.baton/tmp/` directory.
fn cmd_clean(config_path: Option<&PathBuf>, dry_run: bool) -> i32 {
    let (config, _) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    let tmp_dir = &config.defaults.tmp_dir;
    if !tmp_dir.exists() {
        eprintln!("No temporary files to clean.");
        return 0;
    }

    let now = std::time::SystemTime::now();
    let one_hour = std::time::Duration::from_secs(3600);
    let mut cleaned = 0;

    if let Ok(entries) = std::fs::read_dir(tmp_dir) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > one_hour {
                            if dry_run {
                                eprintln!("Would remove: {}", entry.path().display());
                            } else {
                                let _ = std::fs::remove_file(entry.path());
                                eprintln!("Removed: {}", entry.path().display());
                            }
                            cleaned += 1;
                        }
                    }
                }
            }
        }
    }

    if cleaned == 0 {
        eprintln!("No stale files to clean.");
    } else if dry_run {
        eprintln!("{cleaned} file(s) would be removed.");
    } else {
        eprintln!("{cleaned} file(s) removed.");
    }

    0
}

/// Prints baton version, spec version, and config file location.
fn cmd_version(config_path: Option<&PathBuf>) -> i32 {
    println!("baton {}", env!("CARGO_PKG_VERSION"));
    println!("spec version: 0.4");

    match load_config(config_path) {
        Ok((_, path)) => println!("config: {} (found)", path.display()),
        Err(_) => println!("config: not found"),
    }

    0
}

/// Detect how baton was installed based on the current executable path.
/// Returns one of: "cargo", "homebrew", or "binary".
fn detect_install_method() -> (&'static str, PathBuf) {
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize().or(Ok(p)))
        .unwrap_or_else(|_| PathBuf::from("baton"));

    let exe_str = exe.to_string_lossy();

    // Cargo: lives in ~/.cargo/bin/ or $CARGO_HOME/bin/
    let cargo_dir = std::env::var("CARGO_HOME")
        .map(|h| format!("{h}/bin/"))
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            format!("{home}/.cargo/bin/")
        });
    if exe_str.contains(&cargo_dir) {
        return ("cargo", exe);
    }

    // Homebrew: lives under a Homebrew prefix (Cellar, opt, or homebrew bin)
    if exe_str.contains("/Cellar/")
        || exe_str.contains("/homebrew/")
        || exe_str.contains("/opt/homebrew/")
        || exe_str.contains("/usr/local/bin/")
    {
        // Confirm it's actually managed by brew
        if let Ok(output) = std::process::Command::new("brew")
            .args(["list", "baton"])
            .output()
        {
            if output.status.success() {
                return ("homebrew", exe);
            }
        }
    }

    ("binary", exe)
}

/// Downloads and installs a new baton binary from GitHub releases.
/// Detects the install method (cargo/homebrew/binary) and advises accordingly.
fn cmd_update(target_version: Option<String>, skip_confirm: bool) -> i32 {
    let (method, exe_path) = detect_install_method();

    match method {
        "cargo" => {
            eprintln!("This baton was installed via Cargo ({}).", exe_path.display());
            eprintln!("Update it with:");
            eprintln!();
            eprintln!("  cargo install --git https://github.com/apierron/baton.git");
            return 1;
        }
        "homebrew" => {
            eprintln!("This baton was installed via Homebrew ({}).", exe_path.display());
            eprintln!("Update it with:");
            eprintln!();
            eprintln!("  brew upgrade baton");
            return 1;
        }
        _ => {}
    }

    let current_version = env!("CARGO_PKG_VERSION");
    eprintln!("Current version: {current_version}");

    // Fetch release from GitHub API
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("baton/{current_version}"))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: could not create HTTP client: {e}");
            return 1;
        }
    };

    // Normalize the requested version and build the API URL
    let api_url = match &target_version {
        Some(v) => {
            // Ensure the tag has a 'v' prefix for the API lookup
            let tag = if v.starts_with('v') {
                v.clone()
            } else {
                format!("v{v}")
            };
            format!(
                "https://api.github.com/repos/apierron/baton/releases/tags/{tag}"
            )
        }
        None => {
            eprintln!("Checking for updates...");
            "https://api.github.com/repos/apierron/baton/releases/latest".to_string()
        }
    };

    let response = match client.get(&api_url).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: could not reach GitHub: {e}");
            return 1;
        }
    };

    if response.status().as_u16() == 404 {
        if let Some(v) = &target_version {
            eprintln!("Error: version '{v}' not found on GitHub releases.");
        } else {
            eprintln!("Error: no releases found.");
        }
        return 1;
    }

    if !response.status().is_success() {
        eprintln!("Error: GitHub API returned {}", response.status());
        return 1;
    }

    let body: serde_json::Value = match response.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: could not parse release info: {e}");
            return 1;
        }
    };

    let release_tag = match body.get("tag_name").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            eprintln!("Error: could not determine version from release.");
            return 1;
        }
    };

    // Strip leading 'v' if present for version comparison
    let release_version = release_tag.strip_prefix('v').unwrap_or(&release_tag);

    if release_version == current_version && target_version.is_none() {
        eprintln!("Already up to date ({current_version}).");
        return 0;
    }

    if release_version == current_version {
        eprintln!("Version {current_version} is already installed.");
        return 0;
    }

    // Determine target triple
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        eprintln!("Error: unsupported architecture.");
        return 1;
    };

    let (os, ext) = if cfg!(target_os = "macos") {
        ("apple-darwin", "tar.gz")
    } else if cfg!(target_os = "linux") {
        ("unknown-linux-gnu", "tar.gz")
    } else if cfg!(target_os = "windows") {
        ("pc-windows-msvc", "zip")
    } else {
        eprintln!("Error: unsupported operating system.");
        return 1;
    };

    let target = format!("{arch}-{os}");
    let asset_name = format!("baton-{release_tag}-{target}.{ext}");

    // Confirm the asset exists in the release
    let asset_url = body
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a.get("name")?.as_str()?;
                if name == asset_name {
                    a.get("browser_download_url")
                        .and_then(|u| u.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
        });

    let download_url = match asset_url {
        Some(url) => url,
        None => {
            eprintln!(
                "Error: no prebuilt binary found for {target} in release {release_tag}."
            );
            eprintln!("Expected asset: {asset_name}");
            return 1;
        }
    };

    let action = if current_version < release_version {
        "Upgrade"
    } else {
        "Downgrade"
    };
    eprintln!("{action}: {current_version} -> {release_version}");

    if !skip_confirm {
        eprint!("Update? [y/N] ");
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            eprintln!("Error reading input.");
            return 1;
        }
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            eprintln!("Aborted.");
            return 0;
        }
    }

    eprintln!("Downloading {asset_name}...");

    let archive_bytes = match client.get(&download_url).send() {
        Ok(r) if r.status().is_success() => match r.bytes() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error: download failed: {e}");
                return 1;
            }
        },
        Ok(r) => {
            eprintln!("Error: download returned HTTP {}", r.status());
            return 1;
        }
        Err(e) => {
            eprintln!("Error: download failed: {e}");
            return 1;
        }
    };

    // Extract binary from archive into a temp file next to the current exe
    let exe_dir = exe_path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let tmp_path = exe_dir.join(".baton-update.tmp");

    let binary_name = if cfg!(target_os = "windows") {
        "baton.exe"
    } else {
        "baton"
    };

    if ext == "tar.gz" {
        use std::io::Cursor;
        let decoder = flate2::read::GzDecoder::new(Cursor::new(&archive_bytes));
        let mut archive = tar::Archive::new(decoder);
        let mut found = false;
        for entry in archive.entries().unwrap_or_else(|e| {
            eprintln!("Error reading archive: {e}");
            process::exit(1);
        }) {
            let mut entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("Error reading archive entry: {e}");
                    return 1;
                }
            };
            let path = match entry.path() {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };
            if path.file_name().and_then(|f| f.to_str()) == Some(binary_name) {
                if let Err(e) = entry.unpack(&tmp_path) {
                    eprintln!("Error extracting binary: {e}");
                    return 1;
                }
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("Error: '{binary_name}' not found in archive.");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
    } else {
        // zip format (Windows)
        use std::io::Cursor;
        let reader = Cursor::new(&archive_bytes);
        let mut zip = match zip::ZipArchive::new(reader) {
            Ok(z) => z,
            Err(e) => {
                eprintln!("Error reading zip: {e}");
                return 1;
            }
        };
        let mut found = false;
        for i in 0..zip.len() {
            let mut file = match zip.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let name = file.name().to_string();
            if name.ends_with(binary_name) || std::path::Path::new(&name).file_name().and_then(|f| f.to_str()) == Some(binary_name) {
                let mut out = match std::fs::File::create(&tmp_path) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("Error creating temp file: {e}");
                        return 1;
                    }
                };
                if let Err(e) = std::io::copy(&mut file, &mut out) {
                    eprintln!("Error extracting binary: {e}");
                    let _ = std::fs::remove_file(&tmp_path);
                    return 1;
                }
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("Error: '{binary_name}' not found in zip archive.");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
    }

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755));
    }

    // Replace the current binary
    // On Unix: rename is atomic if same filesystem
    // On Windows: rename the old binary out of the way first
    #[cfg(unix)]
    {
        if let Err(e) = std::fs::rename(&tmp_path, &exe_path) {
            eprintln!("Error replacing binary: {e}");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
    }

    #[cfg(windows)]
    {
        let old_path = exe_path.with_extension("exe.old");
        let _ = std::fs::remove_file(&old_path);
        if let Err(e) = std::fs::rename(&exe_path, &old_path) {
            eprintln!("Error moving old binary: {e}");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
        if let Err(e) = std::fs::rename(&tmp_path, &exe_path) {
            eprintln!("Error installing new binary: {e}");
            // Try to restore the old one
            let _ = std::fs::rename(&old_path, &exe_path);
            return 1;
        }
        let _ = std::fs::remove_file(&old_path);
    }

    eprintln!("Updated baton: {current_version} -> {release_version}");
    0
}

/// Removes baton binaries from the system. With `--all`, searches common
/// install locations (cargo, homebrew, install script) and removes all found copies.
fn cmd_uninstall(remove_all: bool, skip_confirm: bool) -> i32 {
    let current_exe = match std::env::current_exe() {
        Ok(p) => match p.canonicalize() {
            Ok(c) => c,
            Err(_) => p,
        },
        Err(e) => {
            eprintln!("Error: could not determine current executable path: {e}");
            return 1;
        }
    };

    // Collect all known baton locations
    let mut targets: Vec<(PathBuf, &str)> = Vec::new();

    // 1. The currently running binary
    targets.push((current_exe.clone(), "current"));

    if remove_all {
        // 2. Default script install location
        let script_dir = std::env::var("BATON_INSTALL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
                PathBuf::from(home).join(".local").join("bin")
            });
        let script_bin = script_dir.join("baton");
        if script_bin.exists() {
            if let Ok(canon) = script_bin.canonicalize() {
                targets.push((canon, "install script"));
            } else {
                targets.push((script_bin, "install script"));
            }
        }

        // 3. Cargo install location
        let cargo_dir = std::env::var("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
                PathBuf::from(home).join(".cargo")
            });
        let cargo_bin = cargo_dir.join("bin").join("baton");
        if cargo_bin.exists() {
            if let Ok(canon) = cargo_bin.canonicalize() {
                targets.push((canon, "cargo"));
            } else {
                targets.push((cargo_bin, "cargo"));
            }
        }

        // 4. Homebrew locations
        for prefix in &["/opt/homebrew/bin/baton", "/usr/local/bin/baton"] {
            let brew_bin = PathBuf::from(prefix);
            if brew_bin.exists() {
                if let Ok(canon) = brew_bin.canonicalize() {
                    targets.push((canon, "homebrew"));
                } else {
                    targets.push((brew_bin, "homebrew"));
                }
            }
        }

        // 5. Windows locations
        #[cfg(target_os = "windows")]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                let win_bin = PathBuf::from(local_app_data).join("baton").join("baton.exe");
                if win_bin.exists() {
                    if let Ok(canon) = win_bin.canonicalize() {
                        targets.push((canon, "install script"));
                    } else {
                        targets.push((win_bin, "install script"));
                    }
                }
            }
        }
    }

    // Deduplicate by canonical path
    targets.sort_by(|a, b| a.0.cmp(&b.0));
    targets.dedup_by(|a, b| a.0 == b.0);

    if targets.is_empty() {
        eprintln!("No baton installations found.");
        return 1;
    }

    // Show what will be removed
    eprintln!("The following baton installation(s) will be removed:");
    for (path, source) in &targets {
        eprintln!("  {} ({})", path.display(), source);
    }

    // Check for homebrew — needs special handling
    let has_homebrew = targets.iter().any(|(_, source)| *source == "homebrew");
    if has_homebrew {
        eprintln!();
        eprintln!("Note: Homebrew installation detected. Run `brew uninstall baton` separately");
        eprintln!("for a clean Homebrew removal. Proceeding here will delete the binary directly.");
    }

    // Confirm
    if !skip_confirm {
        eprintln!();
        eprint!("Continue? [y/N] ");
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            eprintln!("Error reading input.");
            return 1;
        }
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            eprintln!("Aborted.");
            return 0;
        }
    }

    // Remove cargo installation properly if possible
    let has_cargo = targets.iter().any(|(_, source)| *source == "cargo");
    if has_cargo {
        let status = std::process::Command::new("cargo")
            .args(["uninstall", "baton"])
            .status();
        match status {
            Ok(s) if s.success() => {
                eprintln!("Uninstalled baton via cargo.");
                // Remove it from targets so we don't try to delete it again
                targets.retain(|(_, source)| *source != "cargo");
            }
            _ => {
                // cargo uninstall failed — we'll delete the binary directly
            }
        }
    }

    let mut failed = false;

    for (path, source) in &targets {
        // Skip the currently running binary — delete it last via self-delete
        if *path == current_exe {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => eprintln!("Removed {} ({})", path.display(), source),
            Err(e) => {
                eprintln!("Error removing {}: {e}", path.display());
                failed = true;
            }
        }
    }

    // Self-delete: remove the currently running binary
    // On Unix this works because the OS keeps the inode alive until the process exits.
    // On Windows, we rename first then delete.
    let self_is_target = targets.iter().any(|(p, _)| *p == current_exe);
    if self_is_target {
        #[cfg(unix)]
        {
            match std::fs::remove_file(&current_exe) {
                Ok(()) => eprintln!("Removed {} (current)", current_exe.display()),
                Err(e) => {
                    eprintln!("Error removing {}: {e}", current_exe.display());
                    failed = true;
                }
            }
        }
        #[cfg(windows)]
        {
            // On Windows, rename the running binary then delete
            let tmp_path = current_exe.with_extension("exe.old");
            match std::fs::rename(&current_exe, &tmp_path) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&tmp_path);
                    eprintln!("Removed {} (current)", current_exe.display());
                }
                Err(e) => {
                    eprintln!("Error removing {}: {e}", current_exe.display());
                    failed = true;
                }
            }
        }
    }

    if failed {
        eprintln!("Some installations could not be removed.");
        1
    } else {
        eprintln!("baton has been uninstalled.");
        0
    }
}

// Helper trait for display
trait ValidatorTypeStr {
    fn validator_type_str(&self) -> &str;
}

impl ValidatorTypeStr for baton::config::ValidatorConfig {
    fn validator_type_str(&self) -> &str {
        match self.validator_type {
            baton::config::ValidatorType::Script => "script",
            baton::config::ValidatorType::Llm => "llm",
            baton::config::ValidatorType::Human => "human",
        }
    }
}
