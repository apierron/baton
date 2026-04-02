# module: config

Configuration parsing and validation for baton.toml. Two-stage design: `parse_config` deserializes TOML into validated structures, `validate_config` checks semantic correctness (e.g., forward references, undefined runtimes, undefined context slots).

The two-stage split exists because parse-time checks are structural (does this TOML describe a valid config?) while validation checks are semantic (do cross-references resolve? are environment variables set?). This matters because a config can be structurally valid but semantically broken -- a run_if that references a validator defined later in the pipeline parses fine but is semantically invalid.

## Public functions

| Function           | Purpose                                              |
|--------------------|------------------------------------------------------|
| `parse_config`     | Parse TOML string into BatonConfig, validates structure |
| `validate_config`  | Check semantic correctness (forward refs, undefined runtimes, etc) |
| `split_run_if`     | Tokenize run_if expressions into atoms and operators |
| `discover_config`  | Find baton.toml by walking up from start_dir         |

## Internal functions

| Function               | Called by          |
|------------------------|--------------------|
| `validate_run_if_expr` | `validate_config`  |

## Design notes

parse_config stops on the first error (later checks depend on earlier ones succeeding), while validate_config accumulates all errors and warnings so the user can fix them in a single pass.

validate_config distinguishes errors (will cause runtime failure) from warnings (suspicious but functional). Warnings are printed but do not prevent execution.

Runtime base_url (for type="api" runtimes) has env var resolution applied during parsing, not at execution time. This means the env var must be set when the config is loaded, not when the validator runs. This is consistent with the "fail early" philosophy.

---

## parse_config

Parses a TOML string into a validated `BatonConfig`. The `config_dir` parameter is the base directory for resolving relative paths (prompts_dir, log_dir, etc).

### Sections

1. TOML deserialization and version check
2. Defaults resolution
3. Provider parsing
4. Runtime parsing
5. Gate and validator parsing

### parse_config: TOML deserialization and version check

SPEC-CF-PC-001: toml-syntax-error-propagated
  When the input string is not valid TOML, parse_config returns an error propagated from the TOML parser via the `?` operator. The error message is from the toml crate, not wrapped in baton-specific text.
  test: config::tests::malformed_toml_returns_error

SPEC-CF-PC-002: version-must-be-0-4-or-0-5-or-0-6-or-0-7
  The `version` field must be exactly "0.4", "0.5", "0.6", or "0.7". Any other value, including "0.3", "1.0", or an empty string, returns ConfigError containing the rejected version string.
  test: config::tests::parse_wrong_version

SPEC-CF-PC-003: gates-must-not-be-empty
  The `gates` map must contain at least one entry. An empty `[gates]` table returns ConfigError with "No gates defined".
  test: config::tests::parse_no_gates

### parse_config: defaults resolution

Default values are applied from the `[defaults]` section. If `[defaults]` is omitted entirely, serde defaults supply the built-in values. Relative paths in defaults are resolved against `config_dir` via `Path::join`.

SPEC-CF-PC-010: default-timeout-is-300
  When `[defaults]` is omitted or `timeout_seconds` is not specified, timeout_seconds defaults to 300.
  test: IMPLICIT via config::tests::parse_minimal_config (validator gets 300)

SPEC-CF-PC-011: default-blocking-is-true
  When `[defaults]` is omitted or `blocking` is not specified, blocking defaults to true.
  test: IMPLICIT via config::tests::parse_minimal_config (validator gets true)

SPEC-CF-PC-012: default-prompts-dir
  When `prompts_dir` is not specified in `[defaults]`, it defaults to "./prompts" resolved against config_dir.
  test: config::tests::prompts_dir_default

SPEC-CF-PC-013: default-log-dir
  When `log_dir` is not specified in `[defaults]`, it defaults to "./.baton/logs" resolved against config_dir.
  test: config::tests::log_dir_default

SPEC-CF-PC-014: default-history-db
  When `history_db` is not specified in `[defaults]`, it defaults to "./.baton/history.db" resolved against config_dir.
  test: config::tests::history_db_default

SPEC-CF-PC-015: default-tmp-dir
  When `tmp_dir` is not specified in `[defaults]`, it defaults to "./.baton/tmp" resolved against config_dir.
  test: config::tests::tmp_dir_default

SPEC-CF-PC-016: relative-paths-resolved-against-config-dir
  All relative path defaults (prompts_dir, log_dir, history_db, tmp_dir) are joined with `config_dir` to produce absolute paths. This ensures paths are correct regardless of the working directory at invocation time.
  test: config::tests::path_resolution_with_config_dir

SPEC-CF-PC-017: explicit-defaults-override-builtins
  When `[defaults]` explicitly sets timeout_seconds or blocking, those values replace the built-in defaults and are inherited by validators that do not override them.
  test: config::tests::defaults_applied

### parse_config: provider parsing

Providers are removed in v0.7. Provider configuration is replaced by the unified runtime interface — runtimes with type="api" now carry api_key_env, base_url, and default_model directly. The SPEC-CF-PC-020..022 assertions from earlier versions no longer apply.

### parse_config: runtime parsing

Runtimes are iterated from the `[runtimes]` map and stored with their defaults applied by serde.

SPEC-CF-PC-025: runtime-defaults
  Runtime entries default to sandbox=true, timeout_seconds=600, max_iterations=30 when those fields are omitted.
  test: config::tests::runtime_defaults

SPEC-CF-PC-026: runtime-fields-preserved
  Runtime type, base_url, api_key_env, and default_model are stored verbatim from TOML.
  test: config::tests::runtime_fields_stored_verbatim

SPEC-CF-PC-027: api-type-runtime-base-url-env-vars-resolved
  For runtimes with type="api", environment variable references in `base_url` (e.g., `${VAR}`) are resolved via `resolve_env_vars` at parse time. If resolution fails, parse_config returns ConfigError naming the runtime.
  test: UNTESTED

SPEC-CF-PC-028: api-type-runtime-trailing-slash-stripped
  For runtimes with type="api", after env var resolution, if `base_url` ends with '/', the trailing slash is removed. Only a single trailing slash is stripped (the code calls `pop()` once). This normalizes URLs so downstream code can append paths without double-slash issues.
  test: config::tests::api_runtime_trailing_slash_stripped
  test: config::tests::api_runtime_double_trailing_slash_only_one_stripped

### parse_config: gate and validator parsing

Each gate must have at least one validator. Validators are validated in declaration order within each gate. A HashSet tracks seen names for duplicate detection.

The validator name regex check and duplicate check happen before type-specific field validation. This means a validator with an invalid name is rejected before the parser checks whether it has a command or prompt field.

SPEC-CF-PC-030: gate-validators-array-must-have-refs
  A gate's `validators` array must contain at least one `ref` entry. An empty array returns ConfigError with the gate name.
  test: config::tests::gate_empty_validators_rejected

SPEC-CF-PC-033: validator-type-must-be-known
  The `type` field must be "script", "llm", or "human". Any other value returns ConfigError containing "unknown type" and the rejected value.
  test: config::tests::unknown_validator_type

SPEC-CF-PC-034: script-requires-command
  A `[validators.X]` entry with type "script" must have a `command` field. If missing, returns ConfigError containing "command".
  test: config::tests::script_missing_command

SPEC-CF-PC-035: llm-requires-prompt
  A `[validators.X]` entry with type "llm" must have a `prompt` field. If missing, returns ConfigError containing "prompt".
  test: config::tests::llm_missing_prompt

SPEC-CF-PC-036: human-requires-prompt
  A `[validators.X]` entry with type "human" must have a `prompt` field. If missing, returns ConfigError containing "prompt".
  test: config::tests::human_missing_prompt

SPEC-CF-PC-037: mode-defaults-to-query
  When `mode` is omitted or set to "query", the validator gets LlmMode::Query. "completion" is accepted as a back-compat alias and also resolves to LlmMode::Query. When set to "session", it gets LlmMode::Session. Any other value returns ConfigError containing "Expected 'query', 'completion', or 'session'." and the rejected value.
  test: config::tests::invalid_mode_string

SPEC-CF-PC-038: response-format-defaults-to-verdict
  When `response_format` is omitted or set to "verdict", the validator gets ResponseFormat::Verdict. When set to "freeform", it gets ResponseFormat::Freeform. Any other value returns ConfigError containing "invalid response_format".
  test: config::tests::invalid_response_format

SPEC-CF-PC-039: warn-exit-codes-rejects-zero
  The `warn_exit_codes` array must not contain 0. Exit code 0 is unconditionally "pass" and cannot be reclassified as a warning. If 0 is present, returns ConfigError with "warn_exit_codes must not contain 0".
  test: config::tests::warn_exit_codes_contains_zero

SPEC-CF-PC-040: blocking-defaults-from-defaults-section
  When `blocking` is not set on a gate validator reference, it inherits from `[defaults].blocking`. When explicitly set on the gate ref, the ref's value takes precedence.
  test: config::tests::defaults_applied
  test: config::tests::validator_overrides_defaults

SPEC-CF-PC-041: timeout-inheritable-at-gate-ref
  When `timeout_seconds` is not set on a validator, it inherits from `defaults.timeout_seconds`. Timeout can also be overridden at the gate ref level.
  test: config::tests::defaults_applied
  test: config::tests::validator_overrides_defaults

SPEC-CF-PC-052: runtime-field-accepts-string-or-list
  The `runtime` field on LLM validators accepts either a single string or a list of strings. For example, `runtime = "my-runtime"` becomes a single-element list and `runtime = ["rt-a", "rt-b"]` becomes a two-element fallback chain.
  test: UNTESTED

SPEC-CF-PC-053: runtime-field-required-for-llm
  LLM validators must have a `runtime` field. If absent, parse_config returns ConfigError containing "runtime" and the validator name.
  test: UNTESTED

SPEC-CF-PC-043: temperature-defaults-to-zero
  When `temperature` is not set on a validator, it defaults to 0.0. This is a deliberate choice for reproducibility in code review tasks.
  test: IMPLICIT via config::tests::parse_full_config (asserts temperature == 0.0)

SPEC-CF-PC-045: config-dir-stored
  The config_dir path is stored on BatonConfig for later use in path resolution (e.g., resolving working_dir references at execution time).
  test: config::tests::config_dir_stored

SPEC-CF-PC-047: parse-errors-are-early-return
  parse_config returns on the first structural error encountered. If a config has multiple problems (e.g., wrong version AND empty gates), only the first error is reported. Check order is: TOML syntax, version, empty gates, then three independent section parses (sources → validators → gates).
  test: UNTESTED (no test verifies error ordering when multiple errors exist)

SPEC-CF-PC-048: empty-validator-name-rejected
  An empty string for a validator name fails the `[A-Za-z0-9_-]+` check because the check requires `!raw_v.name.is_empty()`. The error message still says "invalid characters" even though the real problem is emptiness.
  test: config::tests::empty_validator_name

### parse_config: env var resolution in validators

Environment variable references (`${VAR}`) in validator string fields are resolved at parse time via `resolve_env_vars`. This applies to both top-level validators and inline validators. If resolution fails, parse_config returns ConfigError naming the validator and the failing field.

SPEC-CF-PC-060: validator-command-env-vars-resolved
  The `command` field in script validators is resolved through `resolve_env_vars` at parse time. If the referenced env var is unset (and has no default), parse_config returns ConfigError containing the validator name.
  test: config::tests::env_var_resolved_in_command
  test: config::tests::env_var_unset_in_command_errors

SPEC-CF-PC-061: validator-working-dir-env-vars-resolved
  The `working_dir` field is resolved through `resolve_env_vars` at parse time.
  test: config::tests::env_var_resolved_in_working_dir

SPEC-CF-PC-062: validator-env-values-env-vars-resolved
  Each value in the `env` map is resolved through `resolve_env_vars` at parse time. Keys are not resolved.
  test: config::tests::env_var_resolved_in_env_values

SPEC-CF-PC-063: validator-prompt-env-vars-resolved
  The `prompt` field in LLM/human validators is resolved through `resolve_env_vars` at parse time.
  test: config::tests::env_var_resolved_in_prompt

SPEC-CF-PC-064: validator-system-prompt-env-vars-resolved
  The `system_prompt` field is resolved through `resolve_env_vars` at parse time.
  test: config::tests::env_var_resolved_in_system_prompt

### parse_config: env var resolution in defaults

SPEC-CF-PC-065: defaults-paths-env-vars-resolved
  The `prompts_dir`, `log_dir`, `history_db`, and `tmp_dir` fields in `[defaults]` are resolved through `resolve_env_vars` before being joined with `config_dir`.
  test: config::tests::env_var_resolved_in_defaults_paths

### parse_config: env var resolution in sources

SPEC-CF-PC-066: source-root-env-vars-resolved
  The `root` field in directory sources is resolved through `resolve_env_vars` at parse time.
  test: config::tests::env_var_resolved_in_source_root

SPEC-CF-PC-067: source-path-env-vars-resolved
  The `path` field in file sources is resolved through `resolve_env_vars` at parse time.
  test: config::tests::env_var_resolved_in_source_path

SPEC-CF-PC-068: source-files-env-vars-resolved
  Each entry in the `files` list is resolved through `resolve_env_vars` at parse time.
  test: config::tests::env_var_resolved_in_source_files

---

## validate_config

Checks semantic correctness of a parsed BatonConfig. Returns a `ConfigValidation` containing accumulated errors and warnings. Unlike parse_config, this function does not short-circuit -- all errors and warnings are collected.

### Sections

1. Per-gate, per-validator checks (run_if, context_refs, runtime references, mode/runtime, freeform+blocking)
2. Runtime API key environment variable checks

### validate_config: per-validator checks

Iterates every gate (in BTreeMap order) and every validator within each gate (in pipeline order). For each validator, checks run_if references, context_refs, and (for LLM validators only) runtime/mode/response_format semantics.

SPEC-CF-VC-001: run-if-references-must-exist
  Each atom in a run_if expression must reference a validator name that exists in the same gate. Referencing a nonexistent validator produces an error containing "unknown validator" and the referenced name.
  test: config::tests::validate_run_if_references_nonexistent

SPEC-CF-VC-002: run-if-rejects-forward-references
  A run_if expression may only reference validators that appear earlier in the pipeline (lower index). Referencing a validator at the same index or later produces an error containing "later in the pipeline". This enforces the invariant that run_if can only depend on already-computed results.
  test: config::tests::validate_run_if_forward_reference

SPEC-CF-VC-003: run-if-syntax-validated
  Each atom in a run_if expression must match the pattern `<name>.status == <value>` where value is one of: pass, fail, warn, error, skip. Malformed expressions (missing ".status == " or invalid status value) produce an error containing "invalid run_if expression". Validation of the entire expression aborts on the first invalid atom (early return within validate_run_if_expr).
  test: UNTESTED (no test for invalid run_if syntax like missing ".status == " or invalid status value)

SPEC-CF-VC-004: run-if-self-reference-is-forward-reference
  A validator referencing itself in run_if (e.g., validator "a" with run_if "a.status == pass") is treated as a forward reference because the validator's own index equals current_idx. The check is `ref_idx >= current_idx`, so self-references produce the "later in the pipeline" error.
  test: config::tests::self_referencing_run_if

SPEC-CF-VC-007: llm-runtimes-must-be-non-empty
  LLM validators must have a non-empty runtimes list. If the list is empty after parsing, an error is produced containing "runtime".
  test: UNTESTED

SPEC-CF-VC-009: runtime-references-must-be-defined
  Each runtime name in the validator's runtimes list must exist in the config's runtimes map. An undefined runtime produces an error containing "not defined in [runtimes]" and the undefined name. Each undefined name produces a separate error.
  test: config::tests::undefined_runtime_reference

SPEC-CF-VC-010: freeform-with-blocking-warns
  An LLM validator with response_format "freeform" and blocking=true produces a warning containing "blocking has no effect". Freeform validators always return warn status, so blocking (which triggers gate failure on fail/error) is meaningless.
  test: config::tests::validate_freeform_blocking_warning

SPEC-CF-VC-011: validation-checks-only-llm-validators
  Runtime, mode, and freeform/blocking checks are gated behind `val.validator_type == ValidatorType::Llm`. Script and human validators skip these checks entirely, even if they have stray LLM fields set (which would be ignored at execution time).
  test: config::tests::script_validator_with_provider_not_flagged

SPEC-CF-VC-025: session-mode-all-api-runtimes-errors
  If mode=session and ALL listed runtimes have type="api", an error is produced containing "no session-capable runtimes". Session mode requires at least one runtime that supports interactive sessions (e.g., type="cli"), not just API runtimes.
  test: config::tests::validate_session_mode_all_api_runtimes_errors

SPEC-CF-VC-026: session-mode-any-api-runtime-warns
  If mode=session and ANY listed runtime has type="api", a warning is produced for each such runtime containing "api runtime 'X' will be skipped for session mode". This is not an error because the validator can still run on the non-API runtimes.
  test: config::tests::validate_session_mode_api_runtime_warns

SPEC-CF-VC-027: api-runtime-api-key-env-check
  For runtimes with type="api", if `api_key_env` is set and non-empty, check that the named environment variable exists (std::env::var returns Ok). If the env var is not set, an error is produced containing the runtime name and the env var name. If `api_key_env` is empty or unset, this check is skipped.
  test: UNTESTED

SPEC-CF-VC-021: validation-accumulates-all-errors
  validate_config does not short-circuit. If multiple validators have problems, all errors and warnings are collected in the returned ConfigValidation. The caller can inspect `has_errors()` and iterate both `errors` and `warnings`.
  test: config::tests::multiple_simultaneous_validation_errors

SPEC-CF-VC-022: has-errors-reflects-error-presence
  `ConfigValidation::has_errors()` returns true if and only if the errors vec is non-empty. Warnings alone do not cause has_errors() to return true.
  test: IMPLICIT via config::tests::validate_completion_with_runtime_warning (has warnings but no errors)

---

## split_run_if

Tokenizes a run_if expression string into a flat list of atoms and operators ("and", "or").

This is a simple string-splitting tokenizer, not a recursive-descent parser. There is no operator precedence and no parentheses -- expressions are evaluated left-to-right by the execution engine. The deliberate absence of precedence keeps the spec simple and avoids surprising evaluation orders.

SPEC-CF-SR-001: single-atom
  An expression with no operators returns a single-element list containing the atom.
  test: config::tests::split_run_if_simple

SPEC-CF-SR-002: and-operator
  The delimiter " and " (space-and-space) splits the expression into atoms separated by "and" tokens.
  test: config::tests::split_run_if_and

SPEC-CF-SR-003: or-operator
  The delimiter " or " (space-or-space) splits the expression into atoms separated by "or" tokens.
  test: config::tests::split_run_if_or

SPEC-CF-SR-004: mixed-operators-first-match-priority
  When both " and " and " or " appear in the remaining string, whichever delimiter has the smaller byte index is consumed first. This produces a flat left-to-right token stream with no precedence. For example, "a and b or c" becomes ["a", "and", "b", "or", "c"].
  test: config::tests::split_run_if_mixed

SPEC-CF-SR-005: whitespace-trimmed
  Leading and trailing whitespace on the full expression is trimmed before tokenization. Each atom is also trimmed.
  test: config::tests::whitespace_in_run_if

SPEC-CF-SR-006: embedded-and-or-not-split
  The delimiters require surrounding spaces (" and ", " or "). A validator name like "command" (containing "and") or "mentor" (containing "or") is not split, because the substrings lack surrounding spaces.
  test: config::tests::names_containing_and_or

---

## discover_config

Searches for baton.toml by walking up the directory tree from `start_dir`. Stops at repository boundaries (.git directories).

The traversal algorithm checks for baton.toml first, then checks for .git, then moves to the parent directory. This ordering means that if baton.toml and .git coexist in the same directory, the config is found before the boundary stops traversal.

### Sections

1. Upward traversal
2. Boundary detection
3. Error reporting

SPEC-CF-DC-001: finds-config-in-start-dir
  If baton.toml exists in start_dir, returns its path immediately.
  test: config::tests::discover_config_found

SPEC-CF-DC-002: finds-config-in-parent
  If baton.toml is not in start_dir but exists in an ancestor directory, traversal continues upward until it is found.
  test: config::tests::discover_config_in_parent

SPEC-CF-DC-003: deeply-nested-traversal
  Traversal works through arbitrarily deep directory nesting (tested with 5 levels).
  test: config::tests::discover_config_deeply_nested

SPEC-CF-DC-004: stops-at-git-boundary
  If a directory in the traversal path contains a `.git` entry (directory or file), traversal stops at that directory after checking for baton.toml. A baton.toml above the .git boundary is not found. This prevents accidentally using a config from a parent repository.
  test: config::tests::discover_config_stops_at_git_boundary

SPEC-CF-DC-005: config-inside-git-boundary-found
  When baton.toml exists in the same directory as .git or in a directory between start_dir and the .git boundary, it is found. The .git boundary only prevents crossing upward past the repository root.
  test: config::tests::discover_config_git_boundary_with_config_inside

SPEC-CF-DC-006: error-includes-start-dir
  When no baton.toml is found, the error message contains the start_dir path (via `display()`) so the user knows where the search began.
  test: config::tests::discover_config_error_message_includes_start_dir

SPEC-CF-DC-007: not-found-returns-error
  When no baton.toml is found after exhausting traversal (either reaching filesystem root or hitting a .git boundary), returns ConfigError with "No baton.toml found".
  test: config::tests::discover_config_not_found

SPEC-CF-DC-008: follows-symlinks
  Traversal follows symbolic links transparently (via std::path operations and `exists()` which follows symlinks). A baton.toml reachable through a symlinked directory is found.
  test: config::tests::discover_config_through_symlink (unix only)

SPEC-CF-DC-009: stops-at-filesystem-root
  If no .git boundary is encountered and the filesystem root is reached without finding baton.toml, traversal stops (`dir.pop()` returns false) and an error is returned.
  test: UNTESTED (impractical to test without polluting filesystem root)

---

## Source parsing

Sources are named file sets — the bottom layer of the `sources → validators → gates` composition model. Each entry under `[sources]` gives a name to a directory, a single file, or an explicit list of files. Validators reference these names in their input declarations. Sources are optional: validators can also use glob patterns directly against the input pool.

SPEC-CF-SC-001: directory-source-requires-root
  A `[sources.X]` entry with `root` must have a valid relative path. `include` defaults to `["**/*"]`, `exclude` defaults to `[]`.
  test: config::tests::source_directory_with_root

SPEC-CF-SC-002: file-source-requires-path
  A `[sources.X]` entry with `path` must point to a single file. `include`/`exclude` do not apply.
  test: config::tests::source_file_with_path

SPEC-CF-SC-003: file-list-source-requires-files
  A `[sources.X]` entry with `files` must contain a non-empty list of paths.
  test: config::tests::source_file_list
  test: config::tests::source_empty_files_list_rejected

SPEC-CF-SC-004: source-type-mutual-exclusion
  Only one of `root`, `path`, or `files` may be set. Setting more than one is a config error.
  test: config::tests::source_type_mutual_exclusion

SPEC-CF-SC-005: source-name-pattern
  Source names must match `[a-zA-Z0-9_-]+`. No dots (prevents ambiguity with dot-notation placeholders).
  test: config::tests::source_name_pattern_rejects_dots

SPEC-CF-SC-006: missing-root-directory-warns
  If `root` points to a nonexistent directory, emit a validation warning (not error).
  test: config::tests::source_missing_root_warns

---

## Top-level validator parsing

Validators are stateless functions. They are defined as top-level entries under `[validators]` — the middle layer of the composition model. Each validator declares what it does (type, command/prompt), what files it needs (input declarations), and nothing about orchestration. The validator name is the TOML key.

A validator's input declaration determines how the dispatch planner turns the file pool into invocations. There are four forms: no input (run once), per-file (run once per matching file), batch (run once with all matches), and multi-input (run once per matched key group). These are the only ways files enter a validator.

SPEC-CF-VP-001: validator-name-is-toml-key
  Validator name comes from the TOML key under `[validators]`. Must match `[A-Za-z0-9_-]+`.
  test: config::tests::validator_name_from_toml_key
  test: config::tests::validator_name_rejects_invalid_chars

SPEC-CF-VP-002: context-refs-removed
  The `context_refs` field is no longer recognized. If present, emit a config error directing the user to use `input` declarations instead.
  test: config::tests::context_refs_field_rejected

SPEC-CF-VP-003: input-form-no-input
  When `input` is absent and no `{file}` placeholders exist, the validator runs once with no files.
  test: config::tests::input_form_no_input

SPEC-CF-VP-004: input-form-per-file
  When `input` is a string, it is a glob pattern. Validator runs once per matching file.
  test: config::tests::input_form_per_file_glob

SPEC-CF-VP-005: input-form-batch
  When `input` is an object with `match` and `collect = true`, all matching files are passed at once.
  test: config::tests::input_form_batch

SPEC-CF-VP-006: input-form-named
  When `input` has named sub-keys (`input.code`, `input.spec`), each is a separate input slot with `match` or `path`.
  test: config::tests::input_form_named

SPEC-CF-VP-007: fixed-input-path-must-exist
  A named input with `path` (fixed input) must reference an existing file at validation time.
  test: config::tests::fixed_input_path_must_exist

SPEC-CF-VP-008: key-expression-must-be-valid
  Key expressions must be one of: `{stem}`, `{name}`, `{parent}`, `{relative:prefix/}`, `{regex:pattern}`. Unknown expressions are config errors.
  test: config::tests::key_expression_must_be_valid
  test: config::tests::key_expression_valid_forms

---

## Gate reference parsing

Gates are the orchestration layer — the top of the composition model. A gate lists which validators to run, in what order, with what sequencing rules. Gates don't touch files and don't define validators; they reference existing validators by name and add two orchestration concerns:

- **`blocking`**: Should a failure stop the pipeline? When a blocking validator fails, subsequent validators in the gate don't run. This is the primary sequencing mechanism.
- **`run_if`**: Should this validator run at all, given a prior validator's result? This adds value beyond blocking in two cases: (a) the dependency isn't the immediately preceding validator, or (b) the dependency is non-blocking (it produces a result but doesn't stop the pipeline, and a later validator should only run if it passed). For simple "stop on failure" sequencing, `blocking` alone is sufficient and `run_if` is unnecessary.

Validators are stateless functions. Gates are the only place where execution order and conditional logic live. The same validator can appear in multiple gates with different orchestration settings — the validator itself never changes.

SPEC-CF-GR-001: ref-must-name-existing-validator
  Each `ref` in a gate's `validators` array must match a key in the `[validators]` section. Missing ref is a config error.
  test: config::tests::gate_ref_must_name_existing_validator

SPEC-CF-GR-002: blocking-defaults-from-defaults-section
  When `blocking` is not set on a gate ref, it inherits from `[defaults].blocking`.
  test: config::tests::gate_ref_blocking_defaults_from_defaults
  test: config::tests::gate_ref_blocking_overrides_defaults

SPEC-CF-GR-003: run-if-references-validated
  `run_if` expressions in gate refs must reference validators that appear earlier in the same gate's validator list.
  test: config::tests::gate_ref_run_if_validated
  test: config::tests::gate_ref_run_if_forward_reference_rejected

SPEC-CF-GR-004: validator-reuse-across-gates
  The same validator can appear in multiple gates with different `blocking` and `run_if` settings.
  test: config::tests::validator_reuse_across_gates
