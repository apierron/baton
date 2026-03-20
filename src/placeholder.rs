//! Template placeholder resolution.
//!
//! Substitutes `{artifact}`, `{context.<name>}`, `{verdict.<name>.status}`,
//! and similar placeholders in command strings and prompt templates.

use crate::types::{Artifact, Context, ValidatorResult};
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
/// - `{artifact}` — absolute path to the artifact file
/// - `{artifact_dir}` — absolute path to the artifact's parent directory
/// - `{artifact_content}` — inline content of the artifact
/// - `{context.<name>}` — absolute path to named context item
/// - `{context.<name>.content}` — inline content of named context item
/// - `{verdict.<validator_name>.status}` — status of a prior validator
/// - `{verdict.<validator_name>.feedback}` — feedback from a prior validator
pub fn resolve_placeholders(
    template: &str,
    artifact: &mut Artifact,
    context: &mut Context,
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
                let resolved =
                    resolve_single(&placeholder, artifact, context, prior_results, warnings);
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

fn resolve_single(
    placeholder: &str,
    artifact: &mut Artifact,
    context: &mut Context,
    prior_results: &BTreeMap<String, ValidatorResult>,
    warnings: &mut ResolutionWarnings,
) -> String {
    // {artifact}
    if placeholder == "artifact" {
        return artifact.absolute_path().unwrap_or_default();
    }

    // {artifact_dir}
    if placeholder == "artifact_dir" {
        return artifact.parent_dir().unwrap_or_default();
    }

    // {artifact_content}
    if placeholder == "artifact_content" {
        return artifact.get_content_as_string().unwrap_or_default();
    }

    // {context.<name>.content}
    if let Some(rest) = placeholder.strip_prefix("context.") {
        if let Some(name) = rest.strip_suffix(".content") {
            if let Some(item) = context.items.get_mut(name) {
                return item.get_content().unwrap_or("").to_string();
            } else {
                warnings.warnings.push(format!(
                    "Placeholder '{{context.{name}.content}}' references undefined context '{name}'"
                ));
                return String::new();
            }
        }
        // {context.<name>} — path
        let name = rest;
        if let Some(item) = context.items.get(name) {
            return item.absolute_path().unwrap_or_default();
        } else {
            warnings.warnings.push(format!(
                "Placeholder '{{context.{name}}}' references undefined context '{name}'"
            ));
            return String::new();
        }
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
        // Value containing ${ should be emitted literally
        std::env::set_var("BATON_TEST_NESTED", "has ${INNER} in it");
        let result = resolve_env_vars("prefix_${BATON_TEST_NESTED}_suffix").unwrap();
        assert_eq!(result, "prefix_has ${INNER} in it_suffix");
        std::env::remove_var("BATON_TEST_NESTED");
    }

    #[test]
    fn env_var_empty_value() {
        // Empty string is a valid value — not the same as unset
        std::env::set_var("BATON_TEST_EMPTY", "");
        let result = resolve_env_vars("before_${BATON_TEST_EMPTY}_after").unwrap();
        assert_eq!(result, "before__after");
        std::env::remove_var("BATON_TEST_EMPTY");
    }

    #[test]
    fn env_var_empty_value_does_not_use_default() {
        // Empty string is set — should NOT fall through to default
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
        // Unclosed ${ should be left as literal text, not error
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
        // The :- delimiter only matches the first occurrence
        std::env::remove_var("BATON_UNSET_COLON");
        let result = resolve_env_vars("${BATON_UNSET_COLON:-key:-value}").unwrap();
        assert_eq!(result, "key:-value");
    }

    #[test]
    fn env_var_adjacent_dollar_signs() {
        // $$ not followed by { is literal
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
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = th::prior_results_detailed();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Lint: {verdict.lint.status}, TC: {verdict.typecheck.status}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Lint: pass, TC: fail");
    }

    #[test]
    fn resolve_verdict_feedback() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = th::prior_results_detailed();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Feedback: {verdict.typecheck.feedback}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Feedback: type error on line 5");
    }

    #[test]
    fn resolve_unrecognized_placeholder() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders("Bad: {typo}", &mut art, &mut ctx, &prior, &mut warns);
        assert_eq!(result, "Bad: {typo}");
        assert_eq!(warns.warnings.len(), 1);
    }

    #[test]
    fn resolve_verdict_for_nonexistent_validator() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Status: {verdict.nonexistent.status}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Status: skip");
    }

    #[test]
    fn no_placeholders_unchanged() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "No placeholders here.",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "No placeholders here.");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn unclosed_brace_left_literal() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result =
            resolve_placeholders("Unclosed {brace", &mut art, &mut ctx, &prior, &mut warns);
        assert_eq!(result, "Unclosed {brace");
    }

    // ─── Spec coverage (UNTESTED) ──────────────────────────

    #[test]
    fn nested_braces_extracted_as_single_placeholder() {
        let mut art = Artifact::from_string("x");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders("{a{b}c}", &mut art, &mut ctx, &prior, &mut warns);
        // The outer braces match: open at 0, inner { at 2, inner } at 4 (depth back to 1),
        // outer } at 6 (depth 0). Extracted placeholder content is "a{b}c".
        // "a{b}c" is unrecognized, so it is kept as literal and a warning is emitted.
        assert_eq!(result, "{a{b}c}");
        assert_eq!(warns.warnings.len(), 1);
        assert!(warns.warnings[0].contains("a{b}c"));
    }

    #[test]
    fn nonexistent_validator_feedback_is_empty() {
        let mut art = Artifact::from_string("x");
        let mut ctx = Context::new();
        let prior = th::prior_results();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "{verdict.nonexistent.feedback}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn unrecognized_verdict_sub_path_warns() {
        let mut art = Artifact::from_string("x");
        let mut ctx = Context::new();
        let prior = th::prior_results();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "{verdict.lint.duration}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "");
        assert_eq!(warns.warnings.len(), 1);
        assert!(warns.warnings[0].contains("verdict"));
    }

    #[test]
    fn multiple_warnings_in_one_call() {
        let mut art = Artifact::from_string("x");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let _result = resolve_placeholders(
            "{unknown1} {unknown2}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert!(
            warns.warnings.len() >= 2,
            "Expected at least 2 warnings, got {}",
            warns.warnings.len()
        );
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 migration: New placeholder tests
    //
    // These tests define the contract for the v2 placeholder system.
    // They test against a new resolve function that takes Invocation
    // instead of Artifact/Context. The tests below are structured as
    // compilable stubs that document the expected behavior.
    //
    // IMPLEMENTATION NOTE: When resolve_placeholders is updated to
    // accept Invocation, uncomment these tests and remove the old
    // Artifact/Context-based tests above.
    // ═══════════════════════════════════════════════════════════════

    // --- Per-file placeholders (SPEC-PH-FP-*) ---
    //
    // SPEC-PH-FP-001: {file} and {file.path} resolve to absolute path
    // SPEC-PH-FP-002: {file.dir} resolves to parent directory
    // SPEC-PH-FP-003: {file.name} resolves to filename with extension
    // SPEC-PH-FP-004: {file.stem} resolves to filename without extension
    // SPEC-PH-FP-005: {file.ext} resolves to extension without dot
    // SPEC-PH-FP-006: {file.content} resolves to file contents as UTF-8
    // SPEC-PH-FP-007: {file.*} in batch/named mode is config validation error
    //
    // --- Batch placeholders (SPEC-PH-BP-*) ---
    //
    // SPEC-PH-BP-001: {input} in batch mode resolves to concatenated content
    // SPEC-PH-BP-002: {input.paths} resolves to space-separated absolute paths
    //
    // --- Named input placeholders (SPEC-PH-NP-*) ---
    //
    // SPEC-PH-NP-001: {input.<name>} resolves to content (LLM) or path (script)
    // SPEC-PH-NP-002: {input.<name>.path} resolves to absolute path
    // SPEC-PH-NP-003: {input.<name>.name} resolves to filename
    // SPEC-PH-NP-004: {input.<name>.stem} resolves to stem
    // SPEC-PH-NP-005: {input.<name>.content} resolves to file content
    // SPEC-PH-NP-006: {input.<name>.paths} resolves to space-separated paths
    // SPEC-PH-NP-007: missing {input.<name>} warns and resolves to empty
    //
    // --- Placeholder validation (SPEC-PH-VL-*) ---
    //
    // SPEC-PH-VL-001: validate-config checks placeholders match declared inputs
    // SPEC-PH-VL-002: {file} in named-input mode is config error
    // SPEC-PH-VL-003: {input} (batch) in per-file mode is config error

    // The test bodies are written below but will need the new function
    // signature. Here's the test for {file} resolution as an example
    // of the pattern all tests will follow once the API is updated:

    #[test]
    fn resolve_file_path_placeholder() {
        // SPEC-PH-FP-001: {file} resolves to absolute path of the input file
        // This test uses the NEW InputFile type and verifies the placeholder
        // resolves to its absolute path. It will need the new resolve function.
        use crate::types::InputFile;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f = NamedTempFile::new().unwrap();
        write!(f, "test content").unwrap();
        let path = f.path().to_path_buf();

        // Verify InputFile stores the path correctly
        let input = InputFile::new(path.clone());
        assert_eq!(input.path, path);
        // The path should have a parent directory (for {file.dir})
        assert!(input.path.parent().is_some());
    }

    #[test]
    fn resolve_file_properties() {
        // SPEC-PH-FP-002 through SPEC-PH-FP-005: file.dir, file.name, file.stem, file.ext
        use crate::types::InputFile;

        let input = InputFile::new(std::path::PathBuf::from("/home/user/project/src/main.rs"));

        // Verify the path components that placeholders will resolve to
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
        // SPEC-PH-FP-006: {file.content} resolves to file contents as UTF-8
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
        // SPEC-PH-NP-005: {input.<name>.content} resolves to file content
        use crate::types::InputFile;
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut code_file = NamedTempFile::new().unwrap();
        write!(code_file, "print('hello')").unwrap();
        let mut spec_file = NamedTempFile::new().unwrap();
        write!(spec_file, "must print hello").unwrap();

        let mut code_input = InputFile::new(code_file.path().to_path_buf());
        let mut spec_input = InputFile::new(spec_file.path().to_path_buf());

        // Verify the content that {input.code.content} and {input.spec.content}
        // will resolve to once the placeholder system is updated
        assert_eq!(code_input.get_content().unwrap(), "print('hello')");
        assert_eq!(spec_input.get_content().unwrap(), "must print hello");
    }
}
