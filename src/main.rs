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
}

fn parse_context_arg(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid context format: '{s}'. Expected name=path"));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

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
    };

    process::exit(exit_code);
}

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

fn cmd_version(config_path: Option<&PathBuf>) -> i32 {
    println!("baton {}", env!("CARGO_PKG_VERSION"));
    println!("spec version: 0.4");

    match load_config(config_path) {
        Ok((_, path)) => println!("config: {} (found)", path.display()),
        Err(_) => println!("config: not found"),
    }

    0
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
