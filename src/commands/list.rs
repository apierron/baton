//! List configured gates and their validators.

use std::path::PathBuf;

use super::{load_config, ValidatorTypeStr};

/// Lists available gates, or shows validators for a specific gate.
pub fn cmd_list(config_path: Option<&PathBuf>, gate_name: Option<&str>) -> i32 {
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
                let blocking = if v.blocking {
                    "blocking"
                } else {
                    "non-blocking"
                };
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
                println!(
                    "  - {} ({}, {blocking}){run_if}{tags}",
                    v.name,
                    v.validator_type_str()
                );
            }
        }
        None => {
            println!("Available gates:");
            for (name, gate) in &config.gates {
                let desc = gate.description.as_deref().unwrap_or("(no description)");
                let count = gate.validators.len();
                println!("  {name} — {desc} ({count} validators)");
            }
        }
    }
    0
}
