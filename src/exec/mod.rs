//! Gate execution engine.
//!
//! Runs validators in pipeline order, evaluates `run_if` conditions,
//! dispatches to script/LLM/human executors, and computes the final verdict.

mod dispatch;
mod file_pool;
mod human;
mod llm;
mod run_if;
mod script;
mod status;

pub use dispatch::plan_dispatch;
pub use file_pool::{collect_file_pool, FileCollectOptions};
pub use run_if::evaluate_run_if;
pub use status::compute_final_status;

use chrono::Utc;
use std::collections::BTreeMap;
use std::time::Instant;

use crate::config::{BatonConfig, GateConfig, ValidatorConfig, ValidatorType};
use crate::error::Result;
use crate::types::*;

/// Dispatches a single validator by type (script, LLM, or human), evaluating
/// its `run_if` condition and recording wall-clock timing.
pub fn execute_validator(
    validator: &ValidatorConfig,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
    prior_results: &BTreeMap<String, ValidatorResult>,
    config: Option<&BatonConfig>,
) -> ValidatorResult {
    let start = Instant::now();

    // Evaluate run_if
    if let Some(ref run_if_expr) = validator.run_if {
        match evaluate_run_if(run_if_expr, prior_results) {
            Ok(true) => {} // proceed
            Ok(false) => {
                return ValidatorResult {
                    name: validator.name.clone(),
                    status: Status::Skip,
                    feedback: None,
                    duration_ms: 0,
                    cost: None,
                };
            }
            Err(e) => {
                return ValidatorResult {
                    name: validator.name.clone(),
                    status: Status::Error,
                    feedback: Some(format!("[baton] run_if evaluation error: {e}")),
                    duration_ms: 0,
                    cost: None,
                };
            }
        }
    }

    let mut result = match validator.validator_type {
        ValidatorType::Script => script::execute_script_validator(validator, inputs, prior_results),
        ValidatorType::Human => human::execute_human_validator(validator, inputs, prior_results),
        ValidatorType::Llm => llm::execute_llm_validator(validator, inputs, prior_results, config),
    };

    result.duration_ms = start.elapsed().as_millis() as i64;
    result
}

/// Runs all validators in a gate's pipeline and returns a [`Verdict`].
///
/// This is the main entry point for gate execution. Validates the artifact
/// and context, then runs each validator in order, respecting `run_if`
/// conditions, `--only`/`--skip`/`--tags` filters, and blocking semantics.
pub fn run_gate(
    gate: &GateConfig,
    config: &BatonConfig,
    input_pool: Vec<InputFile>,
    options: &RunOptions,
) -> Result<Verdict> {
    let run_start = Instant::now();

    // Build inputs map: all files under "file" key for simple dispatch
    let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
    if !input_pool.is_empty() {
        inputs.insert("file".into(), input_pool);
    }

    // Run validators
    let mut results: BTreeMap<String, ValidatorResult> = BTreeMap::new();
    let mut warnings_list: Vec<String> = Vec::new();

    let suppressed = &options.suppressed_statuses;

    for validator in &gate.validators {
        // Apply filters
        if let Some(ref only) = options.only {
            if !only.contains(&validator.name) {
                results.insert(
                    validator.name.clone(),
                    ValidatorResult {
                        name: validator.name.clone(),
                        status: Status::Skip,
                        feedback: None,
                        duration_ms: 0,
                        cost: None,
                    },
                );
                continue;
            }
        }
        if let Some(ref skip) = options.skip {
            if skip.contains(&validator.name) {
                results.insert(
                    validator.name.clone(),
                    ValidatorResult {
                        name: validator.name.clone(),
                        status: Status::Skip,
                        feedback: None,
                        duration_ms: 0,
                        cost: None,
                    },
                );
                continue;
            }
        }
        if let Some(ref tags) = options.tags {
            if !validator.tags.iter().any(|t| tags.contains(t)) {
                results.insert(
                    validator.name.clone(),
                    ValidatorResult {
                        name: validator.name.clone(),
                        status: Status::Skip,
                        feedback: None,
                        duration_ms: 0,
                        cost: None,
                    },
                );
                continue;
            }
        }

        let result = execute_validator(validator, &mut inputs, &results, Some(config));
        results.insert(validator.name.clone(), result.clone());

        if result.status == Status::Warn {
            warnings_list.push(validator.name.clone());
        }

        // Determine effective status (apply suppression)
        let effective_status = if suppressed.contains(&result.status) {
            Status::Pass
        } else {
            result.status
        };

        if (effective_status == Status::Fail || effective_status == Status::Error)
            && validator.blocking
            && !options.run_all
        {
            let verdict_status = match effective_status {
                Status::Error => VerdictStatus::Error,
                Status::Fail => VerdictStatus::Fail,
                _ => unreachable!(),
            };
            return Ok(Verdict {
                status: verdict_status,
                gate: gate.name.clone(),
                failed_at: Some(validator.name.clone()),
                feedback: result.feedback.clone(),
                duration_ms: run_start.elapsed().as_millis() as i64,
                timestamp: Utc::now(),
                warnings: warnings_list,
                suppressed: suppressed.iter().map(|s| s.to_string()).collect(),
                history: results.values().cloned().collect(),
            });
        }
    }

    // Compute final status
    let result_list: Vec<ValidatorResult> = results.values().cloned().collect();
    let final_status = if options.run_all {
        compute_final_status(&result_list, suppressed)
    } else {
        // In normal mode, if we reach here, no blocking validator failed/errored
        VerdictStatus::Pass
    };

    let (failed_at, feedback) =
        if final_status == VerdictStatus::Fail || final_status == VerdictStatus::Error {
            let target_status = match final_status {
                VerdictStatus::Error => Status::Error,
                VerdictStatus::Fail => Status::Fail,
                _ => unreachable!(),
            };
            let first = result_list.iter().find(|r| r.status == target_status);
            (
                first.map(|r| r.name.clone()),
                first.and_then(|r| r.feedback.clone()),
            )
        } else {
            (None, None)
        };

    Ok(Verdict {
        status: final_status,
        gate: gate.name.clone(),
        failed_at,
        feedback,
        duration_ms: run_start.elapsed().as_millis() as i64,
        timestamp: Utc::now(),
        warnings: warnings_list,
        suppressed: suppressed.iter().map(|s| s.to_string()).collect(),
        history: result_list,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{self as th, ValidatorBuilder};

    // ─── Gate run tests ──────────────────────────────

    #[test]
    fn gate_all_pass() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.history.len(), 3);
    }

    #[test]
    fn gate_first_fail_blocks() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1").build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        assert_eq!(verdict.failed_at, Some("b".into()));
        // c should not have run
        assert_eq!(verdict.history.len(), 2);
    }

    #[test]
    fn gate_non_blocking_failure_passes() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // All 3 ran
        assert_eq!(verdict.history.len(), 3);
    }

    #[test]
    fn gate_all_mode_non_blocking_failure_counts() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn gate_conditional_skip() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 1").build(),
                ValidatorBuilder::script("b", "exit 0")
                    .run_if("a.status == pass")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // b should be skipped because a failed
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Skip);
    }

    #[test]
    fn gate_warn_from_script() {
        let gate = th::gate(
            "test",
            vec![ValidatorBuilder::script("check", "exit 2")
                .warn_exit_codes(vec![2])
                .build()],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.warnings, vec!["check"]);
    }

    #[test]
    fn gate_only_filter() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
                ValidatorBuilder::script("c", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.only = Some(vec!["b".into()]);

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Pass);
        let a_result = verdict.history.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a_result.status, Status::Skip);
    }

    #[test]
    fn gate_skip_filter() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.skip = Some(vec!["a".into()]);

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        let a_result = verdict.history.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a_result.status, Status::Skip);
    }

    #[test]
    fn gate_suppress_errors() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                // This will fail, not error, but let's test with a validator that errors
                ValidatorBuilder::script("b", "exit 1").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Fail];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Fail is suppressed, so pipeline continues and gate passes
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // But history still records the true status
        let b_result = verdict.history.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_result.status, Status::Fail);
    }

    #[test]
    fn gate_error_vs_fail_in_all_mode() {
        // Use a command that will error (nonexistent working dir)
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Error takes precedence over fail
        assert_eq!(verdict.status, VerdictStatus::Error);
    }

    #[test]
    fn gate_suppress_errors_with_all_mode() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;
        opts.suppressed_statuses = vec![Status::Error];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Error suppressed, fail remains
        assert_eq!(verdict.status, VerdictStatus::Fail);
    }

    #[test]
    fn gate_suppress_all_in_all_mode() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;
        opts.suppressed_statuses = vec![Status::Error, Status::Fail, Status::Warn];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
    }

    // ─── execute_validator: run_if error propagation ─────

    #[test]
    fn execute_validator_run_if_error_propagates() {
        let v = ValidatorBuilder::script("run-if-err", "exit 0")
            .run_if("bad expression no operator")
            .build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Error);
        assert!(
            result
                .feedback
                .as_ref()
                .unwrap()
                .contains("run_if evaluation error"),
            "expected run_if error, got: {:?}",
            result.feedback
        );
    }

    // ─── run_gate: --all mode with mixed results ─────────

    #[test]
    fn gate_all_mode_mixed_fail_and_pass_reports_fail() {
        // All validators run; final status uses compute_final_status
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("c", "exit 0").build(),
                ValidatorBuilder::script("d", "exit 1").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Fail);
        // All 4 ran
        assert_eq!(verdict.history.len(), 4);
        // failed_at should point to the first failing validator
        assert_eq!(verdict.failed_at, Some("b".into()));
        assert!(verdict.feedback.is_some());
    }

    #[test]
    fn gate_all_mode_all_pass() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("a", "exit 0").build(),
                ValidatorBuilder::script("b", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert!(verdict.failed_at.is_none());
        assert!(verdict.feedback.is_none());
    }

    // ─── run_gate: warnings list tracking ────────────────

    #[test]
    fn gate_warnings_list_tracks_multiple_warn_validators() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("w1", "exit 2")
                    .warn_exit_codes(vec![2])
                    .build(),
                ValidatorBuilder::script("ok", "exit 0").build(),
                ValidatorBuilder::script("w2", "exit 3")
                    .warn_exit_codes(vec![3])
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
        assert_eq!(verdict.warnings, vec!["w1", "w2"]);
    }

    // ─── run_gate: required context error message format ─

    #[test]
    fn gate_required_context_error_includes_gate_name() {
        // Required context checking has been removed in v2 migration.
        // Gates no longer have context slots — they use input files.
        // This test now just verifies a gate with no validators passes.
        let gate = th::gate(
            "my-gate",
            vec![ValidatorBuilder::script("a", "exit 0").build()],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Pass);
    }

    // ─── run_gate: suppress_all mode (Warn + Error + Fail) ─

    #[test]
    fn gate_suppress_all_statuses_in_blocking_mode() {
        // Fail is blocking, but suppressed — so pipeline continues
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("fail-v", "exit 1").build(),
                ValidatorBuilder::script("warn-v", "exit 2")
                    .warn_exit_codes(vec![2])
                    .build(),
                ValidatorBuilder::script("error-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
                ValidatorBuilder::script("ok-v", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Warn, Status::Error, Status::Fail];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // All suppressed, so gate passes
        assert_eq!(verdict.status, VerdictStatus::Pass);
        // All validators ran (blocking failures were suppressed)
        assert_eq!(verdict.history.len(), 4);
        // Suppressed list recorded in verdict
        assert_eq!(verdict.suppressed.len(), 3);
    }

    // ─── run_gate: error in blocking mode propagates Error verdict ─

    #[test]
    fn gate_blocking_error_returns_error_verdict() {
        // A script validator that errors (not fails) should produce VerdictStatus::Error
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("err-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .build(),
                ValidatorBuilder::script("after", "exit 0").build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let opts = RunOptions::new();

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert_eq!(verdict.status, VerdictStatus::Error);
        assert_eq!(verdict.failed_at, Some("err-v".into()));
        // "after" should not have run
        assert_eq!(verdict.history.len(), 1);
    }

    // ─── run_gate: suppressed list in verdict ────────────

    #[test]
    fn gate_verdict_records_suppressed_statuses() {
        let gate = th::gate(
            "test",
            vec![ValidatorBuilder::script("a", "exit 0").build()],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.suppressed_statuses = vec![Status::Warn, Status::Fail];

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        assert!(verdict.suppressed.contains(&"warn".to_string()));
        assert!(verdict.suppressed.contains(&"fail".to_string()));
    }

    // ─── run_gate: all mode with error reports failed_at ─

    #[test]
    fn gate_all_mode_error_reports_failed_at_error_validator() {
        let gate = th::gate(
            "test",
            vec![
                ValidatorBuilder::script("ok", "exit 0").build(),
                ValidatorBuilder::script("fail-v", "exit 1")
                    .blocking(false)
                    .build(),
                ValidatorBuilder::script("err-v", "exit 0")
                    .working_dir("/nonexistent/dir")
                    .blocking(false)
                    .build(),
            ],
        );
        let config = th::config_for_gate(gate.clone());
        let mut opts = RunOptions::new();
        opts.run_all = true;

        let verdict = run_gate(&gate, &config, vec![], &opts).unwrap();
        // Error beats fail
        assert_eq!(verdict.status, VerdictStatus::Error);
        // failed_at should point to the error validator (first with Error status)
        assert_eq!(verdict.failed_at, Some("err-v".into()));
    }

    // ═══════════════════════════════════════════════════════════════
    // v2 migration: Execution pipeline tests (SPEC-EX-PL-*)
    // ═══════════════════════════════════════════════════════════════

    // SPEC-EX-PL-001: Gates filtered by --only/--skip before iteration
    // SPEC-EX-PL-002: If any invocation of a blocking validator fails, gate stops
    // SPEC-EX-PL-003: Each invocation produces a separate ValidatorResult

    // These tests depend on the new pipeline implementation. The patterns
    // are documented here so they can be filled in during implementation:

    #[test]
    fn pipeline_per_invocation_blocking() {
        // SPEC-EX-PL-002: any failing invocation of a blocking validator stops gate
        // This is a design test verifying the contract. When a per-file blocking
        // validator produces a Fail result for file #2 of 5, files #3-5 should
        // not be executed.
        //
        // EDGE CASE: This means a single failing file can block the entire gate,
        // even if all other files would pass. This is intentional — blocking means
        // "stop on any failure".
        use crate::types::{GateResult, ValidatorResult};

        // Simulate: 3 invocations, second one fails, third should not run
        let results = vec![
            ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: None,
                duration_ms: 10,
                cost: None,
            },
            ValidatorResult {
                name: "lint".into(),
                status: Status::Fail,
                feedback: Some("syntax error".into()),
                duration_ms: 10,
                cost: None,
            },
            // Third invocation should be skipped — verify by absence
        ];

        let gate_result = GateResult {
            gate_name: "review".into(),
            status: Status::Fail,
            validator_results: results,
            duration: std::time::Duration::from_millis(20),
        };

        // Only 2 results (third was blocked)
        assert_eq!(gate_result.validator_results.len(), 2);
        assert_eq!(gate_result.status, Status::Fail);
    }
}
