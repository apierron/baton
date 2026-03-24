//! Check connectivity for configured API (provider) runtimes.

use std::path::PathBuf;

use crate::runtime;

use super::load_config;

/// Checks connectivity for one or all configured API runtimes (formerly providers).
pub fn cmd_check_provider(config_path: Option<&PathBuf>, name: Option<&str>, all: bool) -> i32 {
    let (config, _) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    // Filter for api-type runtimes
    let api_runtimes: Vec<(&String, &crate::config::Runtime)> = config
        .runtimes
        .iter()
        .filter(|(_, r)| r.runtime_type == "api")
        .collect();

    if api_runtimes.is_empty() {
        eprintln!("No API runtimes configured in baton.toml.");
        return 1;
    }

    let runtimes_to_check: Vec<(&String, &crate::config::Runtime)> = if all {
        api_runtimes
    } else if let Some(name) = name {
        match api_runtimes.iter().find(|(k, _)| k.as_str() == name) {
            Some(entry) => vec![*entry],
            None => {
                let available: Vec<&str> = api_runtimes.iter().map(|(k, _)| k.as_str()).collect();
                eprintln!(
                    "Error: API runtime '{name}' not found. Available: {}",
                    available.join(", ")
                );
                return 1;
            }
        }
    } else {
        api_runtimes.into_iter().take(1).collect()
    };

    let mut any_failed = false;
    for (rname, runtime_config) in &runtimes_to_check {
        eprintln!("Checking API runtime '{rname}'...");
        match runtime::create_adapter(rname, runtime_config) {
            Ok(adapter) => match adapter.health_check() {
                Ok(health) if health.reachable => {
                    eprintln!("  OK: Runtime '{rname}': reachable");
                }
                Ok(health) => {
                    eprintln!(
                        "  ERROR: Runtime '{rname}': not reachable: {}",
                        health.message.unwrap_or_default()
                    );
                    any_failed = true;
                }
                Err(e) => {
                    eprintln!("  ERROR: Runtime '{rname}': {e}");
                    any_failed = true;
                }
            },
            Err(e) => {
                eprintln!("  ERROR: Runtime '{rname}': {e}");
                any_failed = true;
            }
        }
    }

    if any_failed {
        1
    } else {
        0
    }
}
