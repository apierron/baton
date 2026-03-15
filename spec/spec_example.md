# module: exec

The execution engine. Runs validators in pipeline order, evaluates `run_if` conditions, dispatches to script/LLM/human executors, and computes the final verdict.

This module is the core orchestrator — it ties together config, types, placeholder resolution, runtime adapters, and verdict parsing into a single pipeline. It is the largest module by both code and test count.

## Public functions

| Function             | Purpose                                      |
|----------------------|----------------------------------------------|
| `run_gate`           | Run all validators in a gate, return verdict  |
| `execute_validator`  | Run a single validator by type                |
| `evaluate_run_if`    | Evaluate a run_if expression                  |
| `compute_final_status` | Reduce validator results to a verdict status |

## Internal functions

| Function                     | Called by            |
|------------------------------|----------------------|
| `execute_script_validator`   | `execute_validator`  |
| `execute_human_validator`    | `execute_validator`  |
| `execute_llm_completion`     | `execute_validator`  |
| `execute_llm_session`        | `execute_validator`  |
| `evaluate_atom`              | `evaluate_run_if`    |
| `extract_cost`               | `execute_llm_completion` |

## Design notes

run_gate is intentionally a single function rather than being broken into pre-flight/execute/finalize methods. The pipeline is sequential and every step can fail, so early returns dominate the control flow. Splitting it would just move the returns into the callers.

execute_validator takes Option<&BatonConfig> rather than &BatonConfig because script and human validators don't need config. This lets tests run script validators without constructing a full config. LLM validators return Status::Error immediately if config is None.

---

## run_gate

Runs all validators in a gate's pipeline and returns a Verdict.

This is the main entry point for gate execution. It validates inputs, runs each validator in order, respects filtering and blocking semantics,
and computes the final verdict status.

### Sections

1. Pre-flight checks (input validation, hashing)
2. Execution loop (filter, run_if, dispatch, blocking)
3. Finalization (compute status, build verdict)

### run_gate: pre-flight checks

Before executing any validators, run_gate validates that the inputs are well-formed. These checks run in a fixed order. The first failure returns immediately — there is no accumulation of pre-flight errors, because downstream checks may depend on earlier ones succeeding (e.g., hash computation requires a readable file).

Required-context checking happens here, not in validate_config(), because context is provided at runtime via CLI args, not in baton.toml. The config validator checks that context *slots* are well-formed but cannot know whether the user will supply them at invocation time.

Edge cases to consider:

- Artifact is from_string (no path) — path checks are skipped entirely
- Context item whose path doesn't exist vs. context item with inline string
- The ordering of checks matters: a nonexistent path should get ArtifactNotFound, not ArtifactIsDirectory

SPEC-EX-RG-001: artifact-path-must-exist
  When the artifact is file-backed (artifact.path is Some) and the path does not exist on disk, run_gate returns Err(ArtifactNotFound) containing the path string. When the artifact is from_string (path is None), this check is skipped entirely.
  test: exec::tests::gate_artifact_not_found

SPEC-EX-RG-002: artifact-path-rejects-directory
  When artifact.path points to a directory, run_gate returns Err(ArtifactIsDirectory). This check runs after existence, so a nonexistent path always gets ArtifactNotFound, never ArtifactIsDirectory.
  test: exec::tests::gate_artifact_is_directory

SPEC-EX-RG-003: required-context-enforced
  For each context slot in gate.context where required=true, if the provided context map does not contain that key, run_gate returns Err(MissingRequiredContext) naming both the slot and the gate. Only the first missing context triggers the error (early return).
  test: exec::tests::gate_required_context_missing

SPEC-EX-RG-004: unexpected-context-warns-not-errors
  For each key in the provided context that is not declared in gate.context, a warning is printed to stderr. This is not an error; execution continues. The warning includes the item name and gate name. This allows forward-compatible context (passing extra items that a future version of the gate might use).
  test: UNTESTED

SPEC-EX-RG-005: context-path-must-exist
  For each context item backed by a file path, if the path does not exist, run_gate returns Err(ContextNotFound) with the item name and path. If the path is a directory, returns Err(ContextIsDirectory). Context items with inline string content skip this check.
  test: UNTESTED (ContextNotFound case)
  test: UNTESTED (ContextIsDirectory case)

SPEC-EX-RG-006: hashes-computed-before-execution
  After pre-flight validation passes, artifact_hash and context_hash are computed before any validators run. This ensures the verdict records the exact input state, even if a validator modifies files on disk during execution. Artifact hash is SHA-256 of file content (triggers lazy load). Context hash is SHA-256 of the sorted concatenation of all context item contents.
  test: IMPLICIT via exec::tests::gate_all_pass (verdict contains hashes)

### run_gate: execution loop

Validators run in the order they appear in the gate config. For each validator, the loop applies filters, evaluates run_if, dispatches execution, and checks blocking status.

The loop maintains a BTreeMap<String, ValidatorResult> of prior results. This map is passed to each validator for run_if evaluation and placeholder resolution (e.g., {verdict.lint.status}).

Key invariant: a validator can only reference results from validators that appeared earlier in the pipeline. The config validator enforces this for run_if expressions, but placeholder resolution handles missing references gracefully (empty string).

SPEC-EX-RG-010: only-filter-skips-unlisted
  When options.only is Some, validators whose name is not in the list are recorded as Status::Skip with no feedback. They do not execute. Skipped validators appear in the verdict history.
  test: exec::tests::gate_only_filter

SPEC-EX-RG-011: skip-filter-skips-listed
  When options.skip is Some, validators whose name is in the list are recorded as Status::Skip. This is the inverse of --only.
  test: exec::tests::gate_skip_filter

SPEC-EX-RG-012: tags-filter-skips-untagged
  When options.tags is Some, validators that have no tags in common with the filter set are recorded as Status::Skip. A validator with tags ["a", "b"] matches a filter of ["b", "c"]. A validator with no tags never matches a tags filter.
  test: exec::tests::gate_tags_filter

SPEC-EX-RG-013: filter-order-is-only-then-skip-then-tags
  Filters are evaluated in order: only, skip, tags. If a validator is excluded by --only, --skip is never checked. This matters for the skip reason in dry-run output but not for execution semantics (all three produce Status::Skip).
  test: UNTESTED (ordering specifically)

SPEC-EX-RG-014: blocking-validator-stops-pipeline
  When a validator has blocking=true and its effective status (after suppression) is Fail or Error, the pipeline stops immediately. No subsequent validators execute. The verdict status reflects the blocking failure: Fail→VerdictStatus::Fail, Error→VerdictStatus::Error. The verdict's failed_at field names the blocking validator.
  test: exec::tests::gate_first_fail_blocks

SPEC-EX-RG-015: non-blocking-failure-continues
  When a validator has blocking=false and fails, execution continues to the next validator. The failure is recorded in history and contributes to the final status computation.
  test: exec::tests::gate_non_blocking_failure_passes

SPEC-EX-RG-016: run-all-overrides-blocking
  When options.run_all is true, blocking=true is ignored. All validators execute regardless of intermediate failures. The final status is computed from all results via compute_final_status().
  test: exec::tests::gate_all_mode_runs_everything

SPEC-EX-RG-017: suppression-applied-before-blocking-check
  Status suppression (suppress_warnings, suppress_errors) is applied before the blocking check. A validator that returns Error with blocking=true will not stop the pipeline if Error is suppressed. The suppressed status is treated as Pass for blocking purposes.
  test: exec::tests::gate_suppress_errors

### run_gate: finalization

After the execution loop completes (either all validators ran, or a blocking failure stopped early), run_gate assembles the Verdict.

SPEC-EX-RG-020: normal-mode-pass-on-completion
  In normal mode (run_all=false), if the execution loop completes without a blocking failure, the verdict status is Pass. Non-blocking failures do not affect the final status. This is intentional: non-blocking validators are advisory. Their results appear in history but don't fail the gate.
  test: exec::tests::gate_non_blocking_failure_passes

SPEC-EX-RG-021: run-all-mode-uses-compute-final-status
  In run_all mode, the final status is computed by compute_final_status() over all results. Error beats Fail beats Pass. Skip and Warn are not considered.
  test: exec::tests::gate_all_mode_non_blocking_failure_counts

SPEC-EX-RG-022: verdict-history-contains-all-results
  The verdict.history field contains every validator result, including skipped validators. Order matches the gate config order.
  test: IMPLICIT via exec::tests::gate_all_pass

---

## execute_validator

Dispatches a single validator by type (script, LLM, or human). Evaluates run_if condition first. Records wall-clock timing.

SPEC-EX-EV-001: run-if-evaluated-before-dispatch
  If the validator has a run_if expression, it is evaluated against prior_results before execution. If the expression evaluates to false, the validator is recorded as Status::Skip with feedback "[baton] Skipped: run_if evaluated to false".
  test: exec::tests::gate_conditional_skip

SPEC-EX-EV-002: dispatch-by-type
  Validators are dispatched based on validator_type:
    Script → execute_script_validator
    Llm with mode=Completion → execute_llm_completion
    Llm with mode=Session → execute_llm_session
    Human → execute_human_validator
  test: IMPLICIT via type-specific tests

SPEC-EX-EV-003: duration-recorded
  Wall-clock duration in milliseconds is recorded in the result's duration_ms field, measured around the dispatch call. This includes the full execution time (network, subprocess, polling). run_if evaluation time is included in the measurement.
  test: UNTESTED (timing is not asserted in any current test)

---

## compute_final_status

Reduces a list of ValidatorResults to a single VerdictStatus, applying status suppression.

This is a pure function with no side effects.

SPEC-EX-CF-001: error-beats-fail
  If any non-skipped result has effective status Error, the verdict is VerdictStatus::Error, regardless of other statuses.
  test: exec::tests::final_status_error_beats_fail

SPEC-EX-CF-002: fail-beats-pass
  If no Error is present but any non-skipped result has effective status Fail, the verdict is VerdictStatus::Fail.
  test: exec::tests::final_status_with_fail

SPEC-EX-CF-003: warn-treated-as-pass
  Warn status does not contribute to failure. A gate with only Pass and Warn results produces VerdictStatus::Pass. This is intentional: Warn exists for advisory feedback, not gate failure. Use blocking=true + Fail for hard requirements.
  test: exec::tests::final_status_with_warn

SPEC-EX-CF-004: skip-ignored
  Skipped validators are filtered out before status computation. A gate where all validators are skipped produces VerdictStatus::Pass.
  test: exec::tests::final_status_skip_ignored

SPEC-EX-CF-005: suppression-converts-to-pass
  Suppressed statuses are converted to Pass before aggregation. If Error is suppressed and Fail is not, an Error result is treated as Pass, allowing a Fail from another validator to determine the verdict.
  test: exec::tests::final_status_suppress_errors

SPEC-EX-CF-006: all-suppressed-produces-pass
  When all non-pass statuses are suppressed, the verdict is always VerdictStatus::Pass regardless of individual validator results.
  test: exec::tests::final_status_suppress_all

SPEC-EX-CF-007: empty-results-produces-pass
  If the results list is empty (no validators ran, or all were skipped), the verdict is VerdictStatus::Pass.
  test: UNTESTED
