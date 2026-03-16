# module: config

Configuration parsing and validation for baton.toml. Two-stage design: `parse_config` deserializes TOML into validated structures, `validate_config` checks semantic correctness (e.g., forward references, missing providers, undefined context slots).

The two-stage split exists because parse-time checks are structural (does this TOML describe a valid config?) while validation checks are semantic (do cross-references resolve? are environment variables set?). This matters because a config can be structurally valid but semantically broken -- a run_if that references a validator defined later in the pipeline parses fine but is semantically invalid.

## Public functions

| Function           | Purpose                                              |
|--------------------|------------------------------------------------------|
| `parse_config`     | Parse TOML string into BatonConfig, validates structure |
| `validate_config`  | Check semantic correctness (forward refs, missing providers, etc) |
| `split_run_if`     | Tokenize run_if expressions into atoms and operators |
| `discover_config`  | Find baton.toml by walking up from start_dir         |

## Internal functions

| Function               | Called by          |
|------------------------|--------------------|
| `validate_run_if_expr` | `validate_config`  |

## Design notes

parse_config returns `Result<BatonConfig>` (early-return on first error) while validate_config returns `ConfigValidation` (accumulates all errors and warnings). This is deliberate: parse-time errors are fatal and sequential (later checks depend on earlier ones succeeding), while validation errors are independent and should all be reported at once so the user can fix them in a single pass.

validate_config distinguishes errors (will cause runtime failure) from warnings (suspicious but functional). Warnings are printed but do not prevent execution.

Provider api_base has env var resolution applied during parsing, not at execution time. This means the env var must be set when the config is loaded, not when the validator runs. This is consistent with the "fail early" philosophy.

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

SPEC-CF-PC-002: version-must-be-0-4
  The `version` field must be exactly the string "0.4". Any other value, including "0.3", "0.5", "1.0", or an empty string, returns ConfigError containing the rejected version string.
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

Providers are iterated from the `[providers]` map. For each provider, env vars in api_base are resolved and trailing slashes are stripped. The resolved provider is stored in a BTreeMap on the config.

SPEC-CF-PC-020: provider-api-base-env-vars-resolved
  Environment variable references in `api_base` (e.g., `${VAR}`) are resolved via `resolve_env_vars` at parse time. If resolution fails, parse_config returns ConfigError naming the provider.
  test: UNTESTED (no test for env var resolution in api_base)

SPEC-CF-PC-021: provider-api-base-trailing-slash-stripped
  After env var resolution, if `api_base` ends with '/', the trailing slash is removed. Only a single trailing slash is stripped (the code calls `pop()` once). This normalizes URLs so downstream code can append paths without double-slash issues.
  test: config::tests::provider_trailing_slash_stripped

SPEC-CF-PC-022: provider-fields-preserved
  The `api_key_env` and `default_model` fields are stored verbatim from the TOML. No validation is performed on their values at parse time (api_key_env is validated later in validate_config).
  test: IMPLICIT via config::tests::parse_full_config

### parse_config: runtime parsing

Runtimes are iterated from the `[runtimes]` map and stored with their defaults applied by serde.

SPEC-CF-PC-025: runtime-defaults
  Runtime entries default to sandbox=true, timeout_seconds=600, max_iterations=30 when those fields are omitted.
  test: config::tests::runtime_defaults

SPEC-CF-PC-026: runtime-fields-preserved
  Runtime type, base_url, api_key_env, and default_model are stored verbatim from TOML.
  test: config::tests::runtime_fields_stored_verbatim

### parse_config: gate and validator parsing

Each gate must have at least one validator. Validators are validated in declaration order within each gate. A HashSet tracks seen names for duplicate detection.

The validator name regex check and duplicate check happen before type-specific field validation. This means a validator with an invalid name is rejected before the parser checks whether it has a command or prompt field.

SPEC-CF-PC-030: gate-must-have-validators
  A gate with an empty validators array returns ConfigError with the gate name and "no validators".
  test: config::tests::parse_gate_no_validators

SPEC-CF-PC-031: validator-name-must-match-pattern
  Validator names must match `[A-Za-z0-9_-]+` (ASCII alphanumeric, underscore, hyphen). An empty name or a name containing spaces, punctuation, or other characters returns ConfigError containing "invalid characters".
  test: config::tests::invalid_validator_name

SPEC-CF-PC-032: validator-names-unique-within-gate
  Validator names must be unique within a single gate. A duplicate name returns ConfigError containing "duplicate" and both the gate name and validator name. Duplicate detection uses a HashSet with case-sensitive comparison; the first occurrence wins and the second triggers the error.
  test: config::tests::duplicate_validator_name

SPEC-CF-PC-033: validator-type-must-be-known
  The `type` field must be "script", "llm", or "human". Any other value returns ConfigError containing "unknown type" and the rejected value.
  test: config::tests::unknown_validator_type

SPEC-CF-PC-034: script-requires-command
  A validator with type "script" must have a `command` field. If missing, returns ConfigError containing "command".
  test: config::tests::script_missing_command

SPEC-CF-PC-035: llm-requires-prompt
  A validator with type "llm" must have a `prompt` field. If missing, returns ConfigError containing "prompt".
  test: config::tests::llm_missing_prompt

SPEC-CF-PC-036: human-requires-prompt
  A validator with type "human" must have a `prompt` field. If missing, returns ConfigError containing "prompt".
  test: config::tests::human_missing_prompt

SPEC-CF-PC-037: mode-defaults-to-completion
  When `mode` is omitted or set to "completion", the validator gets LlmMode::Completion. When set to "session", it gets LlmMode::Session. Any other value returns ConfigError containing "invalid mode" and the rejected value.
  test: config::tests::invalid_mode_string

SPEC-CF-PC-038: response-format-defaults-to-verdict
  When `response_format` is omitted or set to "verdict", the validator gets ResponseFormat::Verdict. When set to "freeform", it gets ResponseFormat::Freeform. Any other value returns ConfigError containing "invalid response_format".
  test: config::tests::invalid_response_format

SPEC-CF-PC-039: warn-exit-codes-rejects-zero
  The `warn_exit_codes` array must not contain 0. Exit code 0 is unconditionally "pass" and cannot be reclassified as a warning. If 0 is present, returns ConfigError with "warn_exit_codes must not contain 0".
  test: config::tests::warn_exit_codes_contains_zero

SPEC-CF-PC-040: validator-inherits-blocking-from-defaults
  When `blocking` is not set on a validator (Option is None), it inherits the value from `defaults.blocking`. When explicitly set, the validator's value takes precedence via `unwrap_or`.
  test: config::tests::defaults_applied
  test: config::tests::validator_overrides_defaults

SPEC-CF-PC-041: validator-inherits-timeout-from-defaults
  When `timeout_seconds` is not set on a validator (Option is None), it inherits the value from `defaults.timeout_seconds`. When explicitly set, the validator's value takes precedence.
  test: config::tests::defaults_applied
  test: config::tests::validator_overrides_defaults

SPEC-CF-PC-042: provider-defaults-to-default
  When `provider` is not set on a validator, it defaults to the string "default". This means an LLM validator without an explicit provider will look up providers["default"] at validation and execution time.
  test: config::tests::default_provider

SPEC-CF-PC-043: temperature-defaults-to-zero
  When `temperature` is not set on a validator, it defaults to 0.0. This is a deliberate choice for reproducibility in code review tasks.
  test: IMPLICIT via config::tests::parse_full_config (asserts temperature == 0.0)

SPEC-CF-PC-044: context-slots-parsed
  Gate context slots are parsed into a BTreeMap with description and required fields. The BTreeMap ordering is deterministic (alphabetical by key).
  test: IMPLICIT via config::tests::parse_full_config (asserts context["spec"].required)

SPEC-CF-PC-045: config-dir-stored
  The config_dir path is stored on BatonConfig for later use in path resolution (e.g., resolving working_dir references at execution time).
  test: config::tests::config_dir_stored

SPEC-CF-PC-046: validator-name-uniqueness-is-per-gate
  Duplicate name detection resets for each gate (the seen_names HashSet is created inside the per-gate loop). Two different gates may each have a validator named "lint" without error.
  test: config::tests::duplicate_name_across_gates_is_ok

SPEC-CF-PC-047: parse-errors-are-early-return
  parse_config returns on the first structural error encountered. If a config has multiple problems (e.g., wrong version AND empty gates), only the first error is reported. Check order is: TOML syntax, version, empty gates, then per-gate in BTreeMap order (empty validators, per-validator checks in declaration order).
  test: UNTESTED (no test verifies error ordering when multiple errors exist)

SPEC-CF-PC-048: empty-validator-name-rejected
  An empty string for a validator name fails the `[A-Za-z0-9_-]+` check because the check requires `!raw_v.name.is_empty()`. The error message still says "invalid characters" even though the real problem is emptiness.
  test: config::tests::empty_validator_name

---

## validate_config

Checks semantic correctness of a parsed BatonConfig. Returns a `ConfigValidation` containing accumulated errors and warnings. Unlike parse_config, this function does not short-circuit -- all errors and warnings are collected.

### Sections

1. Per-gate, per-validator checks (run_if, context_refs, provider, mode/runtime, freeform+blocking)
2. Provider API key environment variable checks

### validate_config: per-validator checks

Iterates every gate (in BTreeMap order) and every validator within each gate (in pipeline order). For each validator, checks run_if references, context_refs, and (for LLM validators only) provider/mode/runtime/response_format semantics.

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

SPEC-CF-VC-005: context-refs-must-reference-defined-slots
  Each entry in a validator's `context_refs` must correspond to a key in the gate's context map. Undefined context references produce an error naming the undefined slot and the gate.
  test: config::tests::validate_context_refs_undefined

SPEC-CF-VC-006: llm-provider-must-be-defined
  For LLM validators, if the provider is not "default" and is not present in the config's providers map, an error is produced. The special name "default" is exempt -- it is resolved at execution time, not at validation time. Script and human validators are not checked.
  test: config::tests::undefined_non_default_provider

SPEC-CF-VC-007: session-mode-requires-runtime
  An LLM validator with mode "session" must have a runtime field set. If runtime is None, an error is produced containing "runtime".
  test: config::tests::validate_session_without_runtime

SPEC-CF-VC-008: completion-mode-with-runtime-warns
  An LLM validator with mode "completion" that also sets a runtime field produces a warning containing "ignored in completion mode". This is not an error because the config is functional -- the runtime is simply unused.
  test: config::tests::validate_completion_with_runtime_warning

SPEC-CF-VC-009: runtime-reference-must-be-defined
  When an LLM validator specifies a runtime, that runtime name must exist in the config's runtimes map. An undefined runtime produces an error containing "not defined in [runtimes]".
  test: config::tests::undefined_runtime_reference

SPEC-CF-VC-010: freeform-with-blocking-warns
  An LLM validator with response_format "freeform" and blocking=true produces a warning containing "blocking has no effect". Freeform validators always return warn status, so blocking (which triggers gate failure on fail/error) is meaningless.
  test: config::tests::validate_freeform_blocking_warning

SPEC-CF-VC-011: validation-checks-only-llm-validators
  Provider, mode/runtime, and freeform/blocking checks are gated behind `val.validator_type == ValidatorType::Llm`. Script and human validators skip these checks entirely, even if they have stray LLM fields set (which would be ignored at execution time).
  test: config::tests::script_validator_with_provider_not_flagged

### validate_config: provider API key checks

After all per-validator checks, validate_config iterates every provider in the config and checks that the referenced environment variable is set.

SPEC-CF-VC-020: provider-api-key-env-must-be-set
  For each provider in the config, if `api_key_env` is non-empty and the named environment variable is not set (std::env::var returns Err), an error is produced containing the provider name and the env var name. If `api_key_env` is empty, this check is skipped.
  test: config::tests::api_key_env_validation

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
