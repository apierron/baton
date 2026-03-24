//! Print baton version and config location.

use std::path::PathBuf;

use super::load_config;

/// Prints baton version, spec version, and config file location.
pub fn cmd_version(config_path: Option<&PathBuf>) -> i32 {
    println!("baton {}", env!("CARGO_PKG_VERSION"));
    println!("spec version: 0.5");

    match load_config(config_path) {
        Ok((_, path)) => println!("config: {} (found)", path.display()),
        Err(_) => println!("config: not found"),
    }

    0
}
