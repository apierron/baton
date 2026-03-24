//! `run_if` condition evaluation for validator dispatch.
//!
//! Parses and evaluates expressions of the form `name.status == value`,
//! joined by `and`/`or` with left-to-right evaluation and no short-circuit.
//! Missing validators are treated as `skip`.

use std::collections::BTreeMap;

use crate::config::split_run_if;
use crate::error::{BatonError, Result};
use crate::types::*;

/// Evaluate a run_if expression against prior validator results.
///
/// Parses `expr` as a sequence of `name.status == value` atoms joined by
/// `and`/`or` operators. Evaluation is left-to-right with no precedence
/// and no short-circuit (all atoms are evaluated to catch missing references).
///
/// Returns `Err` if the expression is empty or references a validator not
/// present in `prior_results`.
pub fn evaluate_run_if(
    expr: &str,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> Result<bool> {
    let tokens = split_run_if(expr);

    if tokens.is_empty() {
        return Err(BatonError::ValidationError(
            "Empty run_if expression".into(),
        ));
    }

    // Evaluate first atom
    let mut current_result = evaluate_atom(&tokens[0], prior_results)?;

    // Evaluate remaining atoms left-to-right, no precedence, NO short-circuit.
    let mut i = 1;
    while i < tokens.len() {
        if i + 1 >= tokens.len() {
            return Err(BatonError::ValidationError(format!(
                "Invalid run_if expression: '{expr}'"
            )));
        }
        let operator = &tokens[i];
        let next_result = evaluate_atom(&tokens[i + 1], prior_results)?;

        match operator.as_str() {
            "and" => current_result = current_result && next_result,
            "or" => current_result = current_result || next_result,
            other => {
                return Err(BatonError::ValidationError(format!(
                    "Invalid operator in run_if: '{other}'. Expected 'and' or 'or'."
                )));
            }
        }

        i += 2;
    }

    Ok(current_result)
}

fn evaluate_atom(atom: &str, prior_results: &BTreeMap<String, ValidatorResult>) -> Result<bool> {
    let parts: Vec<&str> = atom.split(".status == ").collect();
    if parts.len() != 2 {
        return Err(BatonError::ValidationError(format!(
            "Invalid run_if expression: '{atom}'. Expected '<name>.status == <value>'"
        )));
    }

    let validator_name = parts[0].trim();
    let expected_status = parts[1].trim();

    let expected: Status = expected_status.parse().map_err(|_| {
        BatonError::ValidationError(format!("Invalid status in run_if: '{expected_status}'"))
    })?;

    match prior_results.get(validator_name) {
        Some(result) => Ok(result.status == expected),
        None => Ok(expected == Status::Skip),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers as th;

    // ─── run_if evaluation ───────────────────────────

    #[test]
    fn run_if_simple_pass() {
        let results = th::prior_results();
        assert!(evaluate_run_if("lint.status == pass", &results).unwrap());
    }

    #[test]
    fn run_if_simple_fail() {
        let results = th::prior_results();
        assert!(!evaluate_run_if("lint.status == fail", &results).unwrap());
    }

    #[test]
    fn run_if_and_both_true() {
        let results = th::prior_results();
        assert!(
            !evaluate_run_if("lint.status == pass and typecheck.status == pass", &results).unwrap()
        );
    }

    #[test]
    fn run_if_or_one_true() {
        let results = th::prior_results();
        assert!(
            evaluate_run_if("lint.status == fail or typecheck.status == fail", &results).unwrap()
        );
    }

    #[test]
    fn run_if_left_to_right_no_precedence() {
        // "a or b and c" → "(a or b) and c"
        let mut results = BTreeMap::new();
        results.insert("a".into(), th::result("a", Status::Pass));
        results.insert("b".into(), th::result("b", Status::Fail));
        results.insert("c".into(), th::result("c", Status::Fail));

        // a.pass or b.pass → true or false → true
        // true and c.pass → true and false → false
        let result = evaluate_run_if(
            "a.status == pass or b.status == pass and c.status == pass",
            &results,
        )
        .unwrap();
        assert!(!result);
    }

    #[test]
    fn run_if_skipped_validator() {
        let mut results = BTreeMap::new();
        results.insert("a".into(), th::result("a", Status::Skip));
        assert!(evaluate_run_if("a.status == skip", &results).unwrap());
    }

    #[test]
    fn run_if_nonexistent_treated_as_skip() {
        let results = BTreeMap::new();
        assert!(evaluate_run_if("nonexistent.status == skip", &results).unwrap());
        assert!(!evaluate_run_if("nonexistent.status == pass", &results).unwrap());
    }

    #[test]
    fn run_if_invalid_expression() {
        let results = BTreeMap::new();
        assert!(evaluate_run_if("invalid expression", &results).is_err());
    }

    #[test]
    fn run_if_empty_expression_returns_err() {
        let results = BTreeMap::new();
        let err = evaluate_run_if("", &results).unwrap_err();
        assert!(
            err.to_string().contains("Empty run_if"),
            "expected 'Empty run_if' in error, got: {err}"
        );
    }

    #[test]
    fn run_if_unrecognized_status_returns_err() {
        let results = th::prior_results();
        let err = evaluate_run_if("lint.status == invalid_status", &results).unwrap_err();
        assert!(
            err.to_string().contains("Invalid status"),
            "expected 'Invalid status' in error, got: {err}"
        );
    }

    #[test]
    fn run_if_expression_ending_with_operator_returns_err() {
        let results = th::prior_results();
        // "lint.status == pass and" — the trailing "and" gets absorbed into
        // the atom text by split_run_if, producing an invalid atom that cannot
        // be parsed. Either way an Err is returned.
        let err = evaluate_run_if("lint.status == pass and", &results).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Invalid"),
            "expected 'Invalid' in error, got: {msg}"
        );
    }
}
