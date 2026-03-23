+++
description = "Check that all error variants are constructed and handled"
expects = "verdict"
+++

You are reviewing error handling completeness.
Given the error type definitions and the full codebase,
check that:

1. Every BatonError variant is constructed somewhere (no dead variants)
2. Every BatonError variant is matched or handled somewhere (no unhandled errors)
3. Error messages include context values (the offending input, not just the error class)
4. No error paths silently discard errors (e.g., `let _ = result;` where the error matters)

Respond with exactly one of:
- PASS — if error handling is complete
- FAIL {variant} — cite the unhandled or dead variant
- WARN {concern} — if coverage is technically complete but suspicious

## Error Definitions

{input.errors.content}

## Codebase Files

{input.code.content}
