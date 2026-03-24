//! Input file pool collection from CLI sources.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;

use crate::error::{BatonError, Result};
use crate::types::*;

/// Options for building the input file pool.
pub struct FileCollectOptions {
    pub files: Vec<PathBuf>,
    pub diff: Option<String>,
    pub file_list: Option<String>,
    pub recursive: bool,
}

/// Build the input pool from positional args, `--diff`, and `--files`.
///
/// Directories are walked recursively unless `recursive` is false.
/// The pool is deduplicated by canonical (absolute, symlink-resolved) path.
pub fn collect_file_pool(opts: &FileCollectOptions) -> Result<Vec<InputFile>> {
    let mut pool: Vec<InputFile> = Vec::new();

    // Positional files/directories
    for file_path in &opts.files {
        if !file_path.exists() {
            return Err(BatonError::ValidationError(format!(
                "File not found: {}",
                file_path.display()
            )));
        }
        if file_path.is_dir() {
            if opts.recursive {
                walk_dir(file_path, &mut pool);
            } else {
                // Non-recursive: only direct children that are files
                if let Ok(entries) = std::fs::read_dir(file_path) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if !p.is_dir() {
                            pool.push(InputFile::new(p));
                        }
                    }
                }
            }
        } else {
            pool.push(InputFile::new(file_path.clone()));
        }
    }

    // --diff <refspec>: run git diff --name-only
    if let Some(ref refspec) = opts.diff {
        match Command::new("git")
            .args(["diff", "--name-only", refspec])
            .output()
        {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let p = PathBuf::from(line.trim());
                    if p.exists() {
                        pool.push(InputFile::new(p));
                    }
                }
            }
            Ok(output) => {
                return Err(BatonError::ValidationError(format!(
                    "git diff failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            Err(e) => {
                return Err(BatonError::ValidationError(format!(
                    "could not run git diff: {e}"
                )));
            }
        }
    }

    // --files <path | ->: read newline-separated paths
    if let Some(ref source) = opts.file_list {
        let content = if source == "-" {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        } else {
            std::fs::read_to_string(source).map_err(|e| {
                BatonError::ValidationError(format!("reading file list '{source}': {e}"))
            })?
        };
        for line in content.lines() {
            let line = line.trim();
            if !line.is_empty() {
                pool.push(InputFile::new(PathBuf::from(line)));
            }
        }
    }

    // Deduplicate by canonical path
    let mut seen = HashSet::new();
    pool.retain(|f| {
        if let Ok(canonical) = std::fs::canonicalize(&f.path) {
            seen.insert(canonical)
        } else {
            true // keep files that can't be canonicalized
        }
    });

    Ok(pool)
}

/// Recursively walk a directory, collecting all files.
fn walk_dir(dir: &std::path::Path, pool: &mut Vec<InputFile>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk_dir(&p, pool);
            } else {
                pool.push(InputFile::new(p));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════
    // File collector tests (SPEC-EX-FC-*)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn file_collector_single_file() {
        // SPEC-EX-FC-001: positional file args populate the input pool
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "content").unwrap();

        let opts = FileCollectOptions {
            files: vec![f.path().to_path_buf()],
            diff: None,
            file_list: None,
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(pool[0].path, f.path().to_path_buf());
    }

    #[test]
    fn file_collector_directory_walk() {
        // SPEC-EX-FC-001: directory paths are walked recursively
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.py"), "a").unwrap();
        std::fs::write(dir.path().join("sub/b.py"), "b").unwrap();

        let opts = FileCollectOptions {
            files: vec![dir.path().to_path_buf()],
            diff: None,
            file_list: None,
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn file_collector_deduplication() {
        // SPEC-EX-FC-004: pool is deduplicated by canonical path
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.py");
        std::fs::write(&file_path, "content").unwrap();

        let canonical = std::fs::canonicalize(&file_path).unwrap();

        let opts = FileCollectOptions {
            files: vec![file_path, canonical],
            diff: None,
            file_list: None,
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn file_collector_reads_file_list() {
        // SPEC-EX-FC-003: --files reads newline-separated paths
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.py");
        let b = dir.path().join("b.py");
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();

        let list_file = dir.path().join("filelist.txt");
        std::fs::write(&list_file, format!("{}\n{}\n", a.display(), b.display())).unwrap();

        let opts = FileCollectOptions {
            files: vec![],
            diff: None,
            file_list: Some(list_file.display().to_string()),
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn file_collector_no_recursive() {
        // SPEC-EX-FC-005: --no-recursive disables recursive directory walking
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("a.py"), "a").unwrap();
        std::fs::write(dir.path().join("sub/b.py"), "b").unwrap();

        let opts = FileCollectOptions {
            files: vec![dir.path().to_path_buf()],
            diff: None,
            file_list: None,
            recursive: false,
        };
        let pool = collect_file_pool(&opts).unwrap();
        // Only the top-level file, not the one in sub/
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn file_collector_not_found_errors() {
        let opts = FileCollectOptions {
            files: vec![std::path::PathBuf::from("/nonexistent/file.py")],
            diff: None,
            file_list: None,
            recursive: true,
        };
        assert!(collect_file_pool(&opts).is_err());
    }

    #[test]
    fn file_collector_empty_lines_skipped() {
        // Empty lines in file list should be skipped
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let a = dir.path().join("a.py");
        std::fs::write(&a, "a").unwrap();

        let list_file = dir.path().join("filelist.txt");
        std::fs::write(&list_file, format!("\n{}\n\n", a.display())).unwrap();

        let opts = FileCollectOptions {
            files: vec![],
            diff: None,
            file_list: Some(list_file.display().to_string()),
            recursive: true,
        };
        let pool = collect_file_pool(&opts).unwrap();
        assert_eq!(pool.len(), 1);
    }
}
