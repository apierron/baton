+++
description = "Check Status enum match exhaustiveness and semantic correctness"
expects = "verdict"
+++

You are reviewing enum match completeness for a status type system.

The codebase has two status enums:
- `Status`: Pass, Fail, Warn, Skip, Error (5 variants)
- `VerdictStatus`: Pass, Fail, Error (3 variants)

For every match expression on these enums in the code below, check:

1. Is the match exhaustive? If it uses `_ =>`, could a specific variant
   be mishandled by the catch-all?
2. Does the catch-all behavior make semantic sense for ALL variants it catches?
   For example, if `_ => Status::Fail` catches `Warn`, is that correct?
3. Are Skip and Warn handled differently from Pass/Fail/Error where they should be?

Do NOT flag matches where the catch-all is clearly correct for all variants.

Respond with exactly one of:
- PASS — if all matches are semantically correct
- FAIL {match location and issue} — cite the specific problematic match
- WARN {concern} — if a catch-all is suspicious but defensible

## Code

{file.content}
