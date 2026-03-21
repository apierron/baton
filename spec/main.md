# module: main

CLI entry point for baton. Provides subcommand dispatch, argument parsing, config loading, and the glue between library modules (`config`, `exec`, `history`, `runtime`, `types`). All functions in this module are private except `main()`. Behavior is tested via integration tests in `tests/cli.rs` that invoke the compiled binary.

## Public entry point

| Function | Purpose |
|---|---|
| `main` | Parses CLI via clap, dispatches to `cmd_*` handler, calls `process::exit` with the handler's return code |

## Internal types

| Type | Purpose |
|---|---|
| `Cli` | clap `#[derive(Parser)]` struct; holds a single `Commands` subcommand |
| `Commands` | clap `#[derive(Subcommand)]` enum with 11 variants: Check, Init, List, History, ValidateConfig, CheckProvider, CheckRuntime, Clean, Version, Update, Uninstall |
| `ValidatorTypeStr` | Helper trait on `ValidatorConfig` for display strings in `cmd_list` dry-run output |

## Internal functions

| Function | Called by |
|---|---|
| `load_config` | All `cmd_*` functions that need baton.toml |
| `cmd_check` | `main` |
| `cmd_init` | `main` |
| `cmd_list` | `main` |
| `cmd_history` | `main` |
| `cmd_validate_config` | `main` |
| `check_single_provider` | `cmd_check_provider` |
| `cmd_check_provider` | `main` |
| `cmd_check_runtime` | `main` |
| `cmd_clean` | `main` |
| `cmd_version` | `main` |
| `detect_install_method` | `cmd_update`, `cmd_uninstall` (path inspection) |
| `cmd_update` | `main` |
| `cmd_uninstall` | `main` |

## Design notes

main.rs is a thin orchestration layer. It owns no domain logic — all validation, execution, and storage happen in library modules. The module's responsibilities are: (1) define the CLI grammar via clap derive macros, (2) translate CLI arguments into library calls, (3) handle I/O (stdin, stderr messaging, exit codes), and (4) provide user-facing commands for installation management (update, uninstall, clean).

Exit code conventions: 0 = success or passing verdict, 1 = user-recoverable error or failing verdict, 2 = infrastructure/config error. These are wired through each `cmd_*` function's `-> i32` return, and `main()` passes the value to `process::exit()`.

All stderr output uses `eprintln!` directly. Stdout is reserved for machine-parseable output (JSON verdicts, history lines, version info, gate listings).

---

## load_config

Loads and parses baton.toml from either an explicit `--config` path or by calling `discover_config` from the current directory.

SPEC-MN-LC-001: explicit-path-must-exist
  When `config_path` is `Some(p)` and `p` does not exist, returns `Err(ConfigError)` with "Config file not found" and the path.
  test: UNTESTED (integration test would need a nonexistent --config path)

SPEC-MN-LC-002: explicit-path-used-directly
  When `config_path` is `Some(p)` and it exists, the file at `p` is read and parsed. No discovery traversal occurs.
  test: cli::validate_config_with_explicit_config
  test: cli::version_with_explicit_config
  test: cli::clean_with_explicit_config

SPEC-MN-LC-003: none-triggers-discovery
  When `config_path` is `None`, `discover_config(&std::env::current_dir()?)` is called to find baton.toml by upward traversal.
  test: IMPLICIT via all cli tests that use `setup_project` without `--config`

SPEC-MN-LC-004: config-dir-derived-from-file-parent
  The config directory is set to the parent of the resolved config file path. This directory is passed to `parse_config` for resolving relative paths in the config.
  test: IMPLICIT via cli tests with explicit config in subdirectories

SPEC-MN-LC-005: returns-config-and-path
  On success, returns `Ok((BatonConfig, PathBuf))` — the parsed config and the resolved file path.
  test: IMPLICIT via all cli tests

---

## main

Parses `Cli` via clap and dispatches to the appropriate `cmd_*` handler. The handler's `i32` return becomes the process exit code via `process::exit()`.

SPEC-MN-MN-001: dispatch-all-subcommands
  All 11 `Commands` variants are matched exhaustively and routed to their corresponding handler function. No variant is silently ignored.
  test: IMPLICIT via per-subcommand cli tests

SPEC-MN-MN-002: exit-code-passthrough
  The exit code returned by the dispatched `cmd_*` function is passed directly to `process::exit()`. No transformation or clamping is applied.
  test: IMPLICIT via cli tests asserting specific exit codes

SPEC-MN-MN-003: version-from-cargo-pkg
  The clap `#[command]` attribute uses `env!("CARGO_PKG_VERSION")` as the version string. This is the single source of truth defined in Cargo.toml.
  test: cli::version_outputs_crate_version

---

## cmd_check

Core subcommand: loads config, builds artifact and context, runs the gate pipeline, stores the verdict in history, and outputs the result.

### Config and gate resolution

SPEC-MN-CK-001: config-error-exits-2
  If `load_config` fails, prints "Error: {e}" to stderr and returns exit code 2.
  test: IMPLICIT via cli tests with missing config

### Dry run

SPEC-MN-CK-040: dry-run-shows-invocation-plan-exits-0
  When `--dry-run` is set, prints the invocation plan (which validators would run, with what inputs) to stderr and returns exit code 0. No validators are executed, no verdict is produced, nothing is written to stdout.
  test: cli::dry_run_lists_validators_and_exits_zero

SPEC-MN-CK-041: dry-run-shows-skip-reasons
  In dry-run mode, validators excluded by `--only` or `--skip` are shown with their skip reason.
  test: cli::dry_run_shows_skip_reasons

SPEC-MN-CK-042: dry-run-shows-run-if-expressions
  In dry-run mode, validators with `run_if` expressions show the expression in parentheses.
  test: UNTESTED

### New CLI flags

SPEC-MN-CK-050: positional-args-are-input-files
  Zero or more positional args are treated as input files. Directories walked recursively.
  test: TODO

SPEC-MN-CK-051: only-accepts-selectors
  `--only` accepts gate names, `gate.validator` dot paths, and `@tag` selectors.
  test: TODO

SPEC-MN-CK-052: skip-accepts-selectors
  `--skip` accepts the same selector syntax as `--only`.
  test: TODO

SPEC-MN-CK-053: skip-applied-after-only
  `--skip` removes from whatever set `--only` selected.
  test: TODO

SPEC-MN-CK-054: diff-flag-adds-git-changed-files
  `--diff <refspec>` adds changed files to the input pool.
  test: TODO

SPEC-MN-CK-055: files-flag-reads-paths
  `--files <path | ->` reads newline-separated file paths.
  test: TODO

SPEC-MN-CK-056: no-positional-args-runs-project-level
  With no files, validators that don't need input run; those that do skip.
  test: TODO

### Removed flags

SPEC-MN-CK-060: gate-flag-removed
  `--gate` is removed. Use `--only <gate-name>`.
  test: TODO

SPEC-MN-CK-061: artifact-flag-removed
  `--artifact` is removed. Use positional args.
  test: TODO

SPEC-MN-CK-062: context-flag-removed
  `--context` is removed. Input is declared in baton.toml.
  test: TODO

SPEC-MN-CK-063: all-flag-removed
  `--all` is removed. Replacement TBD.
  test: TODO

SPEC-MN-CK-064: tags-flag-removed
  `--tags` is removed. Use `--only @tag` / `--skip @tag`.
  test: TODO

### Suppression flags

SPEC-MN-CK-070: suppress-warnings-adds-warn-to-suppressed
  `--suppress-warnings` adds `Status::Warn` to `RunOptions.suppressed_statuses`.
  test: cli::suppress_warnings_treats_warn_as_pass

SPEC-MN-CK-071: suppress-errors-adds-error-to-suppressed
  `--suppress-errors` adds `Status::Error` to `RunOptions.suppressed_statuses`.
  test: UNTESTED

SPEC-MN-CK-072: suppress-all-adds-warn-error-fail
  `--suppress-all` adds `Status::Warn`, `Status::Error`, and `Status::Fail` to `RunOptions.suppressed_statuses`.
  test: UNTESTED

### Gate execution and verdict

SPEC-MN-CK-080: run-gate-error-exits-2
  If `run_gate()` returns `Err`, prints "Error: {e}" and returns exit code 2.
  test: IMPLICIT via cli tests with broken validators

SPEC-MN-CK-081: verdict-stored-in-history
  When `options.log` is true, the verdict is stored via `history::init_db` + `history::store_invocation`. History directory is created if needed.
  test: cli::history_without_gate_filter (verifies stored verdict is queryable)

SPEC-MN-CK-082: no-log-skips-history
  When `--no-log` is set, `options.log` is false and history storage is skipped entirely.
  test: IMPLICIT via cli tests using `--no-log`

SPEC-MN-CK-083: history-errors-are-warnings
  If history database initialization or verdict storage fails, the error is printed as a "Warning:" to stderr. The command does NOT fail — the verdict is still output normally.
  test: UNTESTED

### Output formatting

SPEC-MN-CK-090: format-json-to-stdout
  `--format json` (the default) prints `verdict.to_json()` to stdout.
  test: IMPLICIT via most cli check tests (they parse JSON from stdout)

SPEC-MN-CK-091: format-human-to-stderr
  `--format human` prints `verdict.to_human()` to stderr.
  test: cli::format_human_on_stderr

SPEC-MN-CK-092: format-summary-to-stderr
  `--format summary` prints `verdict.to_summary()` to stderr.
  test: cli::format_summary_on_stderr

SPEC-MN-CK-093: unknown-format-falls-back-to-json
  An unrecognized `--format` value prints "Unknown format: {other}. Using json." to stderr and outputs JSON to stdout.
  test: UNTESTED

### Exit code from verdict

SPEC-MN-CK-100: exit-code-from-verdict-status
  The exit code is `verdict.status.exit_code()`: 0 for pass, 1 for fail, 2 for error.
  test: cli::check_pass
  test: cli::check_fail_exit_code_1

### Stdin cleanup

SPEC-MN-CK-110: stdin-temp-file-removed
  When the artifact was read from stdin, the temp file is removed after the verdict is output. Removal failure is silently ignored.
  test: UNTESTED (cleanup is best-effort)

---

## cmd_init

Scaffolds a new baton project: creates `baton.toml`, `.baton/` directory structure, and optionally starter prompt templates.

SPEC-MN-IN-001: existing-baton-toml-exits-1
  If `baton.toml` already exists in the current directory, prints "Error: baton.toml already exists. Will not overwrite." and returns exit code 1.
  test: cli::init_when_baton_toml_already_exists_returns_error

SPEC-MN-IN-002: creates-baton-dir-structure
  Creates `.baton/logs/` and `.baton/tmp/` directories via `create_dir_all`. If `.baton/` already exists, prints a warning and creates only missing subdirectories.
  test: cli::init_creates_valid_parseable_config

SPEC-MN-IN-003: writes-starter-baton-toml
  Writes a starter `baton.toml` with version, defaults section, commented-out provider, and an example gate. The generated config is valid and passes `validate-config`.
  test: cli::init_creates_valid_parseable_config

SPEC-MN-IN-004: creates-prompt-templates
  Unless `--minimal` is set, creates a `prompts/` directory with three starter templates: `spec-compliance.md`, `adversarial-review.md`, `doc-completeness.md`. Templates are loaded via `include_str!` from the `prompts/` directory at compile time.
  test: UNTESTED (init tests check config validity but not prompt file creation)

SPEC-MN-IN-005: minimal-skips-prompts
  When `--minimal` is set, the prompts directory and template files are not created.
  test: UNTESTED

SPEC-MN-IN-006: prompts-only-skips-config
  When `--prompts-only` is set, baton.toml and .baton/ are not created. Only the prompts directory and templates are written.
  test: UNTESTED

SPEC-MN-IN-007: existing-prompts-not-overwritten
  If a prompt template file already exists, it is skipped. Only missing templates are written.
  test: UNTESTED

SPEC-MN-IN-008: success-exits-0
  On success, prints "baton project initialized." to stderr and returns exit code 0.
  test: cli::init_creates_valid_parseable_config

---

## cmd_list

Lists available gates, or shows validators for a specific gate.

### Gate listing (no --gate)

SPEC-MN-LS-001: lists-all-gates
  Without `--gate`, prints "Available gates:" followed by one line per gate: name, description (or "(no description)"), and validator count. Output goes to stdout.
  test: cli::list_all_gates

SPEC-MN-LS-002: gates-in-btreemap-order
  Gates are listed in the order they appear in `config.gates`, which is a BTreeMap (alphabetical by key).
  test: IMPLICIT via cli::list_all_gates

### Validator detail (with --gate)

SPEC-MN-LS-010: gate-not-found-exits-1
  If the specified gate name is not found, prints "Error: Gate '{name}' not found." and returns exit code 1.
  test: UNTESTED

SPEC-MN-LS-011: shows-gate-name-and-description
  Prints "Gate: {name}" and, if present, "Description: {desc}" to stdout.
  test: UNTESTED

SPEC-MN-LS-012: shows-validator-details
  `cmd_list` now shows top-level validators from the `[validators]` section. The `--gate` flag shows which validators a gate references and with what `blocking`/`run_if` settings.
  test: cli::list_validators_for_specific_gate

### Config error

SPEC-MN-LS-020: config-error-exits-2
  If `load_config` fails, prints "Error: {e}" and returns exit code 2.
  test: IMPLICIT

---

## cmd_history

Queries and displays verdict history, optionally filtered by gate, status, or artifact hash.

SPEC-MN-HY-001: config-error-exits-2
  If `load_config` fails, returns exit code 2.
  test: IMPLICIT

SPEC-MN-HY-002: db-init-error-exits-2
  If `history::init_db` fails, returns exit code 2.
  test: UNTESTED

SPEC-MN-HY-003: file-flag-uses-query-by-file
  When `--file` is provided, `history::query_by_file` is called to search validator runs by file path.
  test: TODO

SPEC-MN-HY-004: hash-flag-uses-query-by-hash
  When `--hash` is provided, `history::query_by_hash` is called to search validator runs by content hash.
  test: TODO

SPEC-MN-HY-005: invocation-flag-uses-query-invocation
  When `--invocation <id>` is provided, `history::query_invocation` is called for detail on a specific invocation.
  test: TODO

SPEC-MN-HY-006: default-uses-query-recent
  Without `--file`, `--hash`, or `--invocation`, calls `history::query_recent` with the limit, gate, and status filters.
  test: cli::history_without_gate_filter
  test: cli::history_respects_limit

SPEC-MN-HY-007: empty-results-prints-message
  When no verdicts are found, prints "No verdicts found." to stdout and returns exit code 0.
  test: UNTESTED

SPEC-MN-HY-008: result-format
  Output format reflects the new schema fields (invocation-based, not verdict-based).
  test: cli::history_without_gate_filter

SPEC-MN-HY-009: default-limit-is-20
  The `--limit` flag defaults to 20 when omitted.
  test: IMPLICIT via clap default_value

---

## cmd_validate_config

Validates baton.toml and reports any errors or warnings.

SPEC-MN-VC-001: parse-error-exits-1
  If `load_config` fails (TOML parse error, file not found, etc.), prints "Error: {e}" and returns exit code 1.
  test: UNTESTED

SPEC-MN-VC-002: no-issues-exits-0
  When validation produces no errors and no warnings, prints "Config OK: {path}" to stderr and returns exit code 0.
  test: cli::validate_config_with_explicit_config

SPEC-MN-VC-003: warnings-printed-to-stderr
  Each warning is printed as "Warning: {w}" to stderr.
  test: UNTESTED

SPEC-MN-VC-004: errors-printed-to-stderr
  Each error is printed as "Error: {e}" to stderr.
  test: UNTESTED

SPEC-MN-VC-005: errors-exit-1-warnings-exit-0
  If `validation.has_errors()` is true, returns exit code 1. If only warnings are present (no errors), returns exit code 0.
  test: UNTESTED

---

## check_single_provider

Tests connectivity to a single LLM provider. Uses `ProviderClient` from the provider module for all HTTP interactions.

SPEC-MN-SP-001: missing-api-key-returns-false
  If ProviderClient::new returns ApiKeyNotSet, prints the env var error and returns false.
  test: UNTESTED

SPEC-MN-SP-002: empty-api-key-env-skips-key-check
  Handled by ProviderClient::new — empty api_key_env means no auth.
  test: UNTESTED

SPEC-MN-SP-003: models-endpoint-auth-failure
  If list_models returns AuthFailed, prints "Authentication failed" and returns false.
  test: UNTESTED

SPEC-MN-SP-004: models-endpoint-timeout
  If list_models returns Timeout, prints "connection timed out" and returns false.
  test: UNTESTED

SPEC-MN-SP-005: model-found-in-list
  If list_models succeeds and the default model is in the list, prints "OK" and returns true.
  test: UNTESTED

SPEC-MN-SP-006: model-not-found-in-list
  If list_models succeeds but the model is absent, prints "WARN" with available models. Returns true.
  test: UNTESTED

SPEC-MN-SP-007: fallback-test-completion
  If list_models returns a non-auth/non-connectivity error, falls through to test_completion.
  test: UNTESTED

---

## cmd_check_provider

Checks connectivity for one or all configured LLM providers.

SPEC-MN-CP-001: no-providers-exits-1
  If `config.providers` is empty, prints "No providers configured in baton.toml." and returns exit code 1.
  test: UNTESTED

SPEC-MN-CP-002: all-flag-checks-every-provider
  `--all` iterates over every provider in the config.
  test: UNTESTED

SPEC-MN-CP-003: named-provider-not-found-exits-1
  If the named provider is not in the config, prints "Error: Provider '{name}' not found. Available providers: ..." and returns exit code 1.
  test: UNTESTED

SPEC-MN-CP-004: default-checks-first-provider
  When neither `--all` nor a name is specified, checks only the first provider (by BTreeMap iteration order).
  test: UNTESTED

SPEC-MN-CP-005: any-failure-exits-1
  If any checked provider fails, returns exit code 1. All providers are still checked (no short-circuit).
  test: UNTESTED

SPEC-MN-CP-006: all-pass-exits-0
  If all checked providers pass, returns exit code 0.
  test: UNTESTED

---

## cmd_check_runtime

Checks health for one or all configured agent runtimes.

SPEC-MN-CR-001: no-runtimes-exits-1
  If `config.runtimes` is empty, prints "No runtimes configured in baton.toml." and returns exit code 1.
  test: UNTESTED

SPEC-MN-CR-002: all-flag-checks-every-runtime
  `--all` iterates over every runtime in the config.
  test: UNTESTED

SPEC-MN-CR-003: named-runtime-not-found-exits-1
  If the named runtime is not in the config, prints "Error: Runtime '{name}' not found. Available runtimes: ..." and returns exit code 1.
  test: UNTESTED

SPEC-MN-CR-004: default-checks-first-runtime
  When neither `--all` nor a name is specified, checks only the first runtime.
  test: UNTESTED

SPEC-MN-CR-005: adapter-creation-failure-continues
  If `create_adapter` fails for a runtime, prints the error, marks it as failed, and continues to the next runtime.
  test: UNTESTED

SPEC-MN-CR-006: health-check-reachable
  If `health_check` returns `Ok` with `reachable: true`, prints "OK" with version info (if available).
  test: UNTESTED

SPEC-MN-CR-007: health-check-unreachable
  If `health_check` returns `Ok` with `reachable: false`, prints "ERROR" with the message.
  test: UNTESTED

SPEC-MN-CR-008: health-check-error
  If `health_check` returns `Err`, prints "ERROR: health check failed" with the error.
  test: UNTESTED

SPEC-MN-CR-009: any-failure-exits-1
  If any runtime check fails, returns exit code 1.
  test: UNTESTED

---

## cmd_clean

Removes stale temporary files (older than 1 hour) from the configured tmp directory.

SPEC-MN-CL-001: no-tmp-dir-exits-0
  If the tmp directory does not exist, prints "No temporary files to clean." and returns exit code 0.
  test: UNTESTED

SPEC-MN-CL-002: stale-threshold-one-hour
  Only files whose modification time is more than 1 hour old (3600 seconds) are considered stale.
  test: UNTESTED

SPEC-MN-CL-003: dry-run-reports-without-deleting
  When `--dry-run` is set, prints "Would remove: {path}" for each stale file but does not delete. Final message says "{N} file(s) would be removed."
  test: UNTESTED

SPEC-MN-CL-004: actual-clean-removes-files
  Without `--dry-run`, stale files are removed via `std::fs::remove_file`. Each removal is reported as "Removed: {path}". Final message says "{N} file(s) removed."
  test: UNTESTED

SPEC-MN-CL-005: no-stale-files-message
  If no files exceed the age threshold, prints "No stale files to clean."
  test: cli::clean_with_explicit_config

SPEC-MN-CL-006: always-exits-0
  cmd_clean always returns exit code 0, regardless of how many files were cleaned.
  test: cli::clean_with_explicit_config

---

## cmd_version

Prints baton version, spec version, and config file location.

SPEC-MN-VR-001: prints-version-from-cargo
  First line is "baton {version}" where version comes from `env!("CARGO_PKG_VERSION")`.
  test: cli::version_outputs_crate_version

SPEC-MN-VR-002: prints-spec-version
  Second line is "spec version: 0.5" (hardcoded).
  test: UNTESTED

SPEC-MN-VR-003: config-found
  If `load_config` succeeds, prints "config: {path} (found)".
  test: cli::version_with_explicit_config

SPEC-MN-VR-004: config-not-found
  If `load_config` fails, prints "config: not found".
  test: UNTESTED

SPEC-MN-VR-005: always-exits-0
  cmd_version always returns exit code 0, regardless of config discovery result.
  test: cli::version_outputs_crate_version

---

## detect_install_method

Inspects the current executable path to determine the installation method.

SPEC-MN-DI-001: cargo-detection
  If the executable path contains `$CARGO_HOME/bin/` (or `~/.cargo/bin/` as fallback), returns `("cargo", path)`.
  test: UNTESTED

SPEC-MN-DI-002: homebrew-detection
  If the path contains `/Cellar/`, `/homebrew/`, `/opt/homebrew/`, or `/usr/local/bin/`, AND `brew list baton` succeeds, returns `("homebrew", path)`.
  test: UNTESTED

SPEC-MN-DI-003: binary-fallback
  If neither cargo nor homebrew is detected, returns `("binary", path)`.
  test: UNTESTED

---

## cmd_update

Downloads and installs a new baton binary from GitHub releases. Defers to package-manager-specific instructions for cargo and homebrew installations.

SPEC-MN-UP-001: cargo-install-defers-exits-1
  If install method is "cargo", prints cargo update instructions and returns exit code 1 without modifying anything.
  test: UNTESTED

SPEC-MN-UP-002: homebrew-install-defers-exits-1
  If install method is "homebrew", prints brew update instructions and returns exit code 1.
  test: UNTESTED

SPEC-MN-UP-003: binary-install-downloads-from-github
  For "binary" installs, fetches the latest (or specified) release from GitHub, downloads the platform-appropriate archive, extracts it, and replaces the current executable.
  test: UNTESTED

SPEC-MN-UP-004: confirmation-prompt
  Unless `--yes` is set, prompts the user for confirmation before downloading. Aborts on anything other than "y" or "yes".
  test: UNTESTED

SPEC-MN-UP-005: specific-version-flag
  `--version` allows targeting a specific release tag instead of latest.
  test: UNTESTED

SPEC-MN-UP-006: already-up-to-date-exits-0
  If the current version matches the release version, prints "Already up to date" and returns exit code 0.
  test: UNTESTED

---

## cmd_uninstall

Removes baton binaries from the system.

SPEC-MN-UN-001: current-exe-always-targeted
  The currently running binary is always included in the removal targets.
  test: UNTESTED

SPEC-MN-UN-002: all-flag-finds-additional-locations
  `--all` searches install-script location (`$BATON_INSTALL_DIR` or `~/.local/bin/baton`), cargo location (`$CARGO_HOME/bin/baton`), and homebrew location. Duplicates (same canonicalized path) are deduplicated.
  test: UNTESTED

SPEC-MN-UN-003: cargo-uninstall-attempted-first
  If a cargo installation is found, `cargo uninstall baton` is attempted. On success, the binary is removed from the target list to avoid double-deletion.
  test: UNTESTED

SPEC-MN-UN-004: homebrew-warning
  If a homebrew installation is detected, prints a note advising `brew uninstall baton` for a clean Homebrew removal.
  test: UNTESTED

SPEC-MN-UN-005: confirmation-prompt
  Unless `--yes` is set, prompts the user for confirmation. Aborts on anything other than "y" or "yes".
  test: UNTESTED

SPEC-MN-UN-006: self-delete-last
  The currently running binary is deleted last. On Unix, this works because the OS keeps the inode alive. On Windows, the binary is renamed first, then deleted.
  test: UNTESTED

SPEC-MN-UN-007: partial-failure-exits-1
  If any binary removal fails, returns exit code 1. Successful removals are still reported.
  test: UNTESTED

SPEC-MN-UN-008: full-success-exits-0
  If all removals succeed, prints "baton has been uninstalled." and returns exit code 0.
  test: UNTESTED
