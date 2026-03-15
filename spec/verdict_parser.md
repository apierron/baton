# module: verdict_parser

Verdict parsing from LLM/agent text output. Uses a two-pass approach: first checks the opening line for PASS/FAIL/WARN keywords, then scans the full text for the last occurrence. Respects word boundaries (e.g., "PASSWORD" does not match "PASS").

This module is intentionally simple and stateless. It exists because LLM output is unpredictable in structure -- the verdict keyword might appear on the first line, the last line, or buried in reasoning. The two-pass design handles the common case (keyword on the first line) cheaply and falls back to a full scan when the LLM buries its verdict in prose.

## Public functions

| Function        | Purpose                                            |
|-----------------|----------------------------------------------------|
| `parse_verdict` | Parse verdict from text -> ParsedVerdict (status + evidence) |

## Internal functions

| Function             | Called by                          |
|----------------------|------------------------------------|
| `starts_with_keyword`| `parse_verdict` (pass 1)           |
| `rfind_keyword`      | `parse_verdict` (pass 2)           |

## Types

`ParsedVerdict` is a struct with two fields: `status: Status` and `evidence: Option<String>`. Evidence carries the textual reason for a FAIL or WARN, or diagnostic information for an Error. PASS never carries evidence -- this is a deliberate choice: if the validator passed, there is nothing for the user to act on, and surfacing LLM commentary after "PASS" would add noise to the output.

---

## starts_with_keyword

Checks whether a string begins with a keyword at a word boundary. This is the core word-boundary primitive used in pass 1.

The function operates on the already-uppercased first line, so it does not perform case folding itself. This is an internal detail but matters for understanding the test setup.

SPEC-VP-SW-001: exact-keyword-match
  When the text equals the keyword exactly (no trailing characters), returns true.
  test: verdict_parser::tests::starts_with_keyword_exact

SPEC-VP-SW-002: keyword-followed-by-non-alphanumeric
  When the text starts with the keyword and the next character is not alphanumeric (e.g., space, punctuation, dash), returns true. This allows "PASS.", "FAIL:", "WARN ---" to match.
  test: verdict_parser::tests::starts_with_keyword_followed_by_space
  test: verdict_parser::tests::starts_with_keyword_followed_by_punct

SPEC-VP-SW-003: keyword-followed-by-alphanumeric-rejected
  When the text starts with the keyword but the next character is alphanumeric, returns false. This prevents "PASSWORD" matching "PASS", "FAILING" matching "FAIL", and "WARNING" matching "WARN".
  test: verdict_parser::tests::starts_with_keyword_followed_by_alpha

SPEC-VP-SW-004: keyword-not-at-start-rejected
  When the text does not start with the keyword, returns false regardless of content.
  test: UNTESTED

---

## rfind_keyword

Finds the last standalone occurrence of a keyword in text, enforcing word boundaries on both sides. Case-insensitive via uppercasing.

The "last occurrence wins" semantic is deliberate. LLMs often reason about failure scenarios before concluding with a pass verdict. Scanning from the end ensures the final verdict takes precedence over intermediate mentions.

SPEC-VP-RF-001: case-insensitive-match
  The search is case-insensitive: both the text and keyword are uppercased before comparison. "pass", "Pass", and "PASS" all match.
  test: UNTESTED (no test uses lowercase keywords in rfind_keyword directly, though parse_verdict tests cover this indirectly)

SPEC-VP-RF-002: returns-last-position
  When the keyword appears multiple times as a standalone word, returns the position of the last (rightmost) occurrence.
  test: verdict_parser::tests::rfind_keyword_multiple_occurrences

SPEC-VP-RF-003: preceding-alphanumeric-rejects
  When the character immediately before the keyword is alphanumeric, that occurrence is not a word boundary match. "BYPASS" does not match "PASS" because 'Y' precedes it.
  test: verdict_parser::tests::rfind_keyword_word_boundary

SPEC-VP-RF-004: following-alphanumeric-rejects
  When the character immediately after the keyword is alphanumeric, that occurrence is not a word boundary match. "PASSWORD" does not match "PASS" because 'W' follows it.
  test: verdict_parser::tests::rfind_keyword_word_boundary

SPEC-VP-RF-005: standalone-keyword-found
  When the keyword appears as a standalone word (preceded and followed by non-alphanumeric characters or string boundaries), returns Some(position).
  test: verdict_parser::tests::rfind_keyword_basic

SPEC-VP-RF-006: keyword-not-present
  When the keyword does not appear in the text at all, returns None.
  test: verdict_parser::tests::rfind_keyword_not_found

SPEC-VP-RF-007: keyword-at-start-of-text
  When the keyword appears at position 0, the preceding-character check is skipped (there is no preceding character). Only the following-character boundary is checked.
  test: verdict_parser::tests::rfind_keyword_basic (first assertion: rfind_keyword("PASS", "PASS") == Some(0))

SPEC-VP-RF-008: keyword-at-end-of-text
  When the keyword appears at the end of the text, the following-character check is skipped (there is no following character). Only the preceding-character boundary is checked.
  test: UNTESTED (no explicit test for keyword at end with preceding boundary check)

---

## parse_verdict

Parse a verdict from LLM/agent text output. This is the only public function in the module. It implements a two-pass strategy:

- **Pass 1**: Check whether the first non-empty line starts with a verdict keyword. This handles the common case where the LLM follows instructions and leads with the verdict.
- **Pass 2**: If pass 1 does not match, scan the full text for the last standalone occurrence of any verdict keyword. This handles the case where the LLM buries the verdict in reasoning.

The two-pass design means that a first-line keyword always takes priority over keywords elsewhere in the text. This is intentional: if the LLM puts "FAIL" on the first line and "PASS" on the last line, the first-line "FAIL" wins. But if the first line is just prose, then the last keyword in the text wins.

### parse_verdict: empty input

SPEC-VP-PV-001: empty-text-returns-error
  When the input text is empty or contains only whitespace, returns Status::Error with evidence "[baton] Validator produced empty output".
  test: verdict_parser::tests::empty_text

SPEC-VP-PV-002: whitespace-only-returns-error
  When the input text contains only whitespace and newlines (no non-empty lines), returns the same Status::Error as empty text. The text is trimmed before processing.
  test: UNTESTED (empty_text covers "" but not "   \n  \n  ")

### parse_verdict: pass 1 -- first line

The first non-empty line is extracted, trimmed, and uppercased. It is checked against PASS, WARN, and FAIL in that order using starts_with_keyword. The check order matters: if a line starts with "PASS", the WARN and FAIL checks are never reached.

SPEC-VP-PV-010: first-line-pass
  When the first non-empty line starts with "PASS" at a word boundary, returns Status::Pass with evidence=None. Any text after "PASS" on the first line is discarded.
  test: verdict_parser::tests::first_line_pass
  test: verdict_parser::tests::first_line_pass_with_comment

SPEC-VP-PV-011: first-line-pass-discards-feedback
  When the first line starts with "PASS" followed by additional text (e.g., "PASS -- all good"), the additional text is not included as evidence. This is deliberate: pass results carry no evidence because there is nothing for the user to act on.
  test: verdict_parser::tests::first_line_pass_with_comment

SPEC-VP-PV-012: first-line-pass-is-case-insensitive
  The first line is uppercased before keyword matching, so "pass", "Pass", and "PASS" all match.
  test: UNTESTED (no test uses lowercase "pass" on the first line)

SPEC-VP-PV-013: first-line-fail-with-inline-evidence
  When the first non-empty line starts with "FAIL" at a word boundary followed by text, returns Status::Fail with evidence set to the trimmed text after "FAIL".
  test: verdict_parser::tests::first_line_fail_with_reason

SPEC-VP-PV-014: first-line-fail-with-multiline-evidence
  When the first line is exactly "FAIL" (no text after keyword), evidence is taken from the remaining lines of the input. The remaining text is trimmed.
  test: verdict_parser::tests::fail_multiline_evidence

SPEC-VP-PV-015: first-line-fail-no-evidence
  When the first line is "FAIL" and there are no remaining lines (or remaining lines are empty), evidence is None.
  test: UNTESTED

SPEC-VP-PV-016: first-line-warn-with-inline-evidence
  When the first non-empty line starts with "WARN" at a word boundary followed by text, returns Status::Warn with evidence set to the trimmed text after "WARN".
  test: verdict_parser::tests::first_line_warn_with_issue

SPEC-VP-PV-017: first-line-warn-with-multiline-evidence
  When the first line is exactly "WARN" (no text after keyword), evidence is taken from the remaining lines of the input.
  test: verdict_parser::tests::warn_multiline_evidence

SPEC-VP-PV-018: first-line-warn-no-evidence
  When the first line is "WARN" and there are no remaining lines, evidence is None.
  test: UNTESTED

SPEC-VP-PV-019: leading-whitespace-trimmed
  Leading whitespace and blank lines before the first non-empty line are stripped. The first non-empty line is used for pass 1 matching.
  test: verdict_parser::tests::pass_with_leading_whitespace

SPEC-VP-PV-020: word-boundary-prevents-false-match-pass1
  Keywords embedded in larger words on the first line do not match in pass 1. "PASSWORD", "FAILING", "WARNING" all fail the starts_with_keyword check and fall through to pass 2.
  test: verdict_parser::tests::password_does_not_match
  test: verdict_parser::tests::passing_does_not_match
  test: verdict_parser::tests::failing_does_not_match_fail
  test: verdict_parser::tests::warning_does_not_match_warn

### parse_verdict: pass 2 -- full text scan

If pass 1 did not match (the first line does not start with a verdict keyword), the full trimmed text is scanned for the last standalone occurrence of each keyword using rfind_keyword. All three keywords (PASS, FAIL, WARN) are searched, and the one with the highest position wins.

The "last keyword wins" rule means that reasoning text like "This would FAIL if not for X, but it does PASS" resolves to PASS. This matches the intuition that the final verdict is what matters, not intermediate reasoning about failure scenarios.

SPEC-VP-PV-030: pass2-last-keyword-wins
  When multiple verdict keywords appear in the text (not on the first line), the keyword with the highest character position determines the status. "This would FAIL ... but it does PASS" returns Status::Pass.
  test: verdict_parser::tests::both_fail_and_pass_last_wins

SPEC-VP-PV-031: pass2-pass-no-evidence
  When PASS is the winning keyword in pass 2, evidence is always None, consistent with the pass-never-carries-evidence rule.
  test: verdict_parser::tests::reasoning_then_pass

SPEC-VP-PV-032: pass2-fail-evidence-after-keyword
  When FAIL is the winning keyword in pass 2, evidence is the trimmed text after the keyword. If no text follows the keyword, evidence is the full input text (truncated to 500 characters).
  test: verdict_parser::tests::fail_at_end_of_reasoning

SPEC-VP-PV-033: pass2-warn-evidence-after-keyword
  When WARN is the winning keyword in pass 2, evidence is the trimmed text after the keyword. If no text follows the keyword, evidence is the full input text (truncated to 500 characters).
  test: UNTESTED (no test has WARN as a pass-2 winner with evidence)

SPEC-VP-PV-034: pass2-fail-empty-evidence-uses-full-text
  When FAIL wins in pass 2 and there is no text after the keyword (e.g., text ends with "FAIL"), evidence is set to the full input text truncated to 500 characters. This ensures the user always gets context about what went wrong.
  test: UNTESTED

SPEC-VP-PV-035: pass2-warn-empty-evidence-uses-full-text
  When WARN wins in pass 2 and there is no text after the keyword, evidence is set to the full input text truncated to 500 characters.
  test: UNTESTED

SPEC-VP-PV-036: pass2-word-boundaries-enforced
  In pass 2, word boundary rules apply via rfind_keyword. "PASSWORD" and "BYPASS" do not match "PASS". Only standalone keyword occurrences are considered.
  test: verdict_parser::tests::password_then_pass

### parse_verdict: no keyword found

SPEC-VP-PV-040: no-keyword-returns-error
  When neither pass 1 nor pass 2 finds a verdict keyword, returns Status::Error with evidence containing "[baton] Could not parse verdict from validator output:" followed by the input text truncated to 500 characters.
  test: verdict_parser::tests::no_verdict_keyword

SPEC-VP-PV-041: unparseable-output-truncated-to-500-chars
  When the input text has no verdict keyword, the evidence includes at most 500 characters of the original text. This prevents extremely long LLM outputs from bloating error messages and history records.
  test: verdict_parser::tests::very_long_output_truncated

### parse_verdict: evidence extraction rules (summary)

Evidence handling follows consistent rules but differs by status:

- PASS: evidence is always None, in both pass 1 and pass 2. Rationale: a passing verdict has nothing actionable for the user.
- FAIL (pass 1): evidence is the text after the keyword on the first line; if empty, the remaining lines; if still empty, None.
- FAIL (pass 2): evidence is the text after the keyword; if empty, the full text truncated to 500 chars.
- WARN: same rules as FAIL for both passes.
- Error: evidence always includes a "[baton]" prefixed diagnostic message.

SPEC-VP-PV-050: pass-never-has-evidence
  Across both passes, Status::Pass always has evidence=None, even when text follows the keyword.
  test: verdict_parser::tests::first_line_pass_with_comment
  test: verdict_parser::tests::reasoning_then_pass

SPEC-VP-PV-051: error-evidence-always-prefixed
  All Status::Error results carry evidence with a "[baton]" prefix, distinguishing parser-generated messages from validator output.
  test: verdict_parser::tests::empty_text
  test: verdict_parser::tests::no_verdict_keyword
