//! Download and install a new baton binary from GitHub releases.
//!
//! Detects the install method (cargo/homebrew/binary) and advises accordingly.
//! For binary installs, fetches the latest (or specified) release from GitHub,
//! downloads the platform-appropriate archive, and atomically replaces the
//! current executable.

use super::detect_install_method;

/// Downloads and installs a new baton binary from GitHub releases.
pub fn cmd_update(target_version: Option<String>, skip_confirm: bool) -> i32 {
    let (method, exe_path) = detect_install_method();

    match method {
        "cargo" => {
            eprintln!(
                "This baton was installed via Cargo ({}).",
                exe_path.display()
            );
            eprintln!("Update it with:");
            eprintln!();
            eprintln!("  cargo install --git https://github.com/apierron/baton.git");
            return 1;
        }
        "homebrew" => {
            eprintln!(
                "This baton was installed via Homebrew ({}).",
                exe_path.display()
            );
            eprintln!("Update it with:");
            eprintln!();
            eprintln!("  brew upgrade baton");
            return 1;
        }
        _ => {}
    }

    let current_version = env!("CARGO_PKG_VERSION");
    eprintln!("Current version: {current_version}");

    // Fetch release from GitHub API
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(format!("baton/{current_version}"))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: could not create HTTP client: {e}");
            return 1;
        }
    };

    // Normalize the requested version and build the API URL
    let api_url = match &target_version {
        Some(v) => {
            // Ensure the tag has a 'v' prefix for the API lookup
            let tag = if v.starts_with('v') {
                v.clone()
            } else {
                format!("v{v}")
            };
            format!("https://api.github.com/repos/apierron/baton/releases/tags/{tag}")
        }
        None => {
            eprintln!("Checking for updates...");
            "https://api.github.com/repos/apierron/baton/releases/latest".to_string()
        }
    };

    let response = match client.get(&api_url).send() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: could not reach GitHub: {e}");
            return 1;
        }
    };

    if response.status().as_u16() == 404 {
        if let Some(v) = &target_version {
            eprintln!("Error: version '{v}' not found on GitHub releases.");
        } else {
            eprintln!("Error: no releases found.");
        }
        return 1;
    }

    if !response.status().is_success() {
        eprintln!("Error: GitHub API returned {}", response.status());
        return 1;
    }

    let body: serde_json::Value = match response.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: could not parse release info: {e}");
            return 1;
        }
    };

    let release_tag = match body.get("tag_name").and_then(|v| v.as_str()) {
        Some(t) => t.to_string(),
        None => {
            eprintln!("Error: could not determine version from release.");
            return 1;
        }
    };

    // Strip leading 'v' if present for version comparison
    let release_version = release_tag.strip_prefix('v').unwrap_or(&release_tag);

    if release_version == current_version && target_version.is_none() {
        eprintln!("Already up to date ({current_version}).");
        return 0;
    }

    if release_version == current_version {
        eprintln!("Version {current_version} is already installed.");
        return 0;
    }

    // Determine target triple
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        eprintln!("Error: unsupported architecture.");
        return 1;
    };

    let (os, ext) = if cfg!(target_os = "macos") {
        ("apple-darwin", "tar.gz")
    } else if cfg!(target_os = "linux") {
        ("unknown-linux-gnu", "tar.gz")
    } else if cfg!(target_os = "windows") {
        ("pc-windows-msvc", "zip")
    } else {
        eprintln!("Error: unsupported operating system.");
        return 1;
    };

    let target = format!("{arch}-{os}");
    let asset_name = format!("baton-{release_tag}-{target}.{ext}");

    // Confirm the asset exists in the release
    let asset_url = body
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|a| {
                let name = a.get("name")?.as_str()?;
                if name == asset_name {
                    a.get("browser_download_url")
                        .and_then(|u| u.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
        });

    let download_url = match asset_url {
        Some(url) => url,
        None => {
            eprintln!("Error: no prebuilt binary found for {target} in release {release_tag}.");
            eprintln!("Expected asset: {asset_name}");
            return 1;
        }
    };

    let action = if current_version < release_version {
        "Upgrade"
    } else {
        "Downgrade"
    };
    eprintln!("{action}: {current_version} -> {release_version}");

    if !skip_confirm {
        eprint!("Update? [y/N] ");
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

    eprintln!("Downloading {asset_name}...");

    let archive_bytes = match client.get(&download_url).send() {
        Ok(r) if r.status().is_success() => match r.bytes() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Error: download failed: {e}");
                return 1;
            }
        },
        Ok(r) => {
            eprintln!("Error: download returned HTTP {}", r.status());
            return 1;
        }
        Err(e) => {
            eprintln!("Error: download failed: {e}");
            return 1;
        }
    };

    // Extract binary from archive into a temp file next to the current exe
    let exe_dir = exe_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let tmp_path = exe_dir.join(".baton-update.tmp");

    let binary_name = if cfg!(target_os = "windows") {
        "baton.exe"
    } else {
        "baton"
    };

    if ext == "tar.gz" {
        use std::io::Cursor;
        let decoder = flate2::read::GzDecoder::new(Cursor::new(&archive_bytes));
        let mut archive = tar::Archive::new(decoder);
        let mut found = false;
        for entry in archive.entries().unwrap_or_else(|e| {
            eprintln!("Error reading archive: {e}");
            std::process::exit(1);
        }) {
            let mut entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("Error reading archive entry: {e}");
                    return 1;
                }
            };
            let path = match entry.path() {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };
            if path.file_name().and_then(|f| f.to_str()) == Some(binary_name) {
                if let Err(e) = entry.unpack(&tmp_path) {
                    eprintln!("Error extracting binary: {e}");
                    return 1;
                }
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("Error: '{binary_name}' not found in archive.");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
    } else {
        // zip format (Windows)
        use std::io::Cursor;
        let reader = Cursor::new(&archive_bytes);
        let mut zip = match zip::ZipArchive::new(reader) {
            Ok(z) => z,
            Err(e) => {
                eprintln!("Error reading zip: {e}");
                return 1;
            }
        };
        let mut found = false;
        for i in 0..zip.len() {
            let mut file = match zip.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let name = file.name().to_string();
            if name.ends_with(binary_name)
                || std::path::Path::new(&name)
                    .file_name()
                    .and_then(|f| f.to_str())
                    == Some(binary_name)
            {
                let mut out = match std::fs::File::create(&tmp_path) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("Error creating temp file: {e}");
                        return 1;
                    }
                };
                if let Err(e) = std::io::copy(&mut file, &mut out) {
                    eprintln!("Error extracting binary: {e}");
                    let _ = std::fs::remove_file(&tmp_path);
                    return 1;
                }
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("Error: '{binary_name}' not found in zip archive.");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
    }

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(0o755));
    }

    // Replace the current binary
    // On Unix: rename is atomic if same filesystem
    // On Windows: rename the old binary out of the way first
    #[cfg(unix)]
    {
        if let Err(e) = std::fs::rename(&tmp_path, &exe_path) {
            eprintln!("Error replacing binary: {e}");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
    }

    #[cfg(windows)]
    {
        let old_path = exe_path.with_extension("exe.old");
        let _ = std::fs::remove_file(&old_path);
        if let Err(e) = std::fs::rename(&exe_path, &old_path) {
            eprintln!("Error moving old binary: {e}");
            let _ = std::fs::remove_file(&tmp_path);
            return 1;
        }
        if let Err(e) = std::fs::rename(&tmp_path, &exe_path) {
            eprintln!("Error installing new binary: {e}");
            // Try to restore the old one
            let _ = std::fs::rename(&old_path, &exe_path);
            return 1;
        }
        let _ = std::fs::remove_file(&old_path);
    }

    eprintln!("Updated baton: {current_version} -> {release_version}");
    0
}
