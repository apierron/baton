//! CLI entry point for baton.
//!
//! Defines the CLI grammar via clap and dispatches each subcommand to its
//! handler in [`commands`]. All domain logic lives in the library crates
//! (`baton::exec`, `baton::config`, etc.) or in the per-command modules
//! under `commands/`.
//!
//! Exit code conventions: 0 = success or passing verdict, 1 = user-recoverable
//! error or failing verdict, 2 = infrastructure/config error.

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

use baton::commands;

#[derive(Parser)]
#[command(name = "baton", version = env!("CARGO_PKG_VERSION"), about = "A composable validation gate for AI agent outputs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run validators against input files
    Check {
        /// Input files and directories (walked recursively)
        files: Vec<PathBuf>,

        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,

        /// Only run matching gates/validators (gate, gate.validator, @tag)
        #[arg(long, value_delimiter = ' ')]
        only: Option<Vec<String>>,

        /// Skip matching gates/validators (gate, gate.validator, @tag)
        #[arg(long, value_delimiter = ' ')]
        skip: Option<Vec<String>>,

        /// Add git-changed files to the input pool
        #[arg(long)]
        diff: Option<String>,

        /// Read newline-separated file paths from a file or stdin (use '-' for stdin)
        #[arg(long = "files")]
        file_list: Option<String>,

        /// Override default timeout for all validators
        #[arg(long)]
        timeout: Option<u64>,

        /// Output format
        #[arg(long, default_value = "json")]
        format: String,

        /// Print invocation plan and exit
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

        /// Suppress warnings, errors, and failures
        #[arg(long)]
        suppress_all: bool,

        /// Disable recursive directory walking for positional args
        #[arg(long)]
        no_recursive: bool,
    },

    /// Initialize a new baton project
    Init {
        /// Only create baton.toml and .baton/ directory
        #[arg(long)]
        minimal: bool,

        /// Only create the prompts/ directory with starter templates
        #[arg(long)]
        prompts_only: bool,

        /// Language profile for starter config (rust, python, generic)
        #[arg(long)]
        profile: Option<String>,
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

    /// Query invocation history
    History {
        /// Filter by gate name
        #[arg(long)]
        gate: Option<String>,

        /// Filter by status
        #[arg(long)]
        status: Option<String>,

        /// Search validator runs by file path
        #[arg(long)]
        file: Option<String>,

        /// Search validator runs by content hash
        #[arg(long)]
        hash: Option<String>,

        /// Show detail for a specific invocation
        #[arg(long)]
        invocation: Option<String>,

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

    /// Add a validator to baton.toml (interactive or via flags)
    Add {
        /// Validator name
        #[arg(long)]
        name: Option<String>,

        /// Validator type: script, llm, or human
        #[arg(long = "type")]
        validator_type: Option<String>,

        /// Script command
        #[arg(long)]
        command: Option<String>,

        /// LLM/human prompt text
        #[arg(long)]
        prompt: Option<String>,

        /// Runtime name for LLM validators
        #[arg(long)]
        runtime: Option<String>,

        /// Model override for LLM validators
        #[arg(long)]
        model: Option<String>,

        /// Add to this gate (existing or new)
        #[arg(long)]
        gate: Option<String>,

        /// Whether the validator is blocking in the gate
        #[arg(long)]
        blocking: Option<bool>,

        /// Tags to apply
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,

        /// Input glob pattern
        #[arg(long)]
        input: Option<String>,

        /// Timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,

        /// Import from file, URL, or registry
        #[arg(long)]
        from: Option<String>,

        /// Path to baton.toml
        #[arg(long)]
        config: Option<PathBuf>,

        /// Preview changes without writing
        #[arg(long)]
        dry_run: bool,

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

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Check {
            config,
            files,
            only,
            skip,
            diff,
            file_list,
            timeout,
            format,
            dry_run,
            no_log,
            verbose: _,
            suppress_warnings,
            suppress_errors,
            suppress_all,
            no_recursive,
        } => commands::check::cmd_check(
            config.as_ref(),
            &files,
            only,
            skip,
            diff.as_deref(),
            file_list.as_deref(),
            timeout,
            &format,
            dry_run,
            no_log,
            suppress_warnings,
            suppress_errors,
            suppress_all,
            no_recursive,
        ),
        Commands::Init {
            minimal,
            prompts_only,
            profile,
        } => commands::init::cmd_init(minimal, prompts_only, profile.as_deref()),
        Commands::List { gate, config } => {
            commands::list::cmd_list(config.as_ref(), gate.as_deref())
        }
        Commands::History {
            gate,
            status,
            file,
            hash,
            invocation,
            limit,
            config,
        } => commands::history::cmd_history(
            config.as_ref(),
            gate.as_deref(),
            status.as_deref(),
            file.as_deref(),
            hash.as_deref(),
            invocation.as_deref(),
            limit,
        ),
        Commands::ValidateConfig { config } => {
            commands::validate_config::cmd_validate_config(config.as_ref())
        }
        Commands::CheckProvider { name, all, config } => {
            commands::check_provider::cmd_check_provider(config.as_ref(), name.as_deref(), all)
        }
        Commands::CheckRuntime { name, all, config } => {
            commands::check_runtime::cmd_check_runtime(config.as_ref(), name.as_deref(), all)
        }
        Commands::Clean { dry_run, config } => commands::clean::cmd_clean(config.as_ref(), dry_run),
        Commands::Version { config } => commands::version::cmd_version(config.as_ref()),
        Commands::Add {
            name,
            validator_type,
            command,
            prompt,
            runtime,
            model,
            gate,
            blocking,
            tags,
            input,
            timeout,
            from,
            config,
            dry_run,
            yes,
        } => commands::add::cmd_add(commands::add::AddOptions {
            name,
            validator_type,
            command,
            prompt,
            runtime,
            model,
            gate,
            blocking,
            tags,
            input,
            timeout,
            from,
            config,
            dry_run,
            yes,
        }),
        Commands::Update { version, yes } => commands::update::cmd_update(version, yes),
        Commands::Uninstall { all, yes } => commands::uninstall::cmd_uninstall(all, yes),
    };

    process::exit(exit_code);
}
