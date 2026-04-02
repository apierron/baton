//! Query and display invocation history from the SQLite database.

use std::io::Write;
use std::path::PathBuf;

use crate::history;

use super::load_config;

/// Queries and displays invocation history.
#[allow(clippy::too_many_arguments)]
pub fn cmd_history(
    config_path: Option<&PathBuf>,
    gate: Option<&str>,
    status: Option<&str>,
    file: Option<&str>,
    hash: Option<&str>,
    invocation: Option<&str>,
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

    // Dispatch based on flags: --invocation, --file, --hash, or default (query_recent)
    if let Some(id) = invocation {
        return display_invocation_detail(&conn, id);
    }

    if let Some(path) = file {
        return display_validator_runs(history::query_by_file(&conn, path), "file", path);
    }

    if let Some(h) = hash {
        return display_validator_runs(history::query_by_hash(&conn, h), "hash", h);
    }

    // Default: query recent verdicts
    let results = match history::query_recent(&conn, limit, gate, status) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    if results.is_empty() {
        let _ = writeln!(std::io::stdout(), "No verdicts found.");
        return 0;
    }

    let mut stdout = std::io::stdout().lock();
    for r in &results {
        let failed_info = r
            .failed_at
            .as_ref()
            .map(|f| format!(" (failed at: {f})"))
            .unwrap_or_default();
        let _ = writeln!(
            stdout,
            "{} {} {} {}ms{}",
            r.timestamp, r.gate, r.status, r.duration_ms, failed_info
        );
    }

    0
}

/// Display full invocation detail with gate results and validator runs.
fn display_invocation_detail(conn: &rusqlite::Connection, id: &str) -> i32 {
    let detail = match history::query_invocation(conn, id) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "Invocation: {}", detail.id);
    let _ = writeln!(stdout, "Timestamp:  {}", detail.timestamp);
    let _ = writeln!(stdout, "Version:    {}", detail.baton_version);
    let _ = writeln!(stdout);

    for gr in &detail.gate_results {
        let _ = writeln!(
            stdout,
            "Gate: {} [{}] {}ms  (pass:{} fail:{} warn:{} error:{} skip:{})",
            gr.gate,
            gr.status,
            gr.duration_ms,
            gr.pass_count,
            gr.fail_count,
            gr.warn_count,
            gr.error_count,
            gr.skip_count,
        );
    }

    if !detail.validator_runs.is_empty() {
        let _ = writeln!(stdout);
        let _ = writeln!(stdout, "Validator runs:");
        for vr in &detail.validator_runs {
            let group = vr
                .group_key
                .as_ref()
                .map(|k| format!(" [{k}]"))
                .unwrap_or_default();
            let _ = writeln!(
                stdout,
                "  {} {}{} [{}] {}ms",
                vr.gate, vr.validator, group, vr.status, vr.duration_ms,
            );
            if let Some(ref fb) = vr.feedback {
                for line in fb.lines().take(5) {
                    let _ = writeln!(stdout, "    {line}");
                }
            }
        }
    }

    0
}

/// Display validator run summaries from query_by_file or query_by_hash.
fn display_validator_runs(
    result: crate::error::Result<Vec<history::ValidatorRunSummary>>,
    filter_name: &str,
    filter_value: &str,
) -> i32 {
    let runs = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    if runs.is_empty() {
        let _ = writeln!(
            std::io::stdout(),
            "No validator runs found for {filter_name} '{filter_value}'."
        );
        return 0;
    }

    let mut stdout = std::io::stdout().lock();
    for vr in &runs {
        let group = vr
            .group_key
            .as_ref()
            .map(|k| format!(" [{k}]"))
            .unwrap_or_default();
        let _ = writeln!(
            stdout,
            "{} {} {}{} [{}] {}ms",
            vr.timestamp, vr.gate, vr.validator, group, vr.status, vr.duration_ms,
        );
        if let Some(ref fb) = vr.feedback {
            let first_line = fb.lines().next().unwrap_or("");
            if !first_line.is_empty() {
                let _ = writeln!(stdout, "  {first_line}");
            }
        }
    }

    0
}
