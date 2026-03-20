# module: placeholder

Template placeholder resolution. Substitutes placeholders in command strings and prompt templates using the invocation's input files and prior validator results. Bare references (`{file}`, `{input}`, `{input.<name>}`) resolve to file content. Dotted variants (`{file.path}`, `{input.paths}`, `{input.<name>.path}`) resolve to paths or other metadata. Also provides environment variable interpolation for config strings.

This module is intentionally lenient: most resolution failures produce warnings and empty strings rather than errors. The rationale is that placeholder resolution happens inside validators that are already running — aborting the entire gate because of a missing input reference would be more disruptive than emitting an empty string and letting the validator produce a meaningful failure on its own terms.

## Public functions

| Function               | Purpose                                                   |
|------------------------|-----------------------------------------------------------|
| `resolve_placeholders` | Resolve all placeholders in a template string             |
| `resolve_env_vars`     | Resolve `${VAR}` and `${VAR:-default}` in config strings  |

## Internal functions

| Function              | Called by              |
|-----------------------|------------------------|
| `find_closing_brace`  | `resolve_placeholders` |
| `resolve_single`      | `resolve_placeholders` |

## Design notes

resolve_placeholders takes the invocation's input files. Resolving `{file}`, `{input}`, and `{input.<name>}` triggers lazy content loading on the relevant InputFile. This is a side effect of resolution, but the alternative — requiring all content to be pre-loaded — would force eager loading of potentially large files that might never be referenced.

The bare forms (`{file}`, `{input}`, `{input.<name>}`) all resolve to content. This is the common case for LLM prompts: you want the file's text in your prompt. The `.path` variants exist for scripts that need to operate on the file by path.

resolve_env_vars is a standalone utility separate from resolve_placeholders. It uses `${VAR}` syntax (shell-style) while resolve_placeholders uses `{placeholder}` syntax (single-brace). The two are never composed — env vars are resolved during config parsing, placeholders during execution. This separation prevents ambiguity and double-resolution issues.

Missing verdict status defaults to "skip" rather than producing an error. This is deliberate: it allows a validator to reference the status of another validator that may not have run yet (e.g., due to filtering or conditional skipping). The "skip" default matches the semantic meaning — the referenced validator was effectively skipped.

---

## resolve_placeholders

Resolves all `{...}` placeholders in a template string against the current invocation's input files and prior validator results. Returns the resolved string. Warnings are accumulated in the `ResolutionWarnings` struct rather than printed, allowing callers to decide how to surface them.

### Sections

1. Brace parsing and dispatch
2. Per-file placeholders (`{file}`, `{file.*}`)
3. Batch placeholders (`{input}`, `{input.paths}`)
4. Named input placeholders (`{input.<name>}`, `{input.<name>.*}`)
5. Verdict placeholders
6. Unrecognized and malformed placeholders

### resolve_placeholders: brace parsing and dispatch

The parser scans the template character by character. When it encounters `{`, it looks for a matching `}` using `find_closing_brace`, which tracks brace depth. This means nested braces are handled correctly — `{outer{inner}}` finds the closing brace at the outermost level.

SPEC-PH-RP-001: unclosed-brace-left-literal
  When a `{` has no matching `}`, it is emitted as a literal `{` character. No warning is produced. Parsing continues from the next character. This is a robustness measure — templates may contain JSON or other brace-heavy content that should pass through unmodified.
  test: placeholder::tests::unclosed_brace_left_literal

SPEC-PH-RP-002: nested-braces-depth-tracked
  `find_closing_brace` increments depth on `{` and decrements on `}`, returning the position where depth reaches zero. This means `{a{b}c}` matches the outermost braces, extracting `a{b}c` as the placeholder content. The inner braces are part of the placeholder name and will likely result in an unrecognized placeholder warning.
  test: placeholder::tests::nested_braces_extracted_as_single_placeholder

SPEC-PH-RP-003: no-placeholders-passthrough
  A template with no `{` characters is returned unchanged with no warnings.
  test: placeholder::tests::no_placeholders_unchanged

SPEC-PH-RP-004: multiple-placeholders-in-one-template
  Multiple placeholders in a single template are each resolved independently. Text between placeholders is preserved literally.
  test: placeholder::tests::resolve_verdict_status (two placeholders in one template)

### resolve_placeholders: verdict placeholders

Verdict placeholders reference prior validator results by name. The name is extracted by stripping the `verdict.` prefix and then checking for `.status` or `.feedback` suffixes.

SPEC-PH-RP-030: verdict-status-resolves-to-status-string
  `{verdict.<name>.status}` resolves to the string representation of the named validator's status (e.g., "pass", "fail", "error", "warn", "skip"). The status is obtained from the `prior_results` BTreeMap.
  test: placeholder::tests::resolve_verdict_status

SPEC-PH-RP-031: missing-verdict-status-defaults-to-skip
  When `{verdict.<name>.status}` references a validator name not present in `prior_results`, the placeholder resolves to the string `"skip"`. No warning is produced. This allows templates to reference validators that haven't executed yet (due to filtering, run_if, or pipeline ordering) without causing errors.
  test: placeholder::tests::resolve_verdict_for_nonexistent_validator

SPEC-PH-RP-032: verdict-feedback-resolves-to-feedback-string
  `{verdict.<name>.feedback}` resolves to the feedback string of the named validator's result. If the result exists but has no feedback (feedback is None), resolves to an empty string.
  test: placeholder::tests::resolve_verdict_feedback

SPEC-PH-RP-033: missing-verdict-feedback-returns-empty
  When `{verdict.<name>.feedback}` references a validator not in `prior_results`, the placeholder resolves to an empty string. No warning is produced. This mirrors the "skip" default for status — a validator that didn't run has no feedback.
  test: placeholder::tests::nonexistent_validator_feedback_is_empty

SPEC-PH-RP-034: unrecognized-verdict-subpath-warns
  When a `verdict.` placeholder has a suffix other than `.status` or `.feedback` (e.g., `{verdict.lint.duration}`), the placeholder resolves to an empty string and a warning is produced. The warning identifies the unrecognized sub-path.
  test: placeholder::tests::unrecognized_verdict_sub_path_warns

### resolve_placeholders: unrecognized and malformed placeholders

SPEC-PH-RP-040: unrecognized-placeholder-left-as-literal
  When a placeholder does not match any known pattern (`file`, `file.*`, `input`, `input.*`, `verdict.*`), it is emitted as a literal including its braces (e.g., `{typo}` remains `{typo}` in the output) and a warning is added. `{artifact}` is now unrecognized and left as literal. This preserves the original text for debugging while signaling the issue through warnings.
  test: placeholder::tests::resolve_unrecognized_placeholder

SPEC-PH-RP-041: warnings-accumulate-across-placeholders
  Multiple resolution warnings from different placeholders in the same template are all collected in the `ResolutionWarnings` struct. The caller receives the full list.
  test: placeholder::tests::multiple_warnings_in_one_call

---

## Per-file placeholders

Placeholders available when a validator operates in per-file mode (one invocation per matching file).

SPEC-PH-FP-001: file-resolves-to-content
  `{file}` resolves to the file's text content (UTF-8). This is the default for consistency with `{input}` and `{input.<name>}`, which also resolve to content. Use `{file.path}` when the path is needed (e.g., passing to a script command).
  test: placeholder::tests::resolve_file_path_placeholder

SPEC-PH-FP-002: file-path-resolves-to-absolute-path
  `{file.path}` resolves to the absolute path of the current file.
  test: placeholder::tests::resolve_file_properties

SPEC-PH-FP-003: file-dir-resolves-to-parent
  `{file.dir}` resolves to the parent directory.
  test: placeholder::tests::resolve_file_properties

SPEC-PH-FP-004: file-name-resolves-to-filename
  `{file.name}` resolves to the filename with extension.
  test: IMPLICIT via placeholder::tests::resolve_file_properties

SPEC-PH-FP-005: file-stem-resolves-to-stem
  `{file.stem}` resolves to the filename without extension.
  test: IMPLICIT via placeholder::tests::resolve_file_properties

SPEC-PH-FP-006: file-ext-resolves-to-extension
  `{file.ext}` resolves to the extension without dot.
  test: IMPLICIT via placeholder::tests::resolve_file_properties

SPEC-PH-FP-007: file-content-is-explicit-alias
  `{file.content}` is an explicit alias for `{file}`. Both resolve to the file's text content.
  test: placeholder::tests::resolve_file_content_placeholder

SPEC-PH-FP-008: file-placeholder-requires-per-file-mode
  Using `{file}` or `{file.*}` in a batch or named-input validator is a config validation error.
  test: TODO

---

## Batch placeholders

Placeholders available when a validator operates in batch mode (`collect = true`).

SPEC-PH-BP-001: input-resolves-to-concatenated-content
  `{input}` in batch mode resolves to the concatenated contents of all matched files.
  test: TODO

SPEC-PH-BP-002: input-paths-resolves-to-space-separated
  `{input.paths}` resolves to space-separated absolute paths.
  test: TODO

---

## Named input placeholders

Placeholders available when a validator has named input slots (`input.code`, `input.spec`, etc.).

SPEC-PH-NP-001: named-input-resolves-to-content
  `{input.<name>}` resolves to the file's text content. This is consistent with `{file}` and `{input}` — bare references always mean content. Use `{input.<name>.path}` for the path.
  test: TODO

SPEC-PH-NP-002: named-input-path
  `{input.<name>.path}` resolves to the absolute path.
  test: TODO

SPEC-PH-NP-003: named-input-name
  `{input.<name>.name}` resolves to the filename.
  test: TODO

SPEC-PH-NP-004: named-input-stem
  `{input.<name>.stem}` resolves to the stem.
  test: TODO

SPEC-PH-NP-005: named-input-content
  `{input.<name>.content}` resolves to the file content.
  test: placeholder::tests::resolve_named_input_content

SPEC-PH-NP-006: named-input-paths-plural
  `{input.<name>.paths}` resolves to space-separated paths when the slot has multiple files.
  test: TODO

SPEC-PH-NP-007: missing-named-input-warns
  Referencing an `{input.<name>}` that doesn't exist in the validator's declarations produces a warning and resolves to empty string.
  test: TODO

---

## Placeholder validation

Static checks on placeholder usage relative to the validator's input mode.

SPEC-PH-VL-001: template-placeholders-validated-against-inputs
  `baton validate-config` checks that every placeholder in a template matches the validator's declared inputs.
  test: TODO

SPEC-PH-VL-002: file-placeholder-in-named-mode-errors
  Using `{file}` when the validator has named inputs (no unnamed input) is a config validation error.
  test: TODO

SPEC-PH-VL-003: batch-placeholder-in-per-file-mode-errors
  Using `{input}` (batch) when the validator has per-file input is a config validation error.
  test: TODO

---

## resolve_env_vars

Resolves `${VAR}` environment variable references in config strings. Returns `Ok(resolved_string)` on success or `Err(message)` when a required variable is unset.

This function is used during config parsing to interpolate environment variables in field values. It is intentionally strict about missing variables (returns error) because config parsing happens before execution — a missing variable should be caught early, not silently produce an empty string mid-run.

### Sections

1. Variable lookup and default values
2. Escape sequences
3. Edge cases and literal passthrough

### resolve_env_vars: variable lookup and default values

SPEC-PH-EV-001: set-variable-substituted
  `${VAR}` resolves to the value of the environment variable `VAR`. The entire `${VAR}` token is replaced by the value.
  test: placeholder::tests::env_var_set

SPEC-PH-EV-002: unset-variable-without-default-errors
  When `${VAR}` references an unset variable and no default is provided, `resolve_env_vars` returns `Err` with a message containing "not set". This is an intentional hard error — config values with missing env vars should fail loudly at parse time.
  test: placeholder::tests::env_var_unset_no_default

SPEC-PH-EV-003: unset-variable-with-default-uses-default
  `${VAR:-default}` uses `default` when `VAR` is not set in the environment. The `:-` is the delimiter between variable name and default value.
  test: placeholder::tests::env_var_with_default

SPEC-PH-EV-004: empty-default-is-valid
  `${VAR:-}` uses an empty string as the default when `VAR` is unset. The empty default is not treated as "no default" — it is a valid fallback value.
  test: placeholder::tests::env_var_with_empty_default

SPEC-PH-EV-005: set-variable-overrides-default
  When `VAR` is set in the environment, `${VAR:-default}` resolves to the environment value, not the default. The default is only used when the variable is unset.
  test: placeholder::tests::env_var_set_overrides_default

SPEC-PH-EV-006: empty-value-is-not-unset
  When an environment variable is set to an empty string, it is considered "set" and its value (empty string) is used. It does NOT fall through to the default. This follows POSIX semantics where `:-` checks for unset only, not empty. This is a deliberate design choice — `${VAR:-default}` means "use default if VAR is not defined in the environment", not "use default if VAR is empty".
  test: placeholder::tests::env_var_empty_value
  test: placeholder::tests::env_var_empty_value_does_not_use_default

SPEC-PH-EV-007: first-colon-dash-splits-name-from-default
  Only the first occurrence of `:-` in the expression splits the variable name from the default value. Subsequent `:-` sequences are part of the default. For example, `${VAR:-key:-value}` has variable name `VAR` and default `key:-value`.
  test: placeholder::tests::env_var_default_containing_colon

SPEC-PH-EV-008: special-chars-preserved-in-value
  Environment variable values containing special characters (equals, ampersand, semicolon, quotes, backslashes, newlines) are emitted verbatim. No escaping or sanitization is applied.
  test: placeholder::tests::env_var_special_chars_in_value

SPEC-PH-EV-009: special-chars-preserved-in-default
  Default values containing special characters (colons, slashes, etc.) are emitted verbatim.
  test: placeholder::tests::env_var_default_with_special_chars

SPEC-PH-EV-010: multiple-variables-in-one-string
  Multiple `${VAR}` references in a single string are each resolved independently. Literal text between them is preserved.
  test: placeholder::tests::env_var_multiple_in_one_string

### resolve_env_vars: escape sequences

SPEC-PH-EV-020: double-dollar-brace-escapes-to-literal
  `$${` resolves to the literal string `${`. The extra `$` acts as an escape character. This allows config strings to contain literal `${` without triggering interpolation.
  test: placeholder::tests::env_var_escaped

SPEC-PH-EV-021: double-dollar-without-brace-is-literal
  `$$` not followed by `{` is left as a literal `$$`. Only the specific three-character sequence `$${` triggers the escape behavior.
  test: placeholder::tests::env_var_adjacent_dollar_signs

### resolve_env_vars: edge cases and literal passthrough

SPEC-PH-EV-030: no-variables-passthrough
  A string with no `$` characters is returned unchanged.
  test: placeholder::tests::env_var_no_interpolation

SPEC-PH-EV-031: unclosed-dollar-brace-left-literal
  When `${` has no matching `}`, the `${` is emitted as a literal and parsing continues from the character after `{`. This is not an error — it allows strings that happen to contain `${` without a closing brace to pass through.
  test: placeholder::tests::env_var_unclosed_brace_literal

SPEC-PH-EV-032: dollar-at-end-of-string-is-literal
  A `$` at the end of the input string (with no following character) is emitted as a literal `$`.
  test: placeholder::tests::env_var_dollar_at_end

SPEC-PH-EV-033: resolved-values-not-re-scanned
  After a variable's value is substituted into the result, the substituted text is NOT re-scanned for further `${...}` references. A value containing `${INNER}` will appear literally in the output.
  test: placeholder::tests::env_var_nested_dollar_brace_in_value

---

## ResolutionWarnings

SPEC-PH-RW-001: default-has-empty-warnings
  `ResolutionWarnings::new()` (and `Default`) produces an instance with an empty warnings vector.
  test: IMPLICIT via all resolve_placeholders tests that assert `warns.warnings.is_empty()`
