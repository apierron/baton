# module: add (cmd_add in main)

Interactive and non-interactive addition of validators to baton.toml. Three modes: interactive wizard, non-interactive (CLI flags), and import (from file/URL). Modifies baton.toml in place, preserving existing content, comments, and formatting via `toml_edit`.

## Public functions

| Function | Purpose |
|---|---|
| `cmd_add` | CLI dispatch (in main.rs) — resolves mode, delegates to add module |
| `run_wizard` | Interactive wizard using dialoguer prompts |
| `build_from_flags` | Build validator from CLI flags (non-interactive mode) |
| `run_import` | Fetch and parse import source, extract validators |
| `apply_edits` | Modify TOML document, validate, write atomically |
| `find_config` | Locate and read baton.toml |
| `write_config` | Write modified TOML atomically via temp file + rename |

---

## Mode detection

SPEC-MN-AD-001: from-flag-triggers-import
  When `--from` is provided, the command operates in import mode. No interactive prompts are shown.
  test: cli::add_from_file
  test: cli::add_from_file_single_format

SPEC-MN-AD-002: type-and-name-trigger-noninteractive
  When both `--type` and `--name` are provided (without `--from`), the command operates in non-interactive mode.
  test: cli::add_noninteractive_script_success
  test: cli::add_noninteractive_script_with_options

SPEC-MN-AD-003: bare-command-triggers-wizard
  When neither `--from` nor `--type`+`--name` are provided, the command enters interactive wizard mode.
  test: cli::add_no_tty_no_flags_exits_1 (verifies wizard path is entered — fails because no TTY)

SPEC-MN-AD-004: non-tty-without-flags-exits-error
  If stdin is not a TTY and no mode-determining flags are provided, prints an error suggesting non-interactive flags and exits with error status.
  test: cli::add_no_tty_no_flags_exits_1

---

## Config file requirements

SPEC-MN-AD-010: requires-existing-baton-toml
  If no baton.toml is found (via --config or discovery), prints "No baton.toml found. Run `baton init` first." and exits with error status.
  test: cli::add_missing_config_exits_2
  test: add::tests::find_config_missing_explicit_path

SPEC-MN-AD-011: duplicate-name-rejected
  If a validator with the given name already exists in the config, prints an error naming the duplicate and exits. No changes are written.
  test: cli::add_duplicate_name_exits_1
  test: add::tests::apply_edits_rejects_duplicate_name

---

## Non-interactive mode

SPEC-MN-AD-020: script-requires-command
  In non-interactive mode, `--type script` requires `--command`. Missing it prints an error and exits.
  test: cli::add_script_missing_command_exits_1
  test: add::tests::build_from_flags_script_requires_command

SPEC-MN-AD-021: llm-requires-prompt-and-runtime
  In non-interactive mode, `--type llm` requires `--prompt` and `--runtime`. Missing either prints an error and exits.
  test: cli::add_llm_missing_prompt_exits_1
  test: cli::add_llm_missing_runtime_exits_1
  test: add::tests::build_from_flags_llm_requires_prompt
  test: add::tests::build_from_flags_llm_requires_runtime

SPEC-MN-AD-022: human-requires-prompt
  In non-interactive mode, `--type human` requires `--prompt`. Missing it prints an error and exits.
  test: cli::add_human_missing_prompt_exits_1
  test: add::tests::build_from_flags_human_requires_prompt

SPEC-MN-AD-023: unknown-type-exits-error
  If `--type` is not "script", "llm", or "human", prints an error listing valid types and exits.
  test: cli::add_unknown_type_exits_1
  test: add::tests::build_from_flags_rejects_unknown_type

---

## Gate assignment

SPEC-MN-AD-030: gate-flag-adds-ref-to-existing
  When `--gate <name>` is provided and the gate exists, a `{ ref = "<name>", blocking = <bool> }` entry is appended to the gate's validators list.
  test: cli::add_with_existing_gate
  test: cli::add_with_gate_blocking_false
  test: add::tests::apply_edits_adds_to_existing_gate
  test: add::tests::apply_edits_gate_ref_blocking_false

SPEC-MN-AD-031: gate-flag-creates-new-gate
  When `--gate <name>` is provided and the gate does not exist, a new gate section is created with the validator as its only entry.
  test: cli::add_with_new_gate
  test: add::tests::apply_edits_creates_new_gate
  test: add::tests::apply_edits_creates_new_gate_no_description

SPEC-MN-AD-032: no-gate-creates-top-level-only
  When no `--gate` is provided in non-interactive mode, the validator is added as a top-level `[validators.<name>]` entry only, with no gate reference.
  test: cli::add_without_gate_top_level_only
  test: add::tests::apply_edits_adds_script_validator_no_gate

---

## Import mode

SPEC-MN-AD-040: import-from-file
  `--from <path>` reads a local TOML file and imports all validators defined in it.
  test: cli::add_from_file
  test: cli::add_from_file_multiple_validators
  test: add::tests::resolve_import_source_reads_local_file

SPEC-MN-AD-041: import-from-url
  `--from <url>` (starting with http:// or https://) fetches the TOML content via HTTP GET and imports all validators.
  test: UNTESTED (requires network; tested via code path inspection — URL branch uses reqwest::blocking::Client)

SPEC-MN-AD-042: import-registry-not-yet-supported
  `--from registry:*` prints "Registry imports are not yet supported. Use a URL or file path." and exits with error status.
  test: cli::add_from_registry_exits_1
  test: add::tests::resolve_import_source_rejects_registry

SPEC-MN-AD-043: import-name-collision-rejected
  If any imported validator name collides with an existing name in the config, the entire import is rejected with an error naming the collision. No partial writes.
  test: cli::add_from_file_collision_exits_1
  test: add::tests::apply_edits_rejects_collision_in_batch

SPEC-MN-AD-044: import-single-format
  Import files with a `[validator]` top-level table are treated as single-validator imports. The `name` field inside is required and becomes the validator key.
  test: cli::add_from_file_single_format
  test: add::tests::parse_import_single_format
  test: add::tests::parse_import_single_format_missing_name
  test: add::tests::parse_import_single_format_missing_type

SPEC-MN-AD-045: import-multi-format
  Import files with `[validators.*]` tables are treated as multi-validator imports. Each subtable key becomes the validator name. This format is identical to the `[validators]` section in baton.toml.
  test: cli::add_from_file_multiple_validators
  test: add::tests::parse_import_multi_format
  test: add::tests::parse_import_multi_format_all_fields
  test: add::tests::parse_import_llm_fields

---

## TOML editing safety

SPEC-MN-AD-050: preserves-existing-content
  Adding a validator preserves all existing comments, formatting, and key ordering in baton.toml.
  test: cli::add_preserves_existing_config_structure
  test: add::tests::apply_edits_preserves_comments

SPEC-MN-AD-051: validates-before-writing
  After modifying the TOML document, the result is parsed and validated via `parse_config` and `validate_config`. If validation fails, no changes are written and the error is reported.
  test: cli::add_result_passes_validate_config
  test: add::tests::apply_edits_validates_result

SPEC-MN-AD-052: dry-run-previews-without-writing
  `--dry-run` prints the TOML that would be added and exits successfully. The file is not modified.
  test: cli::add_dry_run_no_changes
  test: cli::add_dry_run_with_gate_shows_preview

SPEC-MN-AD-053: yes-flag-skips-confirmation
  `--yes` / `-y` skips the confirmation prompt. Combined with `--type`/`--name`, enables fully non-interactive usage.
  test: cli::add_noninteractive_script_success (uses -y throughout)

---

## Exit codes

SPEC-MN-AD-060: success-exits-0
  On successful addition, prints a confirmation message to stderr and exits 0.
  test: cli::add_noninteractive_script_success
  test: cli::add_success_message_on_stderr

SPEC-MN-AD-061: user-cancel-exits-0
  If the user declines at the confirmation prompt, prints "Cancelled." and exits 0.
  test: UNTESTED (requires interactive TTY; tested via code inspection — Confirm::new() → false → "Cancelled." → return 0)

SPEC-MN-AD-062: validation-errors-exit-1
  Missing required fields, name collisions, parse errors, and unknown types exit 1.
  test: cli::add_script_missing_command_exits_1
  test: cli::add_llm_missing_prompt_exits_1
  test: cli::add_llm_missing_runtime_exits_1
  test: cli::add_human_missing_prompt_exits_1
  test: cli::add_unknown_type_exits_1
  test: cli::add_duplicate_name_exits_1
  test: cli::add_from_file_collision_exits_1

SPEC-MN-AD-063: infrastructure-errors-exit-2
  Config not found, file I/O errors, and network failures exit 2.
  test: cli::add_missing_config_exits_2
