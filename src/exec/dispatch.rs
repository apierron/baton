//! Dispatch planner: maps validators + file pools to concrete [`Invocation`]s.
//!
//! Implements the four input declaration modes — `None` (single call, no files),
//! `PerFile` (one call per matching file), `Batch` (one call with all matches),
//! and `Named` (group files by a key expression such as `{stem}`).
//! Returns `(invocations, warnings)`; empty invocations + warnings means skip.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use crate::config::{InputDecl, ValidatorConfig};
use crate::types::*;

/// Match a filename against a glob pattern (e.g. "*.py", "src/**/*.rs").
fn glob_matches(pattern: &str, path: &std::path::Path) -> bool {
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    let path_str = path.to_string_lossy();
    // Try matching against full path first, then filename only
    glob_match::glob_match(pattern, &path_str) || glob_match::glob_match(pattern, &filename)
}

/// Extract a key value from a file path based on a key expression.
///
/// Supported expressions: `{stem}`, `{name}`, `{ext}`.
fn extract_key(expr: &str, path: &std::path::Path) -> Option<String> {
    match expr {
        "{stem}" => path.file_stem().map(|s| s.to_string_lossy().to_string()),
        "{name}" => path.file_name().map(|s| s.to_string_lossy().to_string()),
        "{ext}" => path.extension().map(|s| s.to_string_lossy().to_string()),
        _ => Option::None,
    }
}

/// Plan dispatch: turn a validator's input declaration + file pool into invocations.
///
/// Returns `(invocations, warnings)`. An empty invocations vec with warnings
/// means the validator should be skipped.
pub fn plan_dispatch(
    validator: &ValidatorConfig,
    pool: &[InputFile],
) -> (Vec<Invocation>, Vec<String>) {
    let mut warnings = Vec::new();

    match &validator.input {
        InputDecl::None => {
            // DP-001: single invocation, no files
            let inv = Invocation {
                validator_name: validator.name.clone(),
                group_key: Option::None,
                inputs: BTreeMap::new(),
            };
            (vec![inv], warnings)
        }
        InputDecl::PerFile { pattern } => {
            // DP-002: one invocation per matching file
            let matching: Vec<&InputFile> = pool
                .iter()
                .filter(|f| glob_matches(pattern, &f.path))
                .collect();

            if matching.is_empty() {
                // DP-007
                warnings.push(format!(
                    "Validator '{}': no files match pattern '{}'",
                    validator.name, pattern
                ));
                return (vec![], warnings);
            }

            let invocations = matching
                .into_iter()
                .map(|f| {
                    let mut inputs = BTreeMap::new();
                    inputs.insert("file".to_string(), vec![f.clone()]);
                    Invocation {
                        validator_name: validator.name.clone(),
                        group_key: Option::None,
                        inputs,
                    }
                })
                .collect();
            (invocations, warnings)
        }
        InputDecl::Batch { pattern } => {
            // DP-003: single invocation with all matching files
            let matching: Vec<InputFile> = pool
                .iter()
                .filter(|f| glob_matches(pattern, &f.path))
                .cloned()
                .collect();

            if matching.is_empty() {
                warnings.push(format!(
                    "Validator '{}': no files match pattern '{}'",
                    validator.name, pattern
                ));
                return (vec![], warnings);
            }

            let mut inputs = BTreeMap::new();
            inputs.insert("file".to_string(), matching);
            let inv = Invocation {
                validator_name: validator.name.clone(),
                group_key: Option::None,
                inputs,
            };
            (vec![inv], warnings)
        }
        InputDecl::Named(slots) => {
            // Separate fixed inputs from glob-matched inputs
            let mut fixed_inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
            let mut keyed_slots: Vec<(&String, &crate::config::InputSlot)> = Vec::new();
            let mut unkeyed_slots: Vec<(&String, &crate::config::InputSlot)> = Vec::new();

            for (slot_name, slot) in slots {
                if let Some(ref path) = slot.path {
                    // DP-006: fixed input
                    fixed_inputs
                        .insert(slot_name.clone(), vec![InputFile::new(PathBuf::from(path))]);
                } else if slot.key.is_some() {
                    keyed_slots.push((slot_name, slot));
                } else {
                    unkeyed_slots.push((slot_name, slot));
                }
            }

            if keyed_slots.is_empty() && unkeyed_slots.is_empty() {
                // Only fixed inputs — single invocation
                let inv = Invocation {
                    validator_name: validator.name.clone(),
                    group_key: Option::None,
                    inputs: fixed_inputs,
                };
                return (vec![inv], warnings);
            }

            if !keyed_slots.is_empty() {
                // DP-004: group by key
                // For each keyed slot, match files and extract keys
                let mut slot_keys: BTreeMap<String, BTreeMap<String, Vec<InputFile>>> =
                    BTreeMap::new(); // key_value -> slot_name -> files

                for (slot_name, slot) in &keyed_slots {
                    let pattern = slot.match_pattern.as_deref().unwrap_or("*");
                    let key_expr = slot.key.as_deref().unwrap();

                    for file in pool {
                        if glob_matches(pattern, &file.path) {
                            if let Some(key_val) = extract_key(key_expr, &file.path) {
                                slot_keys
                                    .entry(key_val)
                                    .or_default()
                                    .entry((*slot_name).clone())
                                    .or_default()
                                    .push(file.clone());
                            }
                        }
                    }
                }

                // Collect all key values
                let all_keys: HashSet<String> = slot_keys.keys().cloned().collect();
                let slot_names: Vec<&String> = keyed_slots.iter().map(|(name, _)| *name).collect();

                let mut invocations = Vec::new();
                for key_val in &all_keys {
                    // DP-005: check completeness
                    let group = slot_keys.get(key_val);
                    let complete = slot_names
                        .iter()
                        .all(|name| group.map(|g| g.contains_key(*name)).unwrap_or(false));

                    if !complete {
                        warnings.push(format!(
                            "Validator '{}': incomplete group for key '{}', skipping",
                            validator.name, key_val
                        ));
                        continue;
                    }

                    let mut inputs = fixed_inputs.clone();
                    if let Some(group) = group {
                        for (slot_name, files) in group {
                            inputs.insert(slot_name.clone(), files.clone());
                        }
                    }

                    // Also add unkeyed slots
                    for (slot_name, slot) in &unkeyed_slots {
                        let pattern = slot.match_pattern.as_deref().unwrap_or("*");
                        let matching: Vec<InputFile> = pool
                            .iter()
                            .filter(|f| glob_matches(pattern, &f.path))
                            .cloned()
                            .collect();
                        if !matching.is_empty() {
                            if slot.collect {
                                inputs.insert((*slot_name).clone(), matching);
                            } else if let Some(first) = matching.into_iter().next() {
                                inputs.insert((*slot_name).clone(), vec![first]);
                            }
                        }
                    }

                    invocations.push(Invocation {
                        validator_name: validator.name.clone(),
                        group_key: Some(key_val.clone()),
                        inputs,
                    });
                }

                if invocations.is_empty() && !all_keys.is_empty() {
                    warnings.push(format!(
                        "Validator '{}': all groups incomplete",
                        validator.name
                    ));
                }
                if all_keys.is_empty() {
                    warnings.push(format!(
                        "Validator '{}': no files match any keyed input patterns",
                        validator.name
                    ));
                }

                return (invocations, warnings);
            }

            // Only unkeyed named slots — single invocation
            let mut inputs = fixed_inputs;
            for (slot_name, slot) in &unkeyed_slots {
                let pattern = slot.match_pattern.as_deref().unwrap_or("*");
                let matching: Vec<InputFile> = pool
                    .iter()
                    .filter(|f| glob_matches(pattern, &f.path))
                    .cloned()
                    .collect();
                if slot.collect {
                    inputs.insert((*slot_name).clone(), matching);
                } else if let Some(first) = matching.into_iter().next() {
                    inputs.insert((*slot_name).clone(), vec![first]);
                }
            }

            let inv = Invocation {
                validator_name: validator.name.clone(),
                group_key: Option::None,
                inputs,
            };
            (vec![inv], warnings)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;
    use crate::test_helpers as th;

    fn validator_with_input(name: &str, input: crate::config::InputDecl) -> ValidatorConfig {
        ValidatorConfig {
            name: name.into(),
            input,
            ..th::ValidatorBuilder::script(name, "echo ok").build()
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // Dispatch planner tests (SPEC-EX-DP-*)
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn dispatch_no_input_produces_single_invocation() {
        // SPEC-EX-DP-001: validator with no input field produces one invocation
        let v = validator_with_input("lint", crate::config::InputDecl::None);
        let pool: Vec<InputFile> = vec![];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1);
        assert!(invocations[0].inputs.is_empty());
        assert!(invocations[0].group_key.is_none());
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_per_file_produces_one_per_match() {
        // SPEC-EX-DP-002: per-file input produces one invocation per matching file
        let v = validator_with_input(
            "lint",
            crate::config::InputDecl::PerFile {
                pattern: "*.py".into(),
            },
        );
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/c.rs")),
        ];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 2); // a.py and b.py match, c.rs doesn't
        assert_eq!(
            invocations[0].inputs["file"][0].path,
            std::path::PathBuf::from("/tmp/a.py")
        );
        assert_eq!(
            invocations[1].inputs["file"][0].path,
            std::path::PathBuf::from("/tmp/b.py")
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_batch_produces_single_invocation() {
        // SPEC-EX-DP-003: batch input (collect = true) produces one invocation
        let v = validator_with_input(
            "batch-lint",
            crate::config::InputDecl::Batch {
                pattern: "*.py".into(),
            },
        );
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
        ];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].inputs["file"].len(), 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_keyed_inputs_grouped_by_key() {
        // SPEC-EX-DP-004: named inputs with key expressions grouped by key value
        use crate::config::InputSlot;

        let mut slots = BTreeMap::new();
        slots.insert(
            "code".into(),
            InputSlot {
                match_pattern: Some("*.py".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );
        slots.insert(
            "spec".into(),
            InputSlot {
                match_pattern: Some("*.md".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );

        let v = validator_with_input("check", crate::config::InputDecl::Named(slots));
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/a.md")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.md")),
        ];

        let (mut invocations, warnings) = plan_dispatch(&v, &pool);
        invocations.sort_by(|a, b| a.group_key.cmp(&b.group_key));

        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].group_key, Some("a".into()));
        assert_eq!(invocations[1].group_key, Some("b".into()));
        assert_eq!(invocations[0].inputs.len(), 2); // code + spec
        assert!(warnings.is_empty());
    }

    #[test]
    fn dispatch_incomplete_group_skips() {
        // SPEC-EX-DP-005: incomplete group skipped with warning
        use crate::config::InputSlot;

        let mut slots = BTreeMap::new();
        slots.insert(
            "code".into(),
            InputSlot {
                match_pattern: Some("*.py".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );
        slots.insert(
            "spec".into(),
            InputSlot {
                match_pattern: Some("*.md".into()),
                path: None,
                key: Some("{stem}".into()),
                collect: false,
            },
        );

        let v = validator_with_input("check", crate::config::InputDecl::Named(slots));
        // a has both code and spec, b only has code (no b.md)
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            InputFile::new(std::path::PathBuf::from("/tmp/a.md")),
        ];

        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1); // only "a" group is complete
        assert_eq!(invocations[0].group_key, Some("a".into()));
        assert!(!warnings.is_empty()); // warning about incomplete "b" group
        assert!(warnings
            .iter()
            .any(|w| w.contains("incomplete") && w.contains("b")));
    }

    #[test]
    fn dispatch_fixed_input_injected() {
        // SPEC-EX-DP-006: fixed inputs injected into every invocation
        use crate::config::InputSlot;

        let mut slots = BTreeMap::new();
        slots.insert(
            "config".into(),
            InputSlot {
                match_pattern: None,
                path: Some("/etc/config.toml".into()),
                key: None,
                collect: false,
            },
        );

        let v = validator_with_input("check", crate::config::InputDecl::Named(slots));
        let pool: Vec<InputFile> = vec![];

        let (invocations, _) = plan_dispatch(&v, &pool);
        assert_eq!(invocations.len(), 1);
        assert!(invocations[0].inputs.contains_key("config"));
        assert_eq!(
            invocations[0].inputs["config"][0].path,
            std::path::PathBuf::from("/etc/config.toml")
        );
    }

    #[test]
    fn dispatch_no_matching_files_produces_empty() {
        // SPEC-EX-DP-007: no matching files means validator is skipped
        let v = validator_with_input(
            "lint",
            crate::config::InputDecl::PerFile {
                pattern: "*.py".into(),
            },
        );
        let pool = vec![
            InputFile::new(std::path::PathBuf::from("/tmp/readme.md")),
            InputFile::new(std::path::PathBuf::from("/tmp/notes.txt")),
        ];
        let (invocations, warnings) = plan_dispatch(&v, &pool);
        assert!(invocations.is_empty());
        assert!(!warnings.is_empty());
    }
}
