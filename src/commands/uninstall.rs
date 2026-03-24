//! Remove baton binaries from the system.
//!
//! With `--all`, searches common install locations (cargo, homebrew, install
//! script) and removes all found copies. Without `--all`, removes only the
//! currently running binary.

/// Removes baton binaries from the system.
pub fn cmd_uninstall(remove_all: bool, skip_confirm: bool) -> i32 {
    let current_exe = match std::env::current_exe() {
        Ok(p) => match p.canonicalize() {
            Ok(c) => c,
            Err(_) => p,
        },
        Err(e) => {
            eprintln!("Error: could not determine current executable path: {e}");
            return 1;
        }
    };

    // Collect all known baton locations
    let mut targets: Vec<(std::path::PathBuf, &str)> = Vec::new();

    // 1. The currently running binary
    targets.push((current_exe.clone(), "current"));

    if remove_all {
        // 2. Default script install location
        let script_dir = std::env::var("BATON_INSTALL_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
                std::path::PathBuf::from(home).join(".local").join("bin")
            });
        let script_bin = script_dir.join("baton");
        if script_bin.exists() {
            if let Ok(canon) = script_bin.canonicalize() {
                targets.push((canon, "install script"));
            } else {
                targets.push((script_bin, "install script"));
            }
        }

        // 3. Cargo install location
        let cargo_dir = std::env::var("CARGO_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
                std::path::PathBuf::from(home).join(".cargo")
            });
        let cargo_bin = cargo_dir.join("bin").join("baton");
        if cargo_bin.exists() {
            if let Ok(canon) = cargo_bin.canonicalize() {
                targets.push((canon, "cargo"));
            } else {
                targets.push((cargo_bin, "cargo"));
            }
        }

        // 4. Homebrew locations
        for prefix in &["/opt/homebrew/bin/baton", "/usr/local/bin/baton"] {
            let brew_bin = std::path::PathBuf::from(prefix);
            if brew_bin.exists() {
                if let Ok(canon) = brew_bin.canonicalize() {
                    targets.push((canon, "homebrew"));
                } else {
                    targets.push((brew_bin, "homebrew"));
                }
            }
        }

        // 5. Windows locations
        #[cfg(target_os = "windows")]
        {
            if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
                let win_bin = std::path::PathBuf::from(local_app_data)
                    .join("baton")
                    .join("baton.exe");
                if win_bin.exists() {
                    if let Ok(canon) = win_bin.canonicalize() {
                        targets.push((canon, "install script"));
                    } else {
                        targets.push((win_bin, "install script"));
                    }
                }
            }
        }
    }

    // Deduplicate by canonical path
    targets.sort_by(|a, b| a.0.cmp(&b.0));
    targets.dedup_by(|a, b| a.0 == b.0);

    if targets.is_empty() {
        eprintln!("No baton installations found.");
        return 1;
    }

    // Show what will be removed
    eprintln!("The following baton installation(s) will be removed:");
    for (path, source) in &targets {
        eprintln!("  {} ({})", path.display(), source);
    }

    // Check for homebrew — needs special handling
    let has_homebrew = targets.iter().any(|(_, source)| *source == "homebrew");
    if has_homebrew {
        eprintln!();
        eprintln!("Note: Homebrew installation detected. Run `brew uninstall baton` separately");
        eprintln!("for a clean Homebrew removal. Proceeding here will delete the binary directly.");
    }

    // Confirm
    if !skip_confirm {
        eprintln!();
        eprint!("Continue? [y/N] ");
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            eprintln!("Error reading input.");
            return 1;
        }
        let input = input.trim().to_lowercase();
        if input != "y" && input != "yes" {
            eprintln!("Aborted.");
            return 0;
        }
    }

    // Remove cargo installation properly if possible
    let has_cargo = targets.iter().any(|(_, source)| *source == "cargo");
    if has_cargo {
        let status = std::process::Command::new("cargo")
            .args(["uninstall", "baton"])
            .status();
        match status {
            Ok(s) if s.success() => {
                eprintln!("Uninstalled baton via cargo.");
                // Remove it from targets so we don't try to delete it again
                targets.retain(|(_, source)| *source != "cargo");
            }
            _ => {
                // cargo uninstall failed — we'll delete the binary directly
            }
        }
    }

    let mut failed = false;

    for (path, source) in &targets {
        // Skip the currently running binary — delete it last via self-delete
        if *path == current_exe {
            continue;
        }
        match std::fs::remove_file(path) {
            Ok(()) => eprintln!("Removed {} ({})", path.display(), source),
            Err(e) => {
                eprintln!("Error removing {}: {e}", path.display());
                failed = true;
            }
        }
    }

    // Self-delete: remove the currently running binary
    // On Unix this works because the OS keeps the inode alive until the process exits.
    // On Windows, we rename first then delete.
    let self_is_target = targets.iter().any(|(p, _)| *p == current_exe);
    if self_is_target {
        #[cfg(unix)]
        {
            match std::fs::remove_file(&current_exe) {
                Ok(()) => eprintln!("Removed {} (current)", current_exe.display()),
                Err(e) => {
                    eprintln!("Error removing {}: {e}", current_exe.display());
                    failed = true;
                }
            }
        }
        #[cfg(windows)]
        {
            // On Windows, rename the running binary then delete
            let tmp_path = current_exe.with_extension("exe.old");
            match std::fs::rename(&current_exe, &tmp_path) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&tmp_path);
                    eprintln!("Removed {} (current)", current_exe.display());
                }
                Err(e) => {
                    eprintln!("Error removing {}: {e}", current_exe.display());
                    failed = true;
                }
            }
        }
    }

    if failed {
        eprintln!("Some installations could not be removed.");
        1
    } else {
        eprintln!("baton has been uninstalled.");
        0
    }
}
