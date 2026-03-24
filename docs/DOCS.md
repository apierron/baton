# Documentation

Rules for keeping documentation accurate as the codebase evolves. When you change code, check whether any of the touchpoints below need a corresponding update.

## Hand-Maintained Documentation Touchpoints

These files contain hand-maintained lists or references that can go stale. The table maps each kind of change to the files you must check.

| When you... | Check these files |
| ----------- | ----------------- |
| Add/remove/rename a CLI subcommand | `src/main.rs` (Commands enum), `src/commands/mod.rs` (subcommand table in `//!` docs), `README.md` (CLI Reference section) |
| Add/remove/rename a library module | `src/lib.rs` (`pub mod` declarations), `AGENTS.md` (dependency layers diagram), `docs/ARCHITECTURE.md` (dependency table) |
| Add/remove a submodule in `exec/` or `runtime/` | The parent `mod.rs` (`pub mod` declaration — rustdoc auto-generates the listing from these) |
| Add a new runtime adapter | `src/runtime/mod.rs` (`pub mod` + `create_adapter()` match arm), `README.md` (if user-facing) |
| Add/remove/rename a validator type | `src/config.rs` (`ValidatorType` enum), `README.md` (Validator Types section) |
| Change CLI flags for `baton check` | `src/main.rs` (Check variant in Commands enum), `README.md` (Key flags table) |
| Change placeholder syntax | `src/placeholder.rs` (resolve logic), `README.md` (Available placeholders list) |
| Add a new module to the codebase | Create `spec/<module>.md` (see `docs/SPEC.md` for format), add to `AGENTS.md` (Spec Files list) |

## Rustdoc Best Practices

### What to document

- Every `pub mod` gets a `//!` doc comment. First line: what it does, imperative mood. Keep it to one paragraph — rustdoc auto-generates the "Modules", "Structs", etc. sections, so don't duplicate those.
- Every `pub fn`, `pub struct`, `pub enum`, `pub trait` gets a `///` doc comment.
- Describe *what* and *why*, not *how*. The code shows how.

### Examples

- Add `# Examples` sections to functions that are entry points or whose usage isn't obvious from the signature.
- Use `no_run` for examples that need runtime resources (files, network, database).
- Doc examples are compiled and tested by `cargo test --doc` — keep them working.

### What NOT to document

- Private items, struct fields, or obvious enum variants. Only document these when the meaning isn't clear from the name and type.
- Code you didn't change. Don't add drive-by doc comments to unrelated items.

### Linking

- Link to related items with intra-doc links: `` [`parse_config`] ``, `` [`RuntimeAdapter`] ``.
- Wrap code references in backticks when they appear in prose: `` `BatonError` ``, `` `run_gate()` ``.

## README Sync

`README.md` doubles as the crate-level rustdoc via `#![doc = include_str!("../README.md")]` in `lib.rs`. Changes to the README are visible in both GitHub and `cargo doc` output.

The following README sections are hand-maintained and must stay in sync with code:

| README section | Source of truth |
| -------------- | --------------- |
| CLI Reference (command list) | `Commands` enum in `src/main.rs` |
| Key flags for `baton check` | `Check` variant fields in `src/main.rs` |
| Validator Types | `ValidatorType` enum in `src/config.rs` |
| Available placeholders | `resolve_placeholders()` in `src/placeholder.rs` |
| Prompt template format | `parse_prompt()` in `src/prompt.rs` |

## Spec File Maintenance

Every library module has a corresponding spec file in `spec/`. When you add a new module, create a matching spec file. See `docs/SPEC.md` for the assertion ID format, file structure, and workflow.

Quick check: `grep -r "UNTESTED" spec/` shows assertions that need test coverage.
