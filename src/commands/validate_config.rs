//! Validate baton.toml and report errors and warnings.

use std::path::PathBuf;

use crate::config::validate_config;

use super::load_config;

/// Validates baton.toml and reports any errors or warnings.
pub fn cmd_validate_config(config_path: Option<&PathBuf>) -> i32 {
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

    if validation.has_errors() {
        1
    } else {
        0
    }
}
