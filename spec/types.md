# module: types

Core data types for baton: input files, invocations, verdicts, and run options.

This module defines the value-layer types that every other module depends on. It sits at the bottom of the dependency graph. The key abstractions follow the three-layer composition model: InputFile (a file from the pool), Invocation (a single run of a stateless validator against specific files), GateResult/InvocationResult (orchestration outcomes), Verdict (backward-compatible gate-level output), Status/VerdictStatus (validator-level vs gate-level outcomes), ValidatorResult, and RunOptions.

## Public types and functions

| Type/Function                  | Purpose                                                 |
|--------------------------------|---------------------------------------------------------|
| `InputFile`                    | A file from the input pool: path, lazy content, lazy hash |
| `InputFile::get_content`       | Lazy content loading (reads file on first access)       |
| `InputFile::get_hash`          | SHA-256 hash, computed and cached on first call         |
| `Invocation`                   | Validator name + group key + input files map            |
| `Status`                       | 5-variant enum: Pass, Fail, Warn, Skip, Error           |
| `VerdictStatus`                | 3-variant enum: Pass, Fail, Error                       |
| `ValidatorResult`              | Single validator outcome with name, status, feedback    |
| `GateResult`                   | Gate-level result with status and validator results     |
| `InvocationResult`             | Top-level result with gate results and duration         |
| `Verdict`                      | Gate-level result with history and output formatters    |
| `RunOptions`                   | Runtime options for filtering and reporting              |

## Design notes

InputFile uses lazy content and hash loading. Content is not read from disk until first access, and the hash is not computed until first access. Both are cached after first computation so subsequent accesses return the same value even if the underlying file changes on disk. This is intentional: the hash captures the file state at validation time.

Status is the single status type used at every level: validators, gates, and the top-level invocation. All five variants (Pass, Fail, Warn, Skip, Error) are valid everywhere. A gate can be skipped (e.g., filtered out by `--skip`) and can produce a warning (e.g., all validators passed but some warned). VerdictStatus exists for backward compatibility with v1 output format but should be phased out — it is a lossy reduction of Status to three values (Pass, Fail, Error) and loses skip/warn information.

Status::FromStr only accepts exact lowercase strings. This prevents ambiguity (e.g., "PASS" vs "Pass" vs "pass") and matches the serde representation. The error message includes the rejected value to aid debugging.

`to_human` intentionally hides feedback for Pass validators. When a gate has many validators, showing "all good" messages for every passing check creates visual noise. Only failures, warnings, and errors get their feedback displayed. Feedback is also truncated to 5 lines to keep terminal output manageable.

RunOptions::new() enables logging while Default (derived) disables it. The `new()` constructor is used by the CLI where logging is expected. The Default trait is used by tests and programmatic callers who typically don't want SQLite side effects.

---

## InputFile

Represents a file from the input pool. Content and hash are loaded lazily and cached.

SPEC-TY-IF-001: fields
  An InputFile tracks its filesystem path, its text content (lazy), and its SHA-256 hash (lazy). Construction records the path without reading the file.
  test: types::tests::input_file_fields

SPEC-TY-IF-002: lazy-content-loading
  Content is loaded on first access via `get_content()`, not at construction time.
  test: types::tests::input_file_lazy_content_loading

SPEC-TY-IF-003: lazy-hash-computation
  Hash is computed on first access via `get_hash()`. Returns SHA-256 as lowercase hex. Cached after first computation.
  test: types::tests::input_file_lazy_hash_computation

---

## Invocation

A planned execution of a single stateless validator against a specific set of input files. The dispatch planner produces one Invocation per "run" of a validator — one per matching file for per-file validators, one total for batch validators, one per key-group for multi-input validators.

SPEC-TY-IN-001: fields
  An Invocation identifies which validator to run, an optional group key (the matched key value for multi-input validators, absent for no-input/batch), and a map of named input slots to their files.
  test: types::tests::invocation_fields
  test: types::tests::invocation_with_input_files

---

## GateResult

Result of running all validators in a single gate. Gates are orchestration — they don't touch files, they sequence validators and reduce their statuses to a single verdict.

SPEC-TY-GR-001: fields
  A GateResult records which gate ran, its overall status (any of pass/fail/warn/skip/error), all individual validator results (in pipeline order), and the total wall-clock duration. A gate can be skipped (filtered by `--skip`) or produce a warning (all validators passed but some warned).
  test: types::tests::gate_result_fields

---

## InvocationResult

Top-level result of a single `baton check` invocation. Contains the results of all gates that ran.

SPEC-TY-IR-001: fields
  An InvocationResult has a unique ID, the list of gate results (in execution order), and the total wall-clock duration of the entire invocation.
  test: types::tests::invocation_result_fields

---

## Status

Five-variant enum representing the outcome of a single validator: Pass, Fail, Warn, Skip, Error.

SPEC-TY-ST-001: display-is-lowercase
  `Status::Display` renders each variant as its lowercase name: "pass", "fail", "warn", "skip", "error".
  test: types::tests::status_display

SPEC-TY-ST-002: from-str-accepts-exact-lowercase
  `Status::FromStr` accepts exactly the five lowercase strings: "pass", "fail", "warn", "skip", "error". Each maps to the corresponding variant.
  test: types::tests::status_from_str

SPEC-TY-ST-003: from-str-rejects-non-lowercase
  Any casing other than all-lowercase is rejected. "Pass", "FAIL", "Error" all return Err. This is intentional — it prevents ambiguity and matches the serde representation.
  test: types::tests::status_from_str_rejects_uppercase

SPEC-TY-ST-004: from-str-error-message-includes-value
  The Err string from `FromStr` contains the invalid input value, formatted as `Invalid status: '<value>'`.
  test: types::tests::status_from_str_error_includes_value

SPEC-TY-ST-005: serde-uses-lowercase
  Serialization and deserialization use `#[serde(rename_all = "lowercase")]`, matching the Display/FromStr representations.
  test: IMPLICIT via types::tests::verdict_to_json_roundtrip (Status serialized as part of ValidatorResult)

SPEC-TY-ST-006: status-is-copy
  Status derives Copy, allowing it to be passed by value without cloning.
  test: UNTESTED (structural property, not behavioral)

---

## VerdictStatus

**Deprecated.** Three-variant enum (Pass, Fail, Error) from v1. Exists for backward compatibility with the Verdict output format and exit code mapping. New code should use `Status` everywhere — gates and validators share the same five-variant status.

The exit code mapping (Pass=0, Fail=1, Error=2) remains useful for the CLI. Warn maps to exit 0 (pass with caveats), Skip maps to exit 0 (nothing to do).

SPEC-TY-VS-001: exit-code-mapping
  `exit_code()` maps Pass to 0, Fail to 1, Error to 2. These are used as process exit codes by the CLI.
  test: types::tests::verdict_status_exit_codes

SPEC-TY-VS-002: display-is-lowercase
  `VerdictStatus::Display` renders each variant as its lowercase name: "pass", "fail", "error".
  test: types::tests::verdict_status_display

SPEC-TY-VS-003: serde-uses-lowercase
  Serialization and deserialization use `#[serde(rename_all = "lowercase")]`.
  test: IMPLICIT via types::tests::verdict_json_roundtrip_full

---

## ValidatorResult

Outcome of a single validator run. A plain data struct with no methods.

| Field        | Type             | Description                                     |
|--------------|------------------|-------------------------------------------------|
| `name`       | `String`         | Validator name from config                      |
| `status`     | `Status`         | Outcome status                                  |
| `feedback`   | `Option<String>` | Human-readable feedback text, if any            |
| `duration_ms`| `i64`            | Wall-clock execution time in milliseconds       |
| `cost`       | `Option<Cost>`   | LLM token/cost metadata, None for non-LLM       |

SPEC-TY-VR-001: serializable-and-deserializable
  ValidatorResult derives Serialize and Deserialize for JSON roundtripping as part of Verdict.
  test: IMPLICIT via types::tests::verdict_json_roundtrip_full

SPEC-TY-VR-002: cost-field-optional
  The `cost` field is None for script and human validators. When present (LLM validators), it contains token counts, model name, and estimated USD cost.
  test: types::tests::verdict_json_roundtrip_full

---

## Cost

Token usage and cost metadata from an LLM validator call. A plain data struct with no methods. All fields are optional.

| Field           | Type             | Description                          |
|-----------------|------------------|--------------------------------------|
| `input_tokens`  | `Option<i64>`    | Number of input tokens consumed      |
| `output_tokens` | `Option<i64>`    | Number of output tokens generated    |
| `model`         | `Option<String>` | Model identifier                     |
| `estimated_usd` | `Option<f64>`    | Estimated cost in USD                |

SPEC-TY-CO-001: default-is-all-none
  Cost derives Default, which sets all fields to None.
  test: UNTESTED (structural property)

SPEC-TY-CO-002: serializable-and-deserializable
  Cost derives Serialize and Deserialize. When serialized as part of a ValidatorResult, None fields are included as JSON null.
  test: types::tests::verdict_json_roundtrip_full

---

## Verdict

Final gate-level result containing all validator outcomes and aggregate metadata.

| Field            | Type                   | Description                                    |
|------------------|------------------------|------------------------------------------------|
| `status`         | `VerdictStatus`        | Gate-level pass/fail/error                     |
| `gate`           | `String`               | Gate name from config                          |
| `failed_at`      | `Option<String>`       | Name of first failing validator, if any        |
| `feedback`       | `Option<String>`       | Feedback from the failing validator            |
| `duration_ms`    | `i64`                  | Total gate execution time                      |
| `timestamp`      | `DateTime<Utc>`        | When the verdict was produced                  |
| `warnings`       | `Vec<String>`          | Non-fatal warnings collected during execution  |
| `suppressed`     | `Vec<String>`          | Names of status-suppressed validators          |
| `history`        | `Vec<ValidatorResult>` | All validator results in pipeline order        |

### Verdict: to_json

SPEC-TY-VD-001: to-json-produces-pretty-printed-json
  `to_json` serializes the entire Verdict struct as pretty-printed JSON using `serde_json::to_string_pretty`. The output is valid JSON that can be deserialized back into a Verdict.
  test: types::tests::verdict_to_json_roundtrip

SPEC-TY-VD-002: to-json-preserves-all-fields
  All Verdict fields survive a JSON roundtrip, including nested structures like Cost within ValidatorResult, optional fields (Some and None), and vector fields (warnings, suppressed, history).
  test: types::tests::verdict_json_roundtrip_full

SPEC-TY-VD-003: to-json-panics-on-serialization-failure
  `to_json` calls `expect` on the serialization result. In practice this never fails because all fields are serializable, but the contract is panic rather than Result.
  test: UNTESTED (unreachable in practice)

### Verdict: to_human

SPEC-TY-VD-010: to-human-shows-status-icons
  Each validator result is prefixed with a status icon: Pass=checkmark (U+2713), Fail=ballot-x (U+2717), Warn=exclamation, Skip=em-dash (U+2014), Error=letter-E.
  test: types::tests::verdict_to_human_all_status_icons

SPEC-TY-VD-011: to-human-shows-skip-label
  Skipped validators display "(skipped)" after the validator name, in addition to the em-dash icon.
  test: types::tests::verdict_to_human_skip_label

SPEC-TY-VD-012: to-human-shows-duration
  Each validator line includes the duration in the format "(Nms)" after the name (and skip label, if present).
  test: types::tests::verdict_to_human

SPEC-TY-VD-013: to-human-hides-pass-feedback
  When a validator has Status::Pass, its feedback is not displayed even if present. This is intentional to reduce visual noise — passing validators don't need explanation.
  test: types::tests::verdict_to_human_pass_feedback_not_shown

SPEC-TY-VD-014: to-human-shows-feedback-for-non-pass
  When a validator has Status::Fail, Warn, Skip, or Error and has feedback, the feedback is displayed indented under the validator line.
  test: types::tests::verdict_to_human

SPEC-TY-VD-015: to-human-truncates-feedback-to-5-lines
  Feedback is truncated to the first 5 lines. Lines 6 and beyond are silently dropped. No truncation indicator is added.
  test: types::tests::verdict_to_human_feedback_truncated_to_5_lines

SPEC-TY-VD-016: to-human-feedback-indented
  Each line of feedback is indented with 4 spaces.
  test: IMPLICIT via types::tests::verdict_to_human

SPEC-TY-VD-017: to-human-ends-with-verdict-line
  The final line is formatted as "  VERDICT: {STATUS}" with the status in uppercase. If `failed_at` is present, it appends " (failed at: {name})".
  test: types::tests::verdict_to_human

SPEC-TY-VD-018: to-human-lines-joined-with-newline
  All lines are joined with "\n". There is no trailing newline.
  test: types::tests::verdict_to_human_no_trailing_newline

### Verdict: to_summary

SPEC-TY-VD-020: to-summary-pass-returns-bare-pass
  When status is Pass, `to_summary` returns the literal string "PASS" with no additional information.
  test: types::tests::verdict_to_summary_pass

SPEC-TY-VD-021: to-summary-fail-includes-failed-at-and-feedback
  When status is Fail, `to_summary` returns "FAIL at {failed_at}: {feedback}". The status is uppercased.
  test: types::tests::verdict_to_summary_fail

SPEC-TY-VD-022: to-summary-error-includes-failed-at-and-feedback
  When status is Error, `to_summary` returns "ERROR at {failed_at}: {feedback}". Error and Fail follow the same format with different prefixes.
  test: types::tests::verdict_to_summary_error

SPEC-TY-VD-023: to-summary-uses-first-line-of-multiline-feedback
  When feedback contains multiple lines, only the first line is included in the summary.
  test: types::tests::verdict_to_summary_multiline_feedback_uses_first_line

SPEC-TY-VD-024: to-summary-omits-colon-when-no-feedback
  When feedback is None or empty, the ": {feedback}" suffix is omitted entirely. The output is "FAIL at {name}" with no trailing colon or space.
  test: types::tests::verdict_to_summary_fail_no_feedback

SPEC-TY-VD-025: to-summary-uses-unknown-when-failed-at-is-none
  When `failed_at` is None (which should not happen for Fail/Error verdicts in practice), the string "unknown" is used as the validator name.
  test: types::tests::verdict_to_summary_fail_no_failed_at_uses_unknown

---

## RunOptions

Runtime options controlling which validators to run and how results are reported.

| Field                | Type                 | Default (new) | Default (Default) |
|----------------------|----------------------|---------------|--------------------|
| `run_all`            | `bool`               | false         | false              |
| `only`               | `Option<Vec<String>>`| None          | None               |
| `skip`               | `Option<Vec<String>>`| None          | None               |
| `timeout`            | `Option<u64>`        | None          | None               |
| `log`                | `bool`               | true          | false              |
| `suppressed_statuses`| `Vec<Status>`        | empty         | empty              |

SPEC-TY-RO-001: new-enables-logging
  `RunOptions::new()` sets `log` to true. All other fields are at their Default values (false, None, empty vec).
  test: types::tests::run_options_new_enables_logging

SPEC-TY-RO-002: default-disables-logging
  `RunOptions::default()` (via derived Default) sets `log` to false. This is the only difference from `new()`.
  test: types::tests::run_options_default_disables_logging

SPEC-TY-RO-003: new-sets-all-other-fields-to-default
  `RunOptions::new()` sets `run_all` to false, `only`/`skip`/`tags`/`timeout` to None, and `suppressed_statuses` to empty vec.
  test: types::tests::run_options_new_enables_logging
