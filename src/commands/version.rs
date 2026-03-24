//! Print baton version and config location.

use std::io::Write;
use std::path::PathBuf;

use super::load_config;

/// Prints baton version, spec version, and config file location.
pub fn cmd_version(config_path: Option<&PathBuf>) -> i32 {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "baton {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(stdout, "spec version: 0.5");

    match load_config(config_path) {
        Ok((_, path)) => {
            let _ = writeln!(stdout, "config: {} (found)", path.display());
        }
        Err(_) => {
            let _ = writeln!(stdout, "config: not found");
        }
    }

    0
}
