# Spec Files

The `spec/` directory contains one file per source module. Each file is a detailed decision tree that documents every decision point, error return, and invariant in the module as a machine-readable assertion. These assertions link directly to tests.

The spec files are the authoritative behavior reference for baton. When the implementation disagrees with the spec, the implementation is wrong.

## Purpose

Spec files serve three roles:

1. **Behavior contracts** — each assertion documents exactly what the code should do in a specific situation, including edge cases. Reading the spec for a module tells you what it does without reading the source.
2. **Test coverage tracking** — every assertion maps to a test or is marked `UNTESTED`. This makes coverage gaps visible and actionable.
3. **Development driver** — new features start as spec assertions before any code is written. This keeps the spec, tests, and implementation in sync.

## File Layout

Every spec file follows the same structure:

```markdown
# module: <module_name>

<One-paragraph summary of the module's responsibility.>

<Optional: a longer paragraph explaining the module's design position
in the codebase and why it exists.>

## Public functions

| Function | Purpose |
|---|---|
| `function_name` | Brief description |

## Internal functions

| Function | Called by |
|---|---|
| `helper_fn` | `public_fn` |

## Design notes

<Prose paragraphs explaining non-obvious design decisions.
Why was this approach chosen over the alternatives?
What invariants does this module maintain?>

---

## <function_or_section_name>

<Prose describing what this function does, its inputs, outputs,
and the high-level decision flow.>

### Sections

1. Section name
2. Section name

### <function_name>: <section_name>

<Prose explaining the decision tree for this section.
Edge cases, ordering constraints, rationale for specific behaviors.>

SPEC-XX-YY-NNN: short-kebab-description
  <Plain English description of the exact behavior being asserted.
  Specific enough that someone could write a test from this alone.>
  test: module::tests::test_function_name

SPEC-XX-YY-NNN: another-assertion
  <Description.>
  test: UNTESTED
```

The file starts broad (module summary, public API, design rationale) and gets progressively specific (per-function decision trees, individual assertions). Each section under the `---` divider covers one public function or logical group.

## Assertion Format

Every assertion has three parts:

```text
SPEC-XX-YY-NNN: short-kebab-description
  <behavior description>
  test: <test reference>
```

**ID format:** `SPEC-{module}-{section}-{number}`

The module prefix is a two-letter code identifying the source module. The section prefix identifies the function or logical group within the module. The number is a three-digit sequence within that section.

| Module | Prefix |
|---|---|
| types | `TY` |
| config | `CF` |
| prompt | `PR` |
| placeholder | `PH` |
| verdict_parser | `VP` |
| exec | `EX` |
| history | `HI` |
| runtime | `RT` |
| provider | `PV` |
| main | `MN` |

Section prefixes are specific to each module. For example, in `config.md`: `PC` for parse_config, `VC` for validate_config, `SC` for source parsing, `VP` for validator parsing, `GR` for gate reference parsing, `SR` for split_run_if, `DC` for discover_config. In `exec.md`: `FC` for file collector, `DP` for dispatch planner, `PL` for execution pipeline.

**Test reference** is one of:

- `test: module::tests::test_name` — directly tested by this test function
- `test: UNTESTED` — no test exists yet (a coverage gap)
- `test: UNTESTED (reason)` — no test exists, with an explanation of why (e.g., "requires HTTP mock server")
- `test: IMPLICIT via module::tests::test_name` — behavior is exercised by a test but not its primary focus

An assertion can reference multiple tests when the behavior is covered from different angles:

```text
SPEC-CF-PC-040: validator-inherits-blocking-from-defaults
  When blocking is not set on a validator, it inherits from defaults.blocking.
  When explicitly set, the validator's value takes precedence.
  test: config::tests::defaults_applied
  test: config::tests::validator_overrides_defaults
```

## Design Notes Section

The design notes section at the top of each spec file explains *why*, not *what*. It captures decisions that would otherwise live only in someone's head:

- Why does `execute_validator` take `Option<&BatonConfig>` instead of `&BatonConfig`?
- Why does `resolve_placeholders` emit warnings instead of errors?
- Why is `parse_config` early-return while `validate_config` accumulates?

These notes are prose, not assertions. They don't have IDs or test references. They exist to prevent future contributors from "fixing" things that are intentional.

## Prose Between Assertions

The space between the `---` divider and the first assertion in each section is for decision-tree prose. This is where you describe the logic flow, ordering constraints, and edge cases that the assertions individually verify:

```markdown
## Dispatch planner

The dispatch planner matches the input file pool against each validator's
input declaration to produce Invocations. Each invocation is a concrete
unit of work: one validator + one set of input files.

The planner handles four input forms:
- No input: single invocation with no files
- Per-file: one invocation per matching file
- Batch: single invocation with all matching files
- Named: grouped by key expression, one invocation per distinct key

Edge cases to consider:
- No files match a validator's glob — validator is skipped with a warning
- Incomplete key group (key appears in one input slot but not another) — skipped
- Fixed inputs (path) are injected into every invocation regardless of grouping

SPEC-EX-DP-001: no-input-produces-single-invocation
  ...
```

This prose is what makes spec files decision trees rather than flat assertion lists. A reader can follow the logic top-down and understand how the assertions relate to each other.

## Spec-Driven Development Workflow

New features and bug fixes follow spec → tests → implementation order:

1. **Edit the spec first.** Add or update assertions in the relevant `spec/*.md` file. New assertions are marked `UNTESTED`. If a new function is involved, add the function tables and design notes too.
2. **Write tests.** Implement tests that exercise the new assertions. Update the assertion's `test:` line to reference the test.
3. **Write implementation.** Make the tests pass.

For bug fixes, the same order applies: write the assertion that describes the correct behavior, write a test that fails, then fix the code.

This ordering ensures the spec stays ahead of the code. If you write the implementation first, the spec tends to become documentation of what was built rather than a contract for what should be built.

## Useful Commands

```bash
# List all spec files
ls spec/*.md

# Find all untested assertions
grep -r "UNTESTED" spec/

# Count untested assertions per module
grep -c "UNTESTED" spec/*.md

# Count total assertions per module
grep -c "^SPEC-" spec/*.md

# Find all assertions for a specific function
grep -A2 "^SPEC-EX-RG" spec/exec.md
```

## Adding a New Module

When adding a new source module, create a corresponding `spec/<module>.md`:

1. Start with the `# module:` header and summary paragraph.
2. Add the public/internal function tables.
3. Write design notes explaining non-obvious decisions.
4. Add `---` dividers and per-function sections with assertions.
5. Mark all assertions as `UNTESTED` initially.
6. Add the module prefix to the table in this document.
