//! Check health for configured agent runtimes.

use std::path::PathBuf;

use super::load_config;

/// Checks health for one or all configured agent runtimes.
pub fn cmd_check_runtime(config_path: Option<&PathBuf>, name: Option<&str>, all: bool) -> i32 {
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

    let runtimes_to_check: Vec<(&String, &crate::config::Runtime)> = if all {
        config.runtimes.iter().collect()
    } else if let Some(name) = name {
        match config.runtimes.get_key_value(name) {
            Some((k, r)) => vec![(k, r)],
            None => {
                let available: Vec<&String> = config.runtimes.keys().collect();
                eprintln!(
                    "Error: Runtime '{name}' not found. Available runtimes: {}",
                    available
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
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

        let adapter = match crate::runtime::create_adapter(rname, runtime_config) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("  ERROR: Failed to create adapter for runtime '{rname}': {e}");
                any_failed = true;
                continue;
            }
        };

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

    if any_failed {
        1
    } else {
        0
    }
}
