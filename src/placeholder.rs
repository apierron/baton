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
    fn resolve_artifact_content() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Content: {artifact_content}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Content: hello world");
        assert!(warns.warnings.is_empty());
    }

    #[test]
    fn resolve_context_content() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        ctx.add_string("spec".into(), "requirement: do things".into());
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Spec: {context.spec.content}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Spec: requirement: do things");
    }

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
    fn resolve_missing_context_warns() {
        let mut art = Artifact::from_string("hello world");
        let mut ctx = Context::new();
        let prior = BTreeMap::new();
        let mut warns = ResolutionWarnings::new();
        let result = resolve_placeholders(
            "Missing: {context.nonexistent.content}",
            &mut art,
            &mut ctx,
            &prior,
            &mut warns,
        );
        assert_eq!(result, "Missing: ");
        assert_eq!(warns.warnings.len(), 1);
        assert!(warns.warnings[0].contains("nonexistent"));
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
}
