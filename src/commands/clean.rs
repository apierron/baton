//! Remove stale temporary files from `.baton/tmp/`.

use std::path::PathBuf;

use super::load_config;

/// Removes stale temporary files (older than 1 hour) from the `.baton/tmp/` directory.
pub fn cmd_clean(config_path: Option<&PathBuf>, dry_run: bool) -> i32 {
    let (config, _) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            return 2;
        }
    };

    let tmp_dir = &config.defaults.tmp_dir;
    if !tmp_dir.exists() {
        eprintln!("No temporary files to clean.");
        return 0;
    }

    let now = std::time::SystemTime::now();
    let one_hour = std::time::Duration::from_secs(3600);
    let mut cleaned = 0;

    if let Ok(entries) = std::fs::read_dir(tmp_dir) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > one_hour {
                            if dry_run {
                                eprintln!("Would remove: {}", entry.path().display());
                            } else {
                                let _ = std::fs::remove_file(entry.path());
                                eprintln!("Removed: {}", entry.path().display());
                            }
                            cleaned += 1;
                        }
                    }
                }
            }
        }
    }

    if cleaned == 0 {
        eprintln!("No stale files to clean.");
    } else if dry_run {
        eprintln!("{cleaned} file(s) would be removed.");
    } else {
        eprintln!("{cleaned} file(s) removed.");
    }

    0
}
