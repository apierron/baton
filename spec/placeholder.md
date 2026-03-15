# module: placeholder

Template placeholder resolution. Substitutes `{artifact}`, `{context.<name>}`, `{verdict.<name>.status}`, and similar placeholders in command strings and prompt templates. Also provides environment variable interpolation for config strings.

This module is intentionally lenient: most resolution failures produce warnings and empty strings rather than errors. The rationale is that placeholder resolution happens inside validators that are already running — aborting the entire gate because of a missing context reference would be more disruptive than emitting an empty string and letting the validator produce a meaningful failure on its own terms.

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

resolve_placeholders takes `&mut Artifact` and `&mut Context` rather than shared references because resolving `{artifact_content}` and `{context.<name>.content}` triggers lazy content loading. This is a side effect of resolution, but the alternative — requiring all content to be pre-loaded — would force eager loading of potentially large files that might never be referenced.

resolve_env_vars is a standalone utility separate from resolve_placeholders. It uses `${VAR}` syntax (shell-style) while resolve_placeholders uses `{placeholder}` syntax (single-brace). The two are never composed — env vars are resolved during config parsing, placeholders during execution. This separation prevents ambiguity and double-resolution issues.

Missing verdict status defaults to "skip" rather than producing an error. This is deliberate: it allows a validator to reference the status of another validator that may not have run yet (e.g., due to filtering or conditional skipping). The "skip" default matches the semantic meaning — the referenced validator was effectively skipped.

---

## resolve_placeholders

Resolves all `{...}` placeholders in a template string against the current artifact, context, and prior validator results. Returns the resolved string. Warnings are accumulated in the `ResolutionWarnings` struct rather than printed, allowing callers to decide how to surface them.

### Sections

1. Brace parsing and dispatch
2. Artifact placeholders
3. Context placeholders
4. Verdict placeholders
5. Unrecognized and malformed placeholders

### resolve_placeholders: brace parsing and dispatch

The parser scans the template character by character. When it encounters `{`, it looks for a matching `}` using `find_closing_brace`, which tracks brace depth. This means nested braces are handled correctly — `{outer{inner}}` finds the closing brace at the outermost level.

SPEC-PH-RP-001: unclosed-brace-left-literal
  When a `{` has no matching `}`, it is emitted as a literal `{` character. No warning is produced. Parsing continues from the next character. This is a robustness measure — templates may contain JSON or other brace-heavy content that should pass through unmodified.
  test: placeholder::tests::unclosed_brace_left_literal

SPEC-PH-RP-002: nested-braces-depth-tracked
  `find_closing_brace` increments depth on `{` and decrements on `}`, returning the position where depth reaches zero. This means `{a{b}c}` matches the outermost braces, extracting `a{b}c` as the placeholder content. The inner braces are part of the placeholder name and will likely result in an unrecognized placeholder warning.
  test: UNTESTED

SPEC-PH-RP-003: no-placeholders-passthrough
  A template with no `{` characters is returned unchanged with no warnings.
  test: placeholder::tests::no_placeholders_unchanged

SPEC-PH-RP-004: multiple-placeholders-in-one-template
  Multiple placeholders in a single template are each resolved independently. Text between placeholders is preserved literally.
  test: placeholder::tests::resolve_verdict_status (two placeholders in one template)

### resolve_placeholders: artifact placeholders

SPEC-PH-RP-010: artifact-resolves-to-absolute-path
  `{artifact}` resolves to the absolute path of the artifact file via `artifact.absolute_path()`. When the artifact is from_string (no file path), `absolute_path()` returns None, and the placeholder resolves to an empty string.
  test: UNTESTED (file-backed case)
  test: UNTESTED (from_string yields empty — implicit in content tests but never asserted for `{artifact}`)

SPEC-PH-RP-011: artifact-dir-resolves-to-parent
  `{artifact_dir}` resolves to the parent directory of the artifact file via `artifact.parent_dir()`. When the artifact is from_string, resolves to an empty string.
  test: UNTESTED

SPEC-PH-RP-012: artifact-content-resolves-to-inline-content
  `{artifact_content}` resolves to the artifact's content as a lossy UTF-8 string via `artifact.get_content_as_string()`. This triggers lazy content loading for file-backed artifacts. If content loading fails, resolves to an empty string (via `unwrap_or_default`). For from_string artifacts, returns the string content directly.
  test: placeholder::tests::resolve_artifact_content

### resolve_placeholders: context placeholders

Context placeholder names are extracted by stripping the `context.` prefix. The `.content` suffix, if present, is checked first to avoid ambiguity with context item names that contain dots.

SPEC-PH-RP-020: context-path-resolves-to-absolute-path
  `{context.<name>}` resolves to the absolute path of the named context item via `item.absolute_path()`. For string-content context items (no file path), `absolute_path()` returns None, resolving to an empty string.
  test: UNTESTED (file-backed context path)
  test: UNTESTED (string context path yields empty)

SPEC-PH-RP-021: context-content-resolves-to-inline-content
  `{context.<name>.content}` resolves to the content of the named context item via `item.get_content()`. This triggers lazy loading for file-backed items. Returns the content as a string.
  test: placeholder::tests::resolve_context_content

SPEC-PH-RP-022: missing-context-warns-and-returns-empty
  When `{context.<name>}` or `{context.<name>.content}` references a name not present in the context map, the placeholder resolves to an empty string and a warning is added. The warning message includes the full placeholder expression and the missing context name.
  test: placeholder::tests::resolve_missing_context_warns

SPEC-PH-RP-023: context-content-suffix-checked-before-path
  The `.content` suffix is stripped before checking for the context item. This means a context item named `foo.content` cannot be referenced by path — `{context.foo.content}` will always be interpreted as a content request for context item `foo`. This is a known limitation of the simple suffix-stripping approach.
  test: UNTESTED

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
  test: UNTESTED (nonexistent validator feedback specifically)

SPEC-PH-RP-034: unrecognized-verdict-subpath-warns
  When a `verdict.` placeholder has a suffix other than `.status` or `.feedback` (e.g., `{verdict.lint.duration}`), the placeholder resolves to an empty string and a warning is produced. The warning identifies the unrecognized sub-path.
  test: UNTESTED

### resolve_placeholders: unrecognized and malformed placeholders

SPEC-PH-RP-040: unrecognized-placeholder-left-as-literal
  When a placeholder does not match any known pattern (`artifact`, `artifact_dir`, `artifact_content`, `context.*`, `verdict.*`), it is emitted as a literal including its braces (e.g., `{typo}` remains `{typo}` in the output) and a warning is added. This preserves the original text for debugging while signaling the issue through warnings.
  test: placeholder::tests::resolve_unrecognized_placeholder

SPEC-PH-RP-041: warnings-accumulate-across-placeholders
  Multiple resolution warnings from different placeholders in the same template are all collected in the `ResolutionWarnings` struct. The caller receives the full list.
  test: UNTESTED (multiple warnings in one call)

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
