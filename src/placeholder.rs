//! Template placeholder resolution.
//!
//! Substitutes `{file}`, `{file.path}`, `{file.dir}`, `{file.name}`, `{file.stem}`,
//! `{file.ext}`, `{file.content}`, `{input.<name>}`, `{input.<name>.path}`,
//! `{input.<name>.content}`, `{verdict.<name>.status}`, `{verdict.<name>.feedback}`,
//! and similar placeholders in command strings and prompt templates.

use crate::types::{InputFile, ValidatorResult};
use std::collections::BTreeMap;

/// Warnings emitted during placeholder resolution.
#[derive(Debug, Clone, Default)]
pub struct ResolutionWarnings {
    pub warnings: Vec<String>,
}

impl ResolutionWarnings {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Resolve placeholders in a template string.
///
/// Supported placeholders:
/// - `{file}` — content of first file in the "file" input slot (or first file in any slot)
/// - `{file.path}` — absolute path to the first input file
/// - `{file.dir}` — parent directory of the first input file
/// - `{file.name}` — filename with extension
/// - `{file.stem}` — filename without extension
/// - `{file.ext}` — extension without dot
/// - `{file.content}` — alias for `{file}`
/// - `{input.<name>}` — content of first file in named input slot
/// - `{input.<name>.path}` — absolute path of first file in named slot
/// - `{input.<name>.name}` — filename of first file in named slot
/// - `{input.<name>.stem}` — stem of first file in named slot
/// - `{input.<name>.content}` — content of first file in named slot
/// - `{input.<name>.paths}` — space-separated paths for named slot (multiple files)
/// - `{input}` / `{input.content}` — concatenated content of all files (batch mode)
/// - `{input.paths}` — space-separated paths of all files (batch mode)
/// - `{verdict.<validator_name>.status}` — status of a prior validator
/// - `{verdict.<validator_name>.feedback}` — feedback from a prior validator
pub fn resolve_placeholders(
    template: &str,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
    prior_results: &BTreeMap<String, ValidatorResult>,
    warnings: &mut ResolutionWarnings,
) -> String {
    let mut result = String::with_capacity(template.len());
    let chars: Vec<char> = template.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '{' {
            // Find matching closing brace
            if let Some(close) = find_closing_brace(&chars, i) {
                let placeholder: String = chars[i + 1..close].iter().collect();
                let resolved = resolve_single(&placeholder, inputs, prior_results, warnings);
                result.push_str(&resolved);
                i = close + 1;
            } else {
                // No closing brace — leave as literal
                result.push('{');
                i += 1;
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn find_closing_brace(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (j, &ch) in chars.iter().enumerate().skip(open) {
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
    None
}

/// Get the key of the slot containing the first InputFile: prefer "file", then first available.
fn first_file_key(inputs: &BTreeMap<String, Vec<InputFile>>) -> Option<String> {
    if inputs.get("file").is_some_and(|v| !v.is_empty()) {
        return Some("file".to_string());
    }
    for (key, files) in inputs {
        if !files.is_empty() {
            return Some(key.clone());
        }
    }
    None
}

fn resolve_single(
    placeholder: &str,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
    prior_results: &BTreeMap<String, ValidatorResult>,
    warnings: &mut ResolutionWarnings,
) -> String {
    // {file} — content of first file
    if placeholder == "file" || placeholder == "file.content" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(f) = inputs.get_mut(&key).and_then(|v| v.first_mut()) {
                return f.get_content().unwrap_or("").to_string();
            }
        }
        return String::new();
    }

    // {file.path}
    if placeholder == "file.path" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(f) = inputs.get(&key).and_then(|v| v.first()) {
                return f.path.display().to_string();
            }
        }
        return String::new();
    }

    // {file.dir}
    if placeholder == "file.dir" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(f) = inputs.get(&key).and_then(|v| v.first()) {
                return f
                    .path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
            }
        }
        return String::new();
    }

    // {file.name}
    if placeholder == "file.name" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(f) = inputs.get(&key).and_then(|v| v.first()) {
                return f
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
            }
        }
        return String::new();
    }

    // {file.stem}
    if placeholder == "file.stem" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(f) = inputs.get(&key).and_then(|v| v.first()) {
                return f
                    .path
                    .file_stem()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
            }
        }
        return String::new();
    }

    // {file.ext}
    if placeholder == "file.ext" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(f) = inputs.get(&key).and_then(|v| v.first()) {
                return f
                    .path
                    .extension()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
            }
        }
        return String::new();
    }

    // {input} or {input.content} — concatenated content of all files in the primary slot (batch mode)
    if placeholder == "input" || placeholder == "input.content" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(files) = inputs.get_mut(&key) {
                let mut parts = Vec::new();
                for f in files.iter_mut() {
                    if let Ok(content) = f.get_content() {
                        parts.push(content.to_string());
                    }
                }
                return parts.join("\n");
            }
        }
        return String::new();
    }

    // {input.paths} — space-separated absolute paths of all files in the primary slot
    if placeholder == "input.paths" {
        if let Some(key) = first_file_key(inputs) {
            if let Some(files) = inputs.get(&key) {
                return files
                    .iter()
                    .map(|f| f.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
        }
        return String::new();
    }

    // {input.<name>} or {input.<name>.<prop>}
    if let Some(rest) = placeholder.strip_prefix("input.") {
        // Check for .paths (plural), .path, .name, .stem, .content suffixes
        if let Some(name) = rest.strip_suffix(".paths") {
            if let Some(files) = inputs.get(name) {
                return files
                    .iter()
                    .map(|f| f.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            warnings.warnings.push(format!(
                "Placeholder '{{input.{name}.paths}}' references undefined input '{name}'"
            ));
            return String::new();
        }
        if let Some(name) = rest.strip_suffix(".path") {
            if let Some(files) = inputs.get(name) {
                if let Some(f) = files.first() {
                    return f.path.display().to_string();
                }
            }
            warnings.warnings.push(format!(
                "Placeholder '{{input.{name}.path}}' references undefined input '{name}'"
            ));
            return String::new();
        }
        if let Some(name) = rest.strip_suffix(".name") {
            if let Some(files) = inputs.get(name) {
                if let Some(f) = files.first() {
                    return f
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                }
            }
            warnings.warnings.push(format!(
                "Placeholder '{{input.{name}.name}}' references undefined input '{name}'"
            ));
            return String::new();
        }
        if let Some(name) = rest.strip_suffix(".stem") {
            if let Some(files) = inputs.get(name) {
                if let Some(f) = files.first() {
                    return f
                        .path
                        .file_stem()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                }
            }
            warnings.warnings.push(format!(
                "Placeholder '{{input.{name}.stem}}' references undefined input '{name}'"
            ));
            return String::new();
        }
        if let Some(name) = rest.strip_suffix(".content") {
            if let Some(files) = inputs.get_mut(name) {
                if let Some(f) = files.first_mut() {
                    return f.get_content().unwrap_or("").to_string();
                }
            }
            warnings.warnings.push(format!(
                "Placeholder '{{input.{name}.content}}' references undefined input '{name}'"
            ));
            return String::new();
        }
        // {input.<name>} — content of first file in named slot
        let name = rest;
        if let Some(files) = inputs.get_mut(name) {
            if let Some(f) = files.first_mut() {
                return f.get_content().unwrap_or("").to_string();
            }
        }
        warnings.warnings.push(format!(
            "Placeholder '{{input.{name}}}' references undefined input '{name}'"
        ));
        return String::new();
    }

    // {verdict.<validator_name>.status} or {verdict.<validator_name>.feedback}
    if let Some(rest) = placeholder.strip_prefix("verdict.") {
        if let Some(name) = rest.strip_suffix(".status") {
            if let Some(result) = prior_results.get(name) {
                return result.status.to_string();
            } else {
                return "skip".to_string();
            }
        }
        if let Some(name) = rest.strip_suffix(".feedback") {
            if let Some(result) = prior_results.get(name) {
                return result.feedback.clone().unwrap_or_default();
            } else {
                return String::new();
            }
        }
        warnings.warnings.push(format!(
            "Unrecognized verdict placeholder '{{verdict.{rest}}}'"
        ));
        return String::new();
    }

    // Unrecognized placeholder — leave as literal and warn
    warnings
        .warnings
        .push(format!("Unrecognized placeholder '{{{placeholder}}}'"));
    format!("{{{placeholder}}}")
}

/// Resolve environment variable interpolation in config strings.
/// Syntax: `${VAR_NAME}` or `${VAR_NAME:-default_value}`.
/// Escape: `$${` resolves to literal `${`.
pub fn resolve_env_vars(input: &str) -> Result<String, String> {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if i + 1 < len
            && chars[i] == '$'
            && chars[i + 1] == '$'
            && i + 2 < len
            && chars[i + 2] == '{'
        {
            // Escaped: $${ → ${
            result.push('$');
            result.push('{');
            i += 3;
        } else if i + 1 < len && chars[i] == '$' && chars[i + 1] == '{' {
            // Find closing }
            let start = i + 2;
            let close = chars[start..]
                .iter()
                .position(|&c| c == '}')
                .map(|p| p + start);
            let close = match close {
                Some(c) => c,
                None => {
                    // No closing brace — leave as literal
                    result.push('$');
                    result.push('{');
                    i += 2;
                    continue;
                }
            };

            let expr: String = chars[start..close].iter().collect();
            let (var_name, default) = if let Some(idx) = expr.find(":-") {
                (&expr[..idx], Some(&expr[idx + 2..]))
            } else {
                (expr.as_str(), None)
            };

            match std::env::var(var_name) {
                Ok(val) => result.push_str(&val),
                Err(_) => match default {
                    Some(d) => result.push_str(d),
                    None => {
                        return Err(format!(
                            "Environment variable '{var_name}' is not set and has no default"
                        ));
                    }
                },
            }
            i = close + 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers as th;

    // ═══════════════════════════════════════════════════════════════
    // Internal implementation tests
    // NOTE: resolve_env_vars is pub but is a standalone utility;
    //       resolve_placeholders is the primary entry point.
    // ═══════════════════════════════════════════════════════════════

    // ─── Env var interpolation ──────────────────────

    #[test]
    fn env_var_set() {
        std::env::set_var("BATON_TEST_VAR1", "hello");
        let result = resolve_env_vars("prefix_${BATON_TEST_VAR1}_suffix").unwrap();
        assert_eq!(result, "prefix_hello_suffix");
        std::env::remove_var("BATON_TEST_VAR1");
    }

    #[test]
    fn env_var_unset_no_default() {
        std::env::remove_var("BATON_UNSET_VAR_XYZ");
        let result = resolve_env_vars("${BATON_UNSET_VAR_XYZ}");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not set"));
    }

    #[test]
    fn env_var_with_default() {
        std::env::remove_var("BATON_UNSET_VAR_ABC");
        let result = resolve_env_vars("${BATON_UNSET_VAR_ABC:-fallback}").unwrap();
        assert_eq!(result, "fallback");
    }

    #[test]
    fn env_var_with_empty_default() {
        std::env::remove_var("BATON_UNSET_VAR_DEF");
        let result = resolve_env_vars("${BATON_UNSET_VAR_DEF:-}").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn env_var_escaped() {
        let result = resolve_env_vars("literal $${NOT_A_VAR}").unwrap();
        assert_eq!(result, "literal ${NOT_A_VAR}");
    }

    #[test]
    fn env_var_no_interpolation() {
        let result = resolve_env_vars("no vars here").unwrap();
        assert_eq!(result, "no vars here");
    }

    #[test]
    fn env_var_set_overrides_default() {
        std::env::set_var("BATON_TEST_VAR2", "actual");
        let result = resolve_env_vars("${BATON_TEST_VAR2:-default}").unwrap();
        assert_eq!(result, "actual");
        std::env::remove_var("BATON_TEST_VAR2");
    }

    #[test]
    fn env_var_nested_dollar_brace_in_value() {
        std::env::set_var("BATON_TEST_NESTED", "has ${INNER} in it");
        let result = resolve_env_vars("prefix_${BATON_TEST_NESTED}_suffix").unwrap();
        assert_eq!(result, "prefix_has ${INNER} in it_suffix");
        std::env::remove_var("BATON_TEST_NESTED");
    }

    #[test]
    fn env_var_empty_value() {
        std::env::set_var("BATON_TEST_EMPTY", "");
        let result = resolve_env_vars("before_${BATON_TEST_EMPTY}_after").unwrap();
        assert_eq!(result, "before__after");
        std::env::remove_var("BATON_TEST_EMPTY");
    }

    #[test]
    fn env_var_empty_value_does_not_use_default() {
        std::env::set_var("BATON_TEST_EMPTY2", "");
        let result = resolve_env_vars("${BATON_TEST_EMPTY2:-fallback}").unwrap();
        assert_eq!(result, "");
        std::env::remove_var("BATON_TEST_EMPTY2");
    }

    #[test]
    fn env_var_special_chars_in_value() {
        std::env::set_var("BATON_TEST_SPECIAL", "a=b&c;d\"e'f\\g\nh");
        let result = resolve_env_vars("${BATON_TEST_SPECIAL}").unwrap();
        assert_eq!(result, "a=b&c;d\"e'f\\g\nh");
        std::env::remove_var("BATON_TEST_SPECIAL");
    }

    #[test]
    fn env_var_unclosed_brace_literal() {
        let result = resolve_env_vars("before ${UNCLOSED after").unwrap();
        assert_eq!(result, "before ${UNCLOSED after");
    }

    #[test]
    fn env_var_multiple_in_one_string() {
        std::env::set_var("BATON_TEST_A", "alpha");
        std::env::set_var("BATON_TEST_B", "beta");
        let result = resolve_env_vars("${BATON_TEST_A}_${BATON_TEST_B}").unwrap();
        assert_eq!(result, "alpha_beta");
        std::env::remove_var("BATON_TEST_A");
        std::env::remove_var("BATON_TEST_B");
    }

    #[test]
    fn env_var_default_with_special_chars() {
        std::env::remove_var("BATON_UNSET_SPEC");
        let result = resolve_env_vars("${BATON_UNSET_SPEC:-http://localhost:8080/path}").unwrap();
        assert_eq!(result, "http://localhost:8080/path");
    }

    #[test]
    fn env_var_default_containing_colon() {
        std::env::remove_var("BATON_UNSET_COLON");
        let result = resolve_env_vars("${BATON_UNSET_COLON:-key:-value}").unwrap();
        assert_eq!(result, "key:-value");
    }

    #[test]
    fn env_var_adjacent_dollar_signs() {
        let result = resolve_env_vars("cost is $$100").unwrap();
        assert_eq!(result, "cost is $$100");
    }

    #[test]
    fn env_var_dollar_at_end() {
        let result = resolve_env_vars("trailing $").unwrap();
        assert_eq!(result, "trailing $");
    }

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn resolve_verdict_status() {
        let mut inputs = BTreeMap::new();
        let prior = th::prior_results_detailed();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Lint: {verdict.lint.status}, TC: {verdict.typecheck.status}",
            &mut inputs,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Lint: pass, TC: fail");
    }

    #[test]
    fn resolve_verdict_feedback() {
        let mut inputs = BTreeMap::new();
        let prior = th::prior_results_detailed();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Feedback: {verdict.typecheck.feedback}",
            &mut inputs,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Feedback: type error on line 5");
    }

    #[test]
    fn resolve_unrecognized_placeholder() {
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders("Bad: {typo}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "Bad: {typo}");
        assert_eq!(warns.warnings.len(), 1);
    }

    #[test]
    fn resolve_verdict_for_nonexistent_validator() {
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Status: {verdict.nonexistent.status}",
            &mut inputs,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Status: skip");
    }

    #[test]
    fn no_placeholders_unchanged() {
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders("No placeholders here.", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "No placeholders here.");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn unclosed_brace_left_literal() {
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders("Unclosed {brace", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "Unclosed {brace");
    }

    // ─── Spec coverage (UNTESTED) ──────────────────────────

    #[test]
    fn nested_braces_extracted_as_single_placeholder() {
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders("{a{b}c}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "{a{b}c}");
        assert_eq!(warns.warnings.len(), 1);
        assert!(warns.warnings[0].contains("a{b}c"));
    }

    #[test]
    fn nonexistent_validator_feedback_is_empty() {
        let mut inputs = BTreeMap::new();
        let prior = th::prior_results();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "{verdict.nonexistent.feedback}",
            &mut inputs,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn unrecognized_verdict_sub_path_warns() {
        let mut inputs = BTreeMap::new();
        let prior = th::prior_results();
        let mut warns = ResolutionWarnings::new();
        let result =
            resolve_placeholders("{verdict.lint.duration}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "");
        assert_eq!(warns.warnings.len(), 1);
        assert!(warns.warnings[0].contains("verdict"));
    }

    #[test]
    fn multiple_warnings_in_one_call() {
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let _result =
            resolve_placeholders("{unknown1} {unknown2}", &mut inputs, &prior, &mut warns);
        assert!(
            warns.warnings.len() >= 2,
            "Expected at least 2 warnings, got {}",
            warns.warnings.len()
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 placeholder tests
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn resolve_file_path_placeholder() {
        use crate::types::InputFile;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "test content").unwrap();
        let path = f.path().to_path_buf();

        let input = InputFile::new(path.clone());
        assert_eq!(input.path, path);
        assert!(input.path.parent().is_some());
    }

    #[test]
    fn resolve_file_properties() {
        use crate::types::InputFile;

        let input = InputFile::new(std::path::PathBuf::from("/home/user/project/src/main.rs"));

        assert_eq!(
            input.path.parent().unwrap().to_str().unwrap(),
            "/home/user/project/src"
        );
        assert_eq!(input.path.file_name().unwrap().to_str().unwrap(), "main.rs");
        assert_eq!(input.path.file_stem().unwrap().to_str().unwrap(), "main");
        assert_eq!(input.path.extension().unwrap().to_str().unwrap(), "rs");
    }

    #[test]
    fn resolve_file_content_placeholder() {
        use crate::types::InputFile;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "fn main() {{ }}").unwrap();

        let mut input = InputFile::new(f.path().to_path_buf());
        let content = input.get_content().unwrap();
        assert_eq!(content, "fn main() { }");
    }

    #[test]
    fn resolve_named_input_content() {
        use crate::types::InputFile;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut code_file = NamedTempFile::new().unwrap();
        write!(code_file, "print('hello')").unwrap();
        let mut spec_file = NamedTempFile::new().unwrap();
        write!(spec_file, "must print hello").unwrap();

        let mut code_input = InputFile::new(code_file.path().to_path_buf());
        let mut spec_input = InputFile::new(spec_file.path().to_path_buf());

        assert_eq!(code_input.get_content().unwrap(), "print('hello')");
        assert_eq!(spec_input.get_content().unwrap(), "must print hello");
    }

    // ─── Batch placeholder tests (SPEC-PH-BP-001/002, NP-006) ───

    #[test]
    fn resolve_batch_input_concatenates_content() {
        // SPEC-PH-BP-001: {input} in batch mode → concatenated file contents
        use crate::types::InputFile;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f1 = NamedTempFile::new().unwrap();
        write!(f1, "line one").unwrap();
        let mut f2 = NamedTempFile::new().unwrap();
        write!(f2, "line two").unwrap();

        let mut inputs = BTreeMap::new();
        inputs.insert(
            "file".to_string(),
            vec![
                InputFile::new(f1.path().to_path_buf()),
                InputFile::new(f2.path().to_path_buf()),
            ],
        );
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();

        let result = resolve_placeholders("{input}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "line one\nline two");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn resolve_batch_input_content_alias() {
        // SPEC-PH-BP-001: {input.content} is an alias for {input}
        use crate::types::InputFile;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f1 = NamedTempFile::new().unwrap();
        write!(f1, "content").unwrap();

        let mut inputs = BTreeMap::new();
        inputs.insert(
            "file".to_string(),
            vec![InputFile::new(f1.path().to_path_buf())],
        );
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();

        let result = resolve_placeholders("{input.content}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "content");
    }

    #[test]
    fn resolve_input_paths_space_separated() {
        // SPEC-PH-BP-002: {input.paths} → space-separated absolute paths
        use crate::types::InputFile;

        let mut inputs = BTreeMap::new();
        inputs.insert(
            "file".to_string(),
            vec![
                InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
                InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            ],
        );
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();

        let result = resolve_placeholders("{input.paths}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "/tmp/a.py /tmp/b.py");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn resolve_named_input_paths_plural() {
        // SPEC-PH-NP-006: {input.<name>.paths} → space-separated paths for named slot
        use crate::types::InputFile;

        let mut inputs = BTreeMap::new();
        inputs.insert(
            "code".to_string(),
            vec![
                InputFile::new(std::path::PathBuf::from("/tmp/a.py")),
                InputFile::new(std::path::PathBuf::from("/tmp/b.py")),
            ],
        );
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();

        let result =
            resolve_placeholders("{input.code.paths}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "/tmp/a.py /tmp/b.py");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn resolve_named_input_paths_undefined_warns() {
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();

        let result =
            resolve_placeholders("{input.missing.paths}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "");
        assert_eq!(warns.warnings.len(), 1);
        assert!(warns.warnings[0].contains("missing"));
    }

    #[test]
    fn resolve_batch_input_empty_pool() {
        // {input} with no files resolves to empty string
        let mut inputs = BTreeMap::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();

        let result = resolve_placeholders("{input}", &mut inputs, &prior, &mut warns);
        assert_eq!(result, "");
    }
}
