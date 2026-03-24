# module: main + commands

CLI entry point and per-command handlers for baton. `main.rs` owns the CLI grammar and dispatch; each subcommand's logic lives in its own file under `src/commands/`. All `cmd_*` functions and helpers are private to their respective modules. Behavior is tested via integration tests in `tests/cli.rs` that invoke the compiled binary.

## Public entry point

| Function | Purpose |
|---|---|
| `main` | Parses CLI via clap, dispatches to `cmd_*` handler in `commands/`, calls `process::exit` with the handler's return code |

## Types in `main.rs`

| Type | Purpose |
|---|---|
| `Cli` | clap `#[derive(Parser)]` struct; holds a single `Commands` subcommand |
| `Commands` | clap `#[derive(Subcommand)]` enum with 10 variants: Add, Check, Doctor, Init, List, History, Clean, Version, Update, Uninstall |

## Shared helpers in `commands/mod.rs`

| Item | Used by |
|---|---|
| `load_config` | All `cmd_*` functions that need baton.toml |
| `ValidatorTypeStr` trait | `cmd_check` (dry-run), `cmd_list` |
| `detect_install_method` | `cmd_update`, `cmd_doctor` |

## Per-command modules

| File | Function | Notes |
|---|---|---|
| `commands/add.rs` | `cmd_add` | Interactive/flag/import modes; full TOML editing |
| `commands/check.rs` | `cmd_check` | Gate execution, history logging, output formatting |
| `commands/init.rs` | `cmd_init` | Project scaffold; embedded config/prompt templates |
| `commands/list.rs` | `cmd_list` | Gate and validator listing |
| `commands/history.rs` | `cmd_history` | SQLite history query and display |
| `commands/doctor.rs` | `cmd_doctor` | Comprehensive health check (installation, config, structure, prompts, env, runtimes) |
| `commands/clean.rs` | `cmd_clean` | Stale temp file removal |
| `commands/version.rs` | `cmd_version` | Version and config location display |
| `commands/update.rs` | `cmd_update` | GitHub release download and atomic binary replace |
| `commands/uninstall.rs` | `cmd_uninstall` | Binary removal across install locations |

## Design notes

`main.rs` is a thin orchestration layer (~270 lines): CLI grammar definitions and a dispatch `match`. All domain logic lives in library modules (`baton::exec`, `baton::config`, etc.) or in the per-command modules under `src/commands/`. The `commands/` directory is binary-only and does not appear in `lib.rs`.

The module's responsibilities: (1) define the CLI grammar via clap derive macros, (2) translate CLI arguments into `commands/*` calls, (3) call `process::exit` with the returned exit code.

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
  All 10 `Commands` variants are matched exhaustively and routed to their corresponding handler function. No variant is silently ignored.
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
  Writes a starter `baton.toml` by concatenating the base config template (`defaults/configs/base.toml`) with a profile-specific template. When `--profile` is provided, uses that profile; otherwise defaults to `generic`. The generated config is valid and passes `validate-config`. All generated configs use the separate-block style: validators as top-level `[validators.X]` blocks, gates referencing them via `{ ref = "X" }`.
  test: cli::init_creates_valid_parseable_config

SPEC-MN-IN-004: creates-prompt-templates
  Unless `--minimal` is set, creates a `prompts/` directory with three starter templates: `spec-compliance.md`, `adversarial-review.md`, `doc-completeness.md`. Templates are loaded via `include_str!` from `defaults/prompts/` at compile time.
  test: cli::init_creates_prompt_templates

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

SPEC-MN-IN-009: profile-selects-config
  When `--profile {rust,python,generic}` is provided, the starter `baton.toml` uses the named profile's config template appended to the base template. When omitted, defaults to `generic`.
  test: cli::init_profile_rust, cli::init_profile_python

SPEC-MN-IN-010: starter-uses-separate-blocks
  All generated `baton.toml` files use the separate validator block style: `[validators.X]` defined top-level, gates reference via `{ ref = "X" }`. No generated config uses the inline/nested style.
  test: cli::init_default_uses_separate_blocks

SPEC-MN-IN-011: unknown-profile-exits-1
  When `--profile` is provided with an unrecognized value, prints an error listing valid profiles and returns exit code 1.
  test: cli::init_unknown_profile_exits_1

### Interactive mode

SPEC-MN-IN-020: tty-no-flags-enters-interactive
  When stdin is a TTY and no --profile, --minimal, or --prompts-only flags are provided,
  cmd_init enters interactive mode using dialoguer prompts.
  test: MANUAL (requires TTY)

SPEC-MN-IN-021: non-tty-uses-defaults
  When stdin is not a TTY and no flags are provided, cmd_init uses the default
  behavior (generic profile, prompts included) without prompting.
  test: cli::init_no_flags_non_tty_uses_generic_with_prompts

SPEC-MN-IN-022: flags-skip-interactive
  When any of --profile, --minimal, or --prompts-only are provided, interactive
  mode is skipped regardless of TTY status. The flags are used directly.
  test: cli::init_flags_override_interactive

SPEC-MN-IN-023: interactive-code-validators-prompt
  In interactive mode, the first prompt asks "Include code validators?" (Confirm, default yes).
  If the user selects no, only base.toml content is written (no validators or gates).
  test: MANUAL (requires TTY)

SPEC-MN-IN-024: interactive-language-prompt
  When the user selects yes for code validators, a Select prompt asks
  "Which language?" with options [Rust, Python, Generic]. The selection maps
  to the corresponding profile config from defaults/configs/.
  test: MANUAL (requires TTY)

SPEC-MN-IN-025: interactive-prompts-prompt
  The final prompt asks "Include starter prompt templates?" (Confirm, default yes).
  Yes creates the prompts/ directory with templates; no skips it (equivalent to --minimal).
  test: MANUAL (requires TTY)

SPEC-MN-IN-026: base-only-config-valid-toml
  When code validators are declined, the generated baton.toml contains only the base
  config (version and defaults section). This config is valid TOML, contains version
  and [defaults], and has no [validators] or [gates] sections. Users are expected to
  add validators via `baton add` afterward.
  test: cli::init_base_only_config_valid

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

## cmd_doctor

Runs a comprehensive health check across installation, config, project structure, prompt templates, environment variables, and runtimes. All output goes to stderr. Exit code is 0 if all checks pass or warn, 1 if any fail.

### Sections

1. Installation
2. Configuration
3. Project Structure
4. Prompt Templates
5. Environment
6. Runtimes

Output is grouped by section with numbered headers (`[1/6] Installation`, etc.). Each check within a section gets a status prefix: `[ok]`, `[warn]`, `[fail]`, or `[skip]`. A summary line is printed at the end.

SPEC-MN-DR-001: installation-always-runs
  Section 1 (Installation) runs unconditionally, even if config cannot be loaded.
  Reports baton version from `env!("CARGO_PKG_VERSION")` and install method from
  `detect_install_method()`. Always `[ok]`.
  test: UNTESTED

SPEC-MN-DR-002: config-discovery-reported
  Section 2 attempts to load config via `load_config()`. If successful, reports
  the file path with `[ok]`. If discovery/parse fails, reports the error with `[fail]`.
  test: UNTESTED

SPEC-MN-DR-003: config-validation-reported
  When config loads successfully, `validate_config()` is called. Errors are reported
  as `[fail]`, warnings as `[warn]`, and clean validation as `[ok]`.
  test: UNTESTED

SPEC-MN-DR-004: sections-skip-without-config
  If config loading fails in section 2, sections 3-6 each show a single
  `[skip]` "Requires valid configuration" line.
  test: UNTESTED

SPEC-MN-DR-005: project-structure-checks-directories
  Section 3 checks that `prompts_dir`, `log_dir`, and `tmp_dir` exist as directories.
  Missing directories are reported as `[fail]`.
  test: UNTESTED

SPEC-MN-DR-006: history-db-writable-check
  Section 3 checks `history_db`. If the file exists, `[ok]`. If the file is missing
  but the parent directory exists and is writable, `[warn]` "will be created on first run".
  If the parent directory is missing or not writable, `[fail]`.
  test: UNTESTED

SPEC-MN-DR-007: prompt-templates-resolved
  Section 4 iterates all LLM validators' prompt fields. File references are resolved
  via `prompt::resolve_prompt_value()`. Successful resolution is `[ok]`. Resolution
  failure is `[fail]` with the error message. Duplicate prompt references are checked once.
  test: UNTESTED

SPEC-MN-DR-008: no-prompt-refs-shows-ok
  When no LLM validators reference prompt files, section 4 shows
  `[ok]` "No prompt file references to check".
  test: UNTESTED

SPEC-MN-DR-009: env-vars-for-runtimes-checked
  Section 5 checks `api_key_env` for each runtime. Set and non-empty is `[ok]`.
  Unset or empty is `[fail]` with the runtime name and env var name.
  Runtimes with no `api_key_env` are skipped silently.
  test: UNTESTED

SPEC-MN-DR-010: runtime-health-all-types
  Section 6 calls `health_check()` on every runtime in `config.runtimes` (all types,
  not just api). Reachable is `[ok]` with type and version if available. Unreachable
  or error is `[fail]`.
  test: UNTESTED (requires mock server or real runtime)

SPEC-MN-DR-011: offline-skips-runtimes
  When `--offline` is set, section 6 shows `[skip]` for each runtime with
  "Skipped (--offline)" as the reason.
  test: UNTESTED

SPEC-MN-DR-012: exit-0-no-fails
  Returns exit code 0 when all checks are `[ok]`, `[warn]`, or `[skip]`.
  test: UNTESTED

SPEC-MN-DR-013: exit-1-any-fail
  Returns exit code 1 when at least one check is `[fail]`.
  test: UNTESTED

SPEC-MN-DR-014: summary-line-printed
  Final line shows "Summary: N ok, N warn, N fail" on stderr.
  test: UNTESTED

SPEC-MN-DR-015: all-output-to-stderr
  All doctor output goes to stderr. Nothing is written to stdout.
  test: UNTESTED

SPEC-MN-DR-016: no-env-vars-shows-ok
  When no runtimes have `api_key_env` configured, section 5 shows
  `[ok]` "No environment variables to check".
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
