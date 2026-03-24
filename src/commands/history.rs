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
    _file: Option<&str>,
    _hash: Option<&str>,
    _invocation: Option<&str>,
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

    // v2: --file, --hash, --invocation filters depend on query_by_file,
    // query_by_hash, query_invocation (not yet implemented in history module)
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
