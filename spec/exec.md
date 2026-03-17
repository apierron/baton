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
| `drive_session`              | `execute_llm_session`|
| `evaluate_atom`              | `evaluate_run_if`    |

## Design notes

run_gate is intentionally a single function rather than being broken into pre-flight/execute/finalize methods. The pipeline is sequential and every step can fail, so early returns dominate the control flow. Splitting it would just move the returns into the callers.

execute_validator takes Option<&BatonConfig> rather than &BatonConfig because script and human validators don't need config. This lets tests run script validators without constructing a full config. LLM validators return Status::Error immediately if config is None.

drive_session is extracted from execute_llm_session so the session lifecycle (create → poll → collect → teardown → parse verdict) can be tested independently via MockRuntimeAdapter, without needing HTTP or config resolution.

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
    Llm with mode=Completion → execute_llm_completion
    Llm with mode=Session → execute_llm_session
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
  The {artifact}, {artifact_content}, {context.X}, etc. placeholders are resolved in the command string before execution.
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

## execute_llm_completion

Resolves provider and prompt, builds the request body, delegates the HTTP call to `provider::ProviderClient::post_completion()`, and maps the response or error to a `ValidatorResult`.

### Sections

1. Config and provider resolution
2. ProviderClient construction
3. Prompt resolution (file or inline) and placeholder substitution
4. Request body construction
5. Response parsing and verdict extraction

SPEC-EX-LC-001: no-config-errors
  If config is None, returns Status::Error "[baton] LLM validator requires config with provider settings".
  test: exec::tests::llm_completion_no_config

SPEC-EX-LC-002: missing-provider-errors
  If the validator's provider is not defined in config.providers, returns Status::Error with "not defined in [providers]".
  test: exec::tests::llm_completion_missing_provider

SPEC-EX-LC-003: api-key-env-not-set-errors
  If ProviderClient::new returns ApiKeyNotSet, returns Status::Error with the formatted error.
  test: UNTESTED (would require env var manipulation during test)

SPEC-EX-LC-004: empty-api-key-env-skips-auth
  If api_key_env is empty, no Authorization header is sent. This allows providers that don't require authentication (e.g., local models).
  test: IMPLICIT via mock server tests that don't set api_key_env

SPEC-EX-LC-005: prompt-file-resolution
  If the prompt value is a file reference (has .md/.txt/.prompt/.j2 extension), it is loaded via resolve_prompt_value from prompts_dir or config_dir. If not found, returns Status::Error.
  test: UNTESTED (no test uses file-based prompts for LLM validators)

SPEC-EX-LC-006: prompt-placeholders-resolved
  Placeholders in the prompt body are resolved before sending to the LLM.
  test: exec::tests::llm_completion_with_placeholders

SPEC-EX-LC-007: model-falls-back-to-provider-default
  If the validator has no explicit model, the provider's default_model is used.
  test: exec::tests::llm_completion_uses_default_model

SPEC-EX-LC-008: system-prompt-sent-as-system-message
  If system_prompt is set, it is sent as a system-role message before the user message. Placeholders in the system prompt are also resolved.
  test: exec::tests::llm_completion_with_system_prompt

SPEC-EX-LC-009: max-tokens-included-when-set
  If max_tokens is set, it is included in the request body. Otherwise, the field is omitted.
  test: UNTESTED (no test asserts max_tokens in request)

SPEC-EX-LC-010: http-timeout-uses-validator-timeout
  The HTTP client timeout is set to the validator's timeout_seconds.
  test: UNTESTED

SPEC-EX-LC-011: unreachable-provider-errors
  If the provider cannot be reached (connection error), returns Status::Error with "Cannot reach provider".
  HTTP error classification is performed by ProviderClient::classify_http_error. The exec module maps ProviderError variants to "[baton]" prefixed feedback strings.
  test: exec::tests::llm_completion_unreachable_provider

SPEC-EX-LC-012: timeout-error-distinguished
  If the request times out (e.is_timeout()), returns Status::Error with "Validator timed out after N seconds" rather than the generic connection error.
  HTTP error classification is performed by ProviderClient::classify_http_error.
  test: UNTESTED (would require a slow mock server)

SPEC-EX-LC-013: http-401-403-auth-error
  HTTP 401 or 403 returns Status::Error with "Authentication failed".
  HTTP error classification is performed by ProviderClient::classify_http_error.
  test: exec::tests::llm_completion_http_401

SPEC-EX-LC-014: http-404-model-not-found
  HTTP 404 returns Status::Error with "Model 'X' not found on provider 'Y'".
  HTTP error classification is performed by ProviderClient::classify_http_error.
  test: exec::tests::llm_completion_http_404

SPEC-EX-LC-015: http-429-rate-limited
  HTTP 429 returns Status::Error with "Rate limited by provider".
  HTTP error classification is performed by ProviderClient::classify_http_error.
  test: exec::tests::llm_completion_http_429

SPEC-EX-LC-016: http-5xx-generic-error
  Other HTTP errors return Status::Error with "Provider returned HTTP {code}: {body}".
  HTTP error classification is performed by ProviderClient::classify_http_error.
  test: exec::tests::llm_completion_http_500

SPEC-EX-LC-017: malformed-json-response-errors
  If the response body cannot be parsed as JSON, returns Status::Error with "empty or malformed response".
  test: UNTESTED (would require a mock returning non-JSON)

SPEC-EX-LC-018: empty-content-errors
  If the response has no content (choices[0].message.content is empty or missing), returns Status::Error with "empty or malformed response". Cost is still extracted and returned.
  test: exec::tests::llm_completion_empty_response

SPEC-EX-LC-019: verdict-format-parses-content
  With response_format=Verdict, the content is parsed via parse_verdict(). The parsed status and evidence become the result status and feedback.
  test: exec::tests::llm_completion_pass_verdict, llm_completion_fail_verdict, llm_completion_warn_verdict

SPEC-EX-LC-020: unparseable-verdict-errors
  If the content cannot be parsed as a verdict (no PASS/FAIL/WARN keyword found), the result is Status::Error from parse_verdict with "Could not parse verdict" feedback.
  test: exec::tests::llm_completion_unparseable_verdict

SPEC-EX-LC-021: freeform-always-returns-warn
  With response_format=Freeform, the result is always Status::Warn with the full content as feedback. No verdict parsing occurs.

This is intentional: freeform validators are advisory. They collect LLM commentary without making a pass/fail determination. Use verdict format for gate-affecting results.

  test: exec::tests::llm_completion_freeform_returns_warn

SPEC-EX-LC-022: cost-extracted-from-usage
  If the response includes a `usage` object with prompt_tokens and/or completion_tokens, a Cost is attached to the result. The model name is always set to the model used for the request.
  test: exec::tests::llm_completion_cost_tracking

SPEC-EX-LC-023: no-usage-means-no-cost
  If the response has no `usage` object, cost is None.
  test: exec::tests::llm_completion_no_usage_in_response

---

## execute_llm_session

Orchestrates an agent session via a RuntimeAdapter. Resolves config, creates the adapter, prepares the session config, then delegates to drive_session.

SPEC-EX-LS-001: no-config-errors
  If config is None, returns Status::Error.
  test: exec::tests::llm_session_no_config

SPEC-EX-LS-002: missing-runtime-field-errors
  If the validator has mode=Session but no runtime field, returns Status::Error.
  test: exec::tests::llm_session_missing_runtime

SPEC-EX-LS-003: undefined-runtime-errors
  If the runtime name is not defined in config.runtimes, returns Status::Error.
  test: exec::tests::llm_session_undefined_runtime

SPEC-EX-LS-004: adapter-creation-failure-errors
  If create_adapter fails, returns Status::Error.
  test: UNTESTED (would require an invalid runtime config that passes parse but fails adapter creation)

SPEC-EX-LS-005: files-include-artifact-and-context-refs
  The session's file set includes the artifact path (if file-backed) and any context items referenced by context_refs that are file-backed.
  test: UNTESTED (no test asserts file set contents)

SPEC-EX-LS-006: model-resolution-chain
  Model comes from: validator.model → runtime_config.default_model → "default".
  test: UNTESTED

SPEC-EX-LS-007: sandbox-and-iterations-from-validator-or-runtime
  sandbox comes from validator.sandbox or runtime_config.sandbox. max_iterations comes from validator.max_iterations or runtime_config.max_iterations.
  test: UNTESTED

---

## drive_session

Core session orchestration: create → poll → collect → teardown → parse verdict. Extracted from execute_llm_session for testability.

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

### run_gate: pre-flight checks

Before executing any validators, run_gate validates that the inputs are well-formed. These checks run in a fixed order. The first failure returns immediately — there is no accumulation of pre-flight errors, because downstream checks may depend on earlier ones succeeding (e.g., hash computation requires a readable file).

Required-context checking happens here, not in validate_config(), because context is provided at runtime via CLI args, not in baton.toml. The config validator checks that context *slots* are well-formed but cannot know whether the user will supply them at invocation time.

Edge cases to consider:

- Artifact is from_string (no path) — path checks are skipped entirely
- Context item whose path doesn't exist vs. context item with inline string
- The ordering of checks matters: a nonexistent path should get ArtifactNotFound, not ArtifactIsDirectory

SPEC-EX-RG-001: artifact-path-must-exist
  When the artifact is file-backed (artifact.path is Some) and the path does not exist on disk, run_gate returns Err(ArtifactNotFound) containing the path string. When the artifact is from_string (path is None), this check is skipped entirely.
  test: exec::tests::run_gate_artifact_not_found

SPEC-EX-RG-002: artifact-path-rejects-directory
  When artifact.path points to a directory, run_gate returns Err(ArtifactIsDirectory). This check runs after existence, so a nonexistent path always gets ArtifactNotFound, never ArtifactIsDirectory.
  test: exec::tests::run_gate_artifact_is_directory

SPEC-EX-RG-003: required-context-enforced
  For each context slot in gate.context where required=true, if the provided context map does not contain that key, run_gate returns Err(MissingRequiredContext) naming both the slot and the gate. Only the first missing context triggers the error (early return).
  test: exec::tests::gate_required_context_missing

SPEC-EX-RG-004: unexpected-context-warns-not-errors
  For each key in the provided context that is not declared in gate.context, a warning is printed to stderr. This is not an error; execution continues. The warning includes the item name and gate name. This allows forward-compatible context (passing extra items that a future version of the gate might use).
  test: UNTESTED

SPEC-EX-RG-005: context-path-must-exist
  For each context item backed by a file path, if the path does not exist, run_gate returns Err(ContextNotFound) with the item name and path. If the path is a directory, returns Err(ContextIsDirectory). Context items with inline string content skip this check.
  test: exec::tests::run_gate_context_not_found
  test: exec::tests::run_gate_context_is_directory

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

SPEC-EX-RG-026: llm-completion-in-gate
  LLM completion validators work within run_gate, using the config's provider for HTTP calls.
  test: exec::tests::llm_completion_in_gate_run

SPEC-EX-RG-027: llm-fail-blocks-gate
  A blocking LLM validator that returns FAIL stops the pipeline, same as a script validator.
  test: exec::tests::llm_completion_fail_blocks_gate
