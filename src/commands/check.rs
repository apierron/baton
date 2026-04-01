//! Run validators against input files and output structured results.
//!
//! Loads config, builds the input file pool, filters gates by `--only`/`--skip`,
//! executes each gate, logs verdicts to the history database, and formats output.

use std::io::Write;
use std::path::PathBuf;

use crate::exec::{self, run_gate};
use crate::history;
use crate::types::*;

use super::{load_config, ValidatorTypeStr};

/// Executes the `check` subcommand.
#[allow(clippy::too_many_arguments)]
pub fn cmd_check(
    config_path: Option<&PathBuf>,
    files: &[PathBuf],
    only: Option<Vec<String>>,
    skip: Option<Vec<String>>,
    diff: Option<&str>,
    file_list: Option<&str>,
    timeout: Option<u64>,
    format: &str,
    dry_run: bool,
    no_log: bool,
    suppress_warnings: bool,
    suppress_errors: bool,
    suppress_all: bool,
    no_recursive: bool,
) -> i32 {
    let (config, _config_file) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    // Build input pool
    let collect_opts = exec::FileCollectOptions {
        files: files.to_vec(),
        diff: diff.map(|s| s.to_string()),
        file_list: file_list.map(|s| s.to_string()),
        recursive: !no_recursive,
    };
    let input_pool = match exec::collect_file_pool(&collect_opts) {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    if input_pool.is_empty() && !dry_run {
        eprintln!(
            "Note: No input files provided. \
             Validators that don't require file input will still run."
        );
    }

    // Determine which gates to run
    let mut gate_names: Vec<String> = config.gates.keys().cloned().collect();

    if let Some(ref only_list) = only {
        gate_names.retain(|g| {
            let gate = &config.gates[g];
            exec::gate_matches_only(only_list, g, &gate.validators)
        });
    }
    if let Some(ref skip_list) = skip {
        gate_names.retain(|g| !exec::gate_matches_skip(skip_list, g));
    }

    if gate_names.is_empty() {
        eprintln!("No gates to run.");
        return 0;
    }

    // Dry run
    if dry_run {
        for gate_name in &gate_names {
            let gate = &config.gates[gate_name];
            eprintln!("Gate '{gate_name}':");
            for v in &gate.validators {
                let skip_reason = compute_skip_reason(v, gate_name, &only, &skip);
                match skip_reason {
                    Some(reason) => eprintln!("  \u{2014} {} (skipped by {reason})", v.name),
                    None => {
                        let run_if_note = v
                            .run_if
                            .as_ref()
                            .map(|expr| format!(" (run_if: {expr})"))
                            .unwrap_or_default();
                        eprintln!(
                            "  \u{2713} {} [{}]{run_if_note}",
                            v.name,
                            v.validator_type_str()
                        );
                    }
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
        run_all: false,
        only,
        skip,

        timeout,
        log: !no_log,
        suppressed_statuses,
    };

    // Run each gate and collect the worst exit code
    let mut worst_exit = 0;
    let mut all_verdicts = Vec::new();

    for gate_name in &gate_names {
        let gate = &config.gates[gate_name.as_str()];
        let verdict = match run_gate(gate, &config, input_pool.clone(), &options) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Error: {e}");
                return 2;
            }
        };

        let exit = verdict.status.exit_code();
        if exit > worst_exit {
            worst_exit = exit;
        }

        // Store in history
        if options.log {
            let db_path = &config.defaults.history_db;
            if let Err(e) = std::fs::create_dir_all(
                db_path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(".")),
            ) {
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

        all_verdicts.push(verdict);
    }

    // Output
    let mut stdout = std::io::stdout().lock();
    if all_verdicts.len() == 1 {
        let verdict = &all_verdicts[0];
        match format {
            "json" => {
                let _ = writeln!(stdout, "{}", verdict.to_json());
            }
            "human" => eprintln!("{}", verdict.to_human()),
            "summary" => eprintln!("{}", verdict.to_summary()),
            other => {
                eprintln!("Unknown format: {other}. Using json.");
                let _ = writeln!(stdout, "{}", verdict.to_json());
            }
        }
    } else {
        for verdict in &all_verdicts {
            match format {
                "json" => {
                    let _ = writeln!(stdout, "{}", verdict.to_json());
                }
                "human" => eprintln!("{}", verdict.to_human()),
                "summary" => eprintln!("{}", verdict.to_summary()),
                _ => {
                    let _ = writeln!(stdout, "{}", verdict.to_json());
                }
            }
        }
    }

    worst_exit
}

/// Compute skip reason for a validator based on `--only`/`--skip`.
fn compute_skip_reason(
    v: &crate::config::ValidatorConfig,
    gate_name: &str,
    only: &Option<Vec<String>>,
    skip: &Option<Vec<String>>,
) -> Option<&'static str> {
    if let Some(ref o) = only {
        if !exec::matches_filter(o, gate_name, &v.name, &v.tags) {
            return Some("--only");
        }
    }
    if let Some(ref s) = skip {
        if exec::matches_filter(s, gate_name, &v.name, &v.tags) {
            return Some("--skip");
        }
    }
    None
}
