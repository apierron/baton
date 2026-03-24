//! CLI command handlers. Each submodule implements one top-level baton subcommand.

pub mod add;
pub mod check;
pub mod check_provider;
pub mod check_runtime;
pub mod clean;
pub mod history;
pub mod init;
pub mod list;
pub mod uninstall;
pub mod update;
pub mod validate_config;
pub mod version;

use std::path::{Path, PathBuf};

use baton::config::{discover_config, parse_config};

// ─── Shared helpers ───────────────────────────────────────

/// Loads and parses baton.toml from an explicit path or by discovery.
pub(crate) fn load_config(
    config_path: Option<&PathBuf>,
) -> baton::error::Result<(baton::config::BatonConfig, PathBuf)> {
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

    let config_dir = config_file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let toml_str = std::fs::read_to_string(&config_file)?;
    let config = parse_config(&toml_str, &config_dir)?;
    Ok((config, config_file))
}

/// Helper trait for displaying validator type as a string.
pub(crate) trait ValidatorTypeStr {
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

/// Detect how baton was installed based on the current executable path.
/// Returns one of: "cargo", "homebrew", or "binary".
pub(crate) fn detect_install_method() -> (&'static str, PathBuf) {
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
