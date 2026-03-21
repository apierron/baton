# module: exec

The execution engine. Collects input files into a pool, uses the dispatch planner to turn the pool into invocations, runs validators as stateless functions, and lets gates orchestrate sequencing and blocking.

This module implements the `sources → validators → gates` pipeline at runtime. The file collector builds the input pool. The dispatch planner matches files to validator input declarations and produces invocations. Gate execution iterates validators in pipeline order, applying blocking and `run_if` orchestration rules. Individual validator execution (script/LLM/human) is a leaf operation with no knowledge of gates or pipelines.

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
| `execute_llm_validator`      | `execute_validator`  |
| `drive_session`              | `execute_llm_validator`|
| `evaluate_atom`              | `evaluate_run_if`    |

## Design notes

Validators are stateless functions. `execute_validator` takes an invocation (validator config + input files) and produces a result. It has no knowledge of what gate it belongs to, what other validators exist, or whether it's blocking. This is by design — all orchestration lives in the gate layer.

Gates handle sequencing. `blocking` and `run_if` are both gate-level orchestration concerns, not validator properties. `blocking = true` means "if this validator fails, stop the gate." `run_if` means "only run this validator if a prior validator's result meets a condition." In many cases, `blocking` alone is sufficient — if validator A is blocking and fails, validators B and C never run. `run_if` adds value when: (a) the dependency isn't the immediately preceding validator, or (b) the dependency is non-blocking (A produces a result but doesn't stop the pipeline, and B should only run if A passed). Whether `run_if` carries its weight relative to blocking is an open question; the current design includes it because the baton-vision example uses it for non-blocking conditional chains.

execute_validator takes an optional config because script and human validators don't need provider/runtime configuration. LLM validators return an error if config is absent.

---

## evaluate_run_if

Evaluates a run_if expression against prior validator results. Left-to-right evaluation, no operator precedence, no short-circuit.

### Sections

1. Tokenization (delegated to `config::split_run_if`)
2. Atom evaluation
3. Operator chaining

The lack of short-circuit evaluation is intentional: every atom is evaluated even if the result is already determined. This ensures that missing-validator references are caught at evaluation time rather than silently skipped when the left operand would determine the result.

SPEC-EX-RI-001: empty-expression-errors
  Empty run_if expression returns Err(ValidationError).
  test: exec::tests::run_if_empty_expression_returns_err

SPEC-EX-RI-002: simple-atom-matches-status
  An atom `name.status == value` evaluates to true when the prior result for `name` has the matching status.
  test: exec::tests::run_if_simple_pass

SPEC-EX-RI-003: simple-atom-mismatch
  An atom evaluates to false when the prior result's status does not match the expected value.
  test: exec::tests::run_if_simple_fail

SPEC-EX-RI-004: and-requires-both-true
  The `and` operator returns true only when both operands are true.
  test: exec::tests::run_if_and_both_true

SPEC-EX-RI-005: or-returns-true-if-either-true
  The `or` operator returns true when at least one operand is true.
  test: exec::tests::run_if_or_one_true

SPEC-EX-RI-006: left-to-right-no-precedence
  Mixed `and`/`or` expressions evaluate left-to-right without operator precedence. `a or b and c` is evaluated as `(a or b) and c`, not `a or (b and c)`.
  test: exec::tests::run_if_left_to_right_no_precedence

SPEC-EX-RI-007: skipped-validator-matches-skip
  A validator with Status::Skip in prior results matches `name.status == skip`.
  test: exec::tests::run_if_skipped_validator

SPEC-EX-RI-008: nonexistent-validator-treated-as-skip
  If a referenced validator is not in prior_results, it is treated as having Status::Skip. This means `nonexistent.status == skip` is true and `nonexistent.status == pass` is false.

This is a deliberate design choice: it allows run_if to reference validators that may have been filtered out by --only/--skip/--tags without erroring.

  test: exec::tests::run_if_nonexistent_treated_as_skip

SPEC-EX-RI-009: invalid-atom-syntax-errors
  An atom that doesn't match the `name.status == value` pattern returns Err(ValidationError).
  test: exec::tests::run_if_invalid_expression

SPEC-EX-RI-010: invalid-status-value-errors
  An atom with an unrecognized status value (not pass/fail/warn/skip/error) returns Err(ValidationError).
  test: exec::tests::run_if_unrecognized_status_returns_err

SPEC-EX-RI-011: trailing-operator-errors
  An expression ending with an operator and no following atom (e.g., "a.status == pass and") returns Err(ValidationError).
  test: exec::tests::run_if_expression_ending_with_operator_returns_err

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
  test: exec::tests::compute_final_status_empty_results_is_pass

---

## execute_validator

Dispatches a single validator by type (script, LLM, or human). Evaluates run_if condition first. Records wall-clock timing.

SPEC-EX-EV-001: run-if-evaluated-before-dispatch
  If the validator has a run_if expression, it is evaluated against prior_results before execution. If the expression evaluates to false, the validator is recorded as Status::Skip with no feedback.
  test: exec::tests::gate_conditional_skip

SPEC-EX-EV-002: run-if-error-returns-error-status
  If run_if evaluation fails (e.g., invalid expression), the validator is recorded as Status::Error with feedback "[baton] run_if evaluation error: ...".
  test: UNTESTED (config validation catches malformed expressions before runtime)

SPEC-EX-EV-003: dispatch-by-type
  Validators are dispatched based on validator_type:
    Script → execute_script_validator
    Llm → execute_llm_validator (handles both query and session modes internally via runtime fallback)
    Human → execute_human_validator
  test: IMPLICIT via type-specific tests

SPEC-EX-EV-004: duration-recorded
  Wall-clock duration in milliseconds is recorded in the result's duration_ms field, measured around the dispatch call. This includes the full execution time (network, subprocess, polling). run_if evaluation time is included in the measurement.
  test: UNTESTED (timing is not asserted in any current test)

---

## execute_script_validator

Runs a script command as a subprocess, resolves placeholders, and maps exit codes to statuses.

### Sections

1. Placeholder resolution in command
2. Working directory validation
3. Process spawning (sh -c on unix, cmd /C on windows)
4. Exit code mapping

SPEC-EX-SV-001: exit-0-is-pass
  Exit code 0 produces Status::Pass with no feedback.
  test: exec::tests::script_exit_0_pass

SPEC-EX-SV-002: exit-nonzero-is-fail
  Exit codes not in warn_exit_codes produce Status::Fail. Feedback is stdout+stderr combined, or "[baton] Script exited with code N (no output)" if both are empty.
  test: exec::tests::script_exit_1_fail

SPEC-EX-SV-003: warn-exit-codes-produce-warn
  Exit codes listed in warn_exit_codes produce Status::Warn with stdout+stderr as feedback.
  test: exec::tests::script_exit_with_warn_code

SPEC-EX-SV-004: exit-2-without-warn-codes-is-fail
  Exit code 2 is Fail when not listed in warn_exit_codes — warn_exit_codes must be explicitly configured.
  test: exec::tests::script_exit_2_without_warn_codes_is_fail

SPEC-EX-SV-005: no-output-fail-includes-exit-code
  When a script fails with no stdout/stderr, feedback includes the exit code in a "[baton]" prefix message.
  test: exec::tests::script_no_output_fail_feedback

SPEC-EX-SV-006: stderr-included-in-feedback
  Both stdout and stderr are captured and combined in the feedback string.
  test: exec::tests::script_with_stderr_feedback

SPEC-EX-SV-007: placeholders-resolved-in-command
  The {file}, {file.content}, {input.X}, etc. placeholders are resolved in the command string before execution.
  test: exec::tests::script_placeholder_resolution

SPEC-EX-SV-008: empty-command-after-resolution-errors
  If the command is empty (or whitespace-only) after placeholder resolution, Status::Error with "[baton] Command is empty after placeholder resolution".
  test: exec::tests::script_empty_command_returns_error

SPEC-EX-SV-009: working-dir-not-found-errors
  If the specified working_dir does not exist, Status::Error with "[baton] Working directory not found: ...".
  test: IMPLICIT via exec::tests::gate_error_vs_fail_in_all_mode

SPEC-EX-SV-010: command-not-found-errors
  If the shell reports command not found (io::ErrorKind::NotFound), Status::Error with "[baton] Command not found: ...".
  test: UNTESTED

SPEC-EX-SV-011: permission-denied-errors
  If the shell reports permission denied, Status::Error with "[baton] Permission denied: ...".
  test: UNTESTED

SPEC-EX-SV-012: env-vars-passed-to-subprocess
  Environment variables from validator.env are passed to the subprocess via cmd.env().
  test: exec::tests::script_env_vars_passed_to_subprocess

SPEC-EX-SV-013: platform-specific-shell
  On unix, commands run via `sh -c`. On windows, via `cmd /C`. This allows shell features like pipes, redirects, and chaining in commands.
  test: IMPLICIT via all script tests passing on both platforms

SPEC-EX-SV-014: warn-exit-code-no-output
  When a warn exit code fires but stdout+stderr are empty, feedback is "[baton] Script exited with code N (warn, no output)".
  test: exec::tests::script_warn_exit_code_with_empty_output

---

## execute_human_validator

Always returns Status::Fail with a `[human-review-requested]` prefix followed by the rendered prompt.

Human validators are a placeholder — they always fail the gate to signal that a human needs to take action. The prompt is resolved with placeholders so the human reviewer gets context.

SPEC-EX-HV-001: always-fails-with-prompt
  Human validators always return Status::Fail with feedback "[human-review-requested] {rendered_prompt}".
  test: exec::tests::human_validator_fails_with_prompt

SPEC-EX-HV-002: placeholders-resolved-in-prompt
  Placeholders in the human validator's prompt are resolved before inclusion in the feedback.
  test: IMPLICIT via human_validator_fails_with_prompt (prompt text appears resolved)

---

## execute_llm_validator

Unified LLM validator execution with runtime fallback. Resolves prompt and placeholders, then iterates the validator's runtimes list in order. For each runtime, attempts the operation based on mode (query or session). Falls through on unreachable runtimes or capability mismatches.

### Sections

1. Config validation
2. Prompt resolution and placeholder substitution
3. Runtime fallback loop
4. Query mode dispatch
5. Session mode dispatch

### execute_llm_validator: config validation

SPEC-EX-LV-001: no-config-errors
  If config is None, returns Status::Error "[baton] LLM validator requires config with runtime settings".
  test: UNTESTED

SPEC-EX-LV-002: empty-runtimes-errors
  If the validator's runtimes list is empty, returns Status::Error.
  test: UNTESTED

### execute_llm_validator: prompt resolution

SPEC-EX-LV-010: prompt-required
  If the validator has no prompt, returns Status::Error.
  test: UNTESTED

SPEC-EX-LV-011: prompt-file-resolution
  File-reference prompts are loaded via resolve_prompt_value.
  test: UNTESTED

SPEC-EX-LV-012: prompt-placeholders-resolved
  Placeholders in the prompt are resolved before use.
  test: UNTESTED

### execute_llm_validator: runtime fallback loop

The fallback loop iterates the validator's runtimes in order. For each runtime:
1. Look up runtime config — if undefined, error (should be caught at validation)
2. Create adapter via create_adapter — if fails, warn and try next
3. Health check — if unreachable, warn and try next
4. Dispatch based on mode:
   - Query: call adapter.post_completion() — if returns RuntimeError "not supported", try next
   - Session: if runtime type is "api", skip; otherwise drive_session()
5. Once a runtime responds with a result (even error/fail), that's final — no further fallback.

If all runtimes exhausted, return Error "no reachable runtime".

SPEC-EX-LV-020: runtime-config-lookup
  Each runtime name in the list is looked up in config.runtimes. If not found, returns Status::Error (should not happen if validate_config ran).
  test: UNTESTED

SPEC-EX-LV-021: adapter-creation-failure-tries-next
  If create_adapter fails for a runtime, a warning is logged and the next runtime is tried.
  test: UNTESTED

SPEC-EX-LV-022: health-check-failure-tries-next
  If health_check returns unreachable, a warning is logged and the next runtime is tried.
  test: UNTESTED

SPEC-EX-LV-023: query-mode-calls-post-completion
  In query mode, builds a CompletionRequest from the resolved prompt, model, temperature, max_tokens, and system_prompt, then calls adapter.post_completion().
  test: UNTESTED

SPEC-EX-LV-024: query-mode-not-supported-tries-next
  If post_completion returns RuntimeError "not supported", the next runtime is tried.
  test: UNTESTED

SPEC-EX-LV-025: query-mode-parses-result
  On successful completion, parses the result using verdict or freeform response format, same as the old execute_llm_completion.
  test: UNTESTED

SPEC-EX-LV-026: session-mode-skips-api-runtime
  In session mode, if the runtime type is "api", skip it and try next.
  test: UNTESTED

SPEC-EX-LV-027: session-mode-drives-session
  In session mode with a non-api runtime, builds SessionConfig and calls drive_session().
  test: UNTESTED

SPEC-EX-LV-028: all-runtimes-exhausted-errors
  If all runtimes in the list are exhausted (unreachable, unsupported, or skipped), returns Status::Error "[baton] No reachable runtime for validator 'X'".
  test: UNTESTED

SPEC-EX-LV-029: model-resolution-chain
  Model comes from: validator.model → runtime_config.default_model → "default".
  test: UNTESTED

SPEC-EX-LV-030: cost-propagated
  Cost from CompletionResult or SessionResult is propagated to ValidatorResult.
  test: UNTESTED

---

## drive_session

Core session orchestration: create → poll → collect → teardown → parse verdict. Extracted from execute_llm_validator for testability.

SPEC-EX-DS-001: create-failure-errors-no-teardown
  If adapter.create_session fails, returns Status::Error. Teardown is NOT called because no session was created.
  test: exec::tests::session_create_error

SPEC-EX-DS-002: poll-loop-sleeps-2-seconds
  The poll loop sleeps 2 seconds between each poll_status call.
  test: UNTESTED (timing not asserted)

SPEC-EX-DS-003: poll-timeout-cancels-and-tears-down
  If polling exceeds timeout_seconds, the session is cancelled and torn down, and Status::Error "timed out" is returned.
  test: exec::tests::session_timeout_cancels_and_tears_down

SPEC-EX-DS-004: poll-error-cancels-and-tears-down
  If poll_status returns an error, the session is cancelled and torn down.
  test: UNTESTED (MockRuntimeAdapter doesn't support poll errors currently)

SPEC-EX-DS-005: completed-status-proceeds-to-collect
  When poll returns Completed, the loop exits and collect_result is called.
  test: exec::tests::session_completes_pass

SPEC-EX-DS-006: failed-completed-timedout-cancelled-all-exit-loop
  Any terminal status (Completed, Failed, TimedOut, Cancelled) exits the poll loop.
  test: IMPLICIT via session_failed_status, session_timed_out_status, session_cancelled_status

SPEC-EX-DS-007: collect-error-tears-down
  If collect_result fails, teardown is called and Status::Error returned.
  test: exec::tests::session_collect_error_tears_down

SPEC-EX-DS-008: teardown-always-called-on-success
  After successful collection, teardown is called exactly once. Cancel is NOT called.
  test: exec::tests::session_teardown_always_called_on_success

SPEC-EX-DS-009: non-completed-session-returns-error
  If the session ends with Failed, TimedOut, or Cancelled status, Status::Error is returned with an appropriate message. Cost is preserved even on failure.
  test: exec::tests::session_failed_status, session_timed_out_status, session_cancelled_status

SPEC-EX-DS-010: empty-output-errors
  If a completed session has empty output (whitespace-only), returns Status::Error "no PASS/FAIL/WARN verdict".
  test: exec::tests::session_empty_output

SPEC-EX-DS-011: output-parsed-as-verdict
  Completed session output is parsed via parse_verdict(). Status and evidence become the result.
  test: exec::tests::session_completes_pass, session_completes_fail, session_completes_warn

SPEC-EX-DS-012: unparseable-output-errors
  If output cannot be parsed as a verdict, Status::Error with "Could not parse verdict" is returned.
  test: exec::tests::session_unparseable_output

SPEC-EX-DS-013: cost-propagated-from-session
  Cost from SessionResult is propagated to ValidatorResult, on both success and failure paths.
  test: exec::tests::session_cost_propagated, session_cost_on_failure

SPEC-EX-DS-014: validator-name-propagated
  The name parameter is used as the ValidatorResult.name.
  test: exec::tests::session_validator_name_propagated

---

Note: extract_cost has been moved to the provider module. See spec/provider.md SPEC-PV-EC-*.

---

## run_gate

Runs all validators in a gate's pipeline and returns a Verdict.

This is the main entry point for gate execution. It validates inputs, runs each validator in order, respects filtering and blocking semantics, and computes the final verdict status.

### Sections

1. Pre-flight checks (input validation, hashing)
2. Execution loop (filter, run_if, dispatch, blocking)
3. Finalization (compute status, build verdict)

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

SPEC-EX-RG-017: suppression-applied-before-blocking-check
  Status suppression (suppress_warnings, suppress_errors) is applied before the blocking check. A validator that returns Error with blocking=true will not stop the pipeline if Error is suppressed. The suppressed status is treated as Pass for blocking purposes.
  test: exec::tests::gate_suppress_errors

SPEC-EX-RG-018: warn-validators-tracked-in-warnings-list
  When a validator returns Status::Warn, its name is added to the verdict's warnings list.
  test: exec::tests::gate_warn_from_script

SPEC-EX-RG-019: history-records-true-status
  The verdict history records each validator's true status, not the suppressed status. This lets consumers distinguish "passed because suppressed" from "genuinely passed".
  test: exec::tests::gate_suppress_errors (asserts b_result.status is Fail despite suppression)

### run_gate: finalization

After the execution loop completes (either all validators ran, or a blocking failure stopped early), run_gate assembles the Verdict.

SPEC-EX-RG-020: normal-mode-pass-on-completion
  In normal mode (run_all=false), if the execution loop completes without a blocking failure, the verdict status is Pass. Non-blocking failures do not affect the final status. This is intentional: non-blocking validators are advisory. Their results appear in history but don't fail the gate.
  test: exec::tests::gate_non_blocking_failure_passes

SPEC-EX-RG-021: run-all-mode-uses-compute-final-status
  In run_all mode, the final status is computed by compute_final_status() over all results. Error beats Fail beats Pass. Skip and Warn are not considered.
  test: exec::tests::gate_all_mode_non_blocking_failure_counts

SPEC-EX-RG-022: verdict-history-contains-all-results
  The verdict.history field contains every validator result, including skipped validators. Order matches the BTreeMap iteration order (alphabetical by name), not the gate config order.

Note: this is a subtle consequence of using BTreeMap<String, ValidatorResult> for the results collection. The config order is preserved during execution, but the final .values().collect() produces alphabetical order. This should be considered if order-sensitive output is needed.

  test: IMPLICIT via exec::tests::gate_all_pass

SPEC-EX-RG-023: failed-at-set-on-blocking-failure
  When a blocking validator fails, verdict.failed_at is set to the validator's name and verdict.feedback is set to the validator's feedback.
  test: exec::tests::gate_first_fail_blocks

SPEC-EX-RG-024: failed-at-set-in-run-all-mode
  In run_all mode, if the final status is Fail or Error, failed_at is set to the first validator with the determining status. For Error, it finds the first Error result; for Fail, the first Fail result.
  test: exec::tests::gate_error_vs_fail_in_all_mode

SPEC-EX-RG-025: suppression-in-run-all-mode
  In run_all mode, suppression affects compute_final_status. Suppressing Error with a Fail present produces VerdictStatus::Fail. Suppressing all produces VerdictStatus::Pass.
  test: exec::tests::gate_suppress_errors_with_all_mode, gate_suppress_all_in_all_mode

SPEC-EX-RG-026: llm-validator-in-gate
  LLM validators work within run_gate, using the config's runtimes for execution. The LLM validator receives its prompt input from the `Invocation`'s input files, not from a single artifact + context.
  test: exec::tests::llm_completion_in_gate_run

SPEC-EX-RG-027: llm-fail-blocks-gate
  A blocking LLM validator that returns FAIL stops the pipeline, same as a script validator.
  test: exec::tests::llm_completion_fail_blocks_gate

---

## File collector

Builds the input pool. All positional args are files or directories. Directories are walked recursively. `--diff` and `--files` add more paths. The pool is deduplicated by canonical path. This is the entry point — everything downstream operates on this pool.

SPEC-EX-FC-001: positional-args-populate-pool
  File and directory paths from positional CLI args are added to the input pool. Directories are walked recursively by default.
  test: exec::tests::file_collector_single_file
  test: exec::tests::file_collector_directory_walk

SPEC-EX-FC-002: diff-flag-adds-changed-files
  `--diff <refspec>` runs `git diff --name-only <refspec>` and adds the result to the pool.
  test: TODO

SPEC-EX-FC-003: files-flag-reads-file-list
  `--files <path | ->` reads newline-separated paths from a file or stdin.
  test: exec::tests::file_collector_reads_file_list

SPEC-EX-FC-004: deduplication-by-canonical-path
  The pool is deduplicated by canonical (absolute, symlink-resolved) path.
  test: exec::tests::file_collector_deduplication

SPEC-EX-FC-005: no-recursive-flag
  `--no-recursive` disables recursive directory walking.
  test: TODO

---

## Dispatch planner

Turns the file pool into invocations. For each validator, the planner examines its `input` declaration and matches files from the pool. The result is zero or more Invocation objects per validator — each one a concrete "run this validator with these specific files."

The planner is where the four input forms (no-input, per-file, batch, multi-input) become concrete. A validator never sees the pool directly; it sees only the files the planner selected for its invocation.

SPEC-EX-DP-001: no-input-produces-single-invocation
  A validator with no `input` field produces exactly one invocation with no files.
  test: exec::tests::dispatch_no_input_produces_single_invocation

SPEC-EX-DP-002: per-file-produces-one-invocation-per-match
  A per-file input produces one invocation per file matching the glob against the pool.
  test: exec::tests::dispatch_per_file_produces_one_per_match

SPEC-EX-DP-003: batch-produces-single-invocation
  A batch input (`collect = true`) produces one invocation with all matching files.
  test: exec::tests::dispatch_batch_produces_single_invocation

SPEC-EX-DP-004: keyed-inputs-joined-by-key
  Named inputs with `key` expressions are grouped by matching key values. One invocation per distinct key.
  test: exec::tests::dispatch_keyed_inputs_grouped_by_key

SPEC-EX-DP-005: incomplete-group-skips-with-warning
  If a key value appears in one input slot but not another, the group is skipped and a warning is emitted.
  test: TODO

SPEC-EX-DP-006: fixed-inputs-injected-into-every-invocation
  An input with `path` (fixed) is present in every invocation, regardless of key grouping.
  test: TODO

SPEC-EX-DP-007: no-matching-files-skips-validator
  If a validator requires file input but no files in the pool match, the validator is skipped with a warning.
  test: exec::tests::dispatch_no_matching_files_produces_empty

---

## Execution pipeline

Gate-level orchestration. After the file collector and dispatch planner have done their work, this layer iterates gates, applies `--only`/`--skip` filtering, runs validators in pipeline order, and enforces blocking and `run_if` rules. This is where stateless validators meet sequential orchestration.

SPEC-EX-PL-001: gates-iterated-after-only-skip-filtering
  Gates are filtered by `--only` / `--skip` before iteration.
  test: TODO

SPEC-EX-PL-002: per-invocation-blocking
  If any invocation of a blocking validator fails, the gate stops. Not just one invocation — any.
  test: exec::tests::pipeline_per_invocation_blocking

SPEC-EX-PL-003: per-invocation-verdict-recorded
  Each invocation produces a separate `ValidatorResult` with its group key and input file hashes.
  test: TODO
