//! Verdict parsing from LLM/agent text output.
//!
//! Uses a two-pass approach: first checks the opening line for PASS/FAIL/WARN
//! keywords, then scans the full text for the last occurrence. Respects word
//! boundaries (e.g., "PASSWORD" does not match "PASS").

use crate::types::Status;

/// Parsed verdict from LLM/agent text output.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedVerdict {
    pub status: Status,
    pub evidence: Option<String>,
}

/// Returns true if `text` starts with `keyword` and the next character
/// (if any) is not alphanumeric.
fn starts_with_keyword(text: &str, keyword: &str) -> bool {
    if !text.starts_with(keyword) {
        return false;
    }
    if text.len() == keyword.len() {
        return true;
    }
    let next_char = text[keyword.len()..].chars().next().unwrap();
    !next_char.is_alphanumeric()
}

/// Returns the position of the last occurrence of `keyword` in `text`
/// where it appears as a standalone word. Returns None if not found.
fn rfind_keyword(text: &str, keyword: &str) -> Option<usize> {
    let text_upper = text.to_uppercase();
    let keyword_upper = keyword.to_uppercase();
    let mut search_end = text_upper.len();

    loop {
        let pos = text_upper[..search_end].rfind(&keyword_upper);
        let pos = pos?;

        // Check preceding character
        if pos > 0 {
            let prev = text_upper[..pos].chars().next_back().unwrap();
            if prev.is_alphanumeric() {
                search_end = pos;
                continue;
            }
        }

        // Check following character
        let end_pos = pos + keyword_upper.len();
        if end_pos < text_upper.len() {
            let next = text_upper[end_pos..].chars().next().unwrap();
            if next.is_alphanumeric() {
                search_end = pos;
                continue;
            }
        }

        return Some(pos);
    }
}

/// Parse a verdict from LLM/agent text output.
///
/// # Examples
///
/// ```
/// use baton::verdict_parser::parse_verdict;
/// use baton::types::Status;
///
/// let verdict = parse_verdict("PASS — code looks good");
/// assert_eq!(verdict.status, Status::Pass);
///
/// let verdict = parse_verdict("FAIL — missing error handling");
/// assert_eq!(verdict.status, Status::Fail);
/// ```
pub fn parse_verdict(text: &str) -> ParsedVerdict {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ParsedVerdict {
            status: Status::Error,
            evidence: Some("[baton] Validator produced empty output".into()),
        };
    }

    let lines = trimmed.lines();

    // Pass 1: Check first non-empty line
    let first_line = match lines.into_iter().find(|l| !l.trim().is_empty()) {
        Some(l) => l.trim().to_string(),
        None => {
            return ParsedVerdict {
                status: Status::Error,
                evidence: Some("[baton] Validator produced empty output".into()),
            };
        }
    };

    let first_upper = first_line.to_uppercase();

    if starts_with_keyword(&first_upper, "PASS") {
        return ParsedVerdict {
            status: Status::Pass,
            evidence: None,
        };
    }

    if starts_with_keyword(&first_upper, "WARN") {
        let mut evidence = first_line[4..].trim().to_string();
        if evidence.is_empty() {
            // Get rest of text after first line
            let rest = trimmed[trimmed.find(&first_line).unwrap() + first_line.len()..]
                .trim()
                .to_string();
            evidence = rest;
        }
        return ParsedVerdict {
            status: Status::Warn,
            evidence: if evidence.is_empty() {
                None
            } else {
                Some(evidence)
            },
        };
    }

    if starts_with_keyword(&first_upper, "FAIL") {
        let mut evidence = first_line[4..].trim().to_string();
        if evidence.is_empty() {
            let rest = trimmed[trimmed.find(&first_line).unwrap() + first_line.len()..]
                .trim()
                .to_string();
            evidence = rest;
        }
        return ParsedVerdict {
            status: Status::Fail,
            evidence: if evidence.is_empty() {
                None
            } else {
                Some(evidence)
            },
        };
    }

    // Pass 2: Scan full text for last verdict keyword
    let last_pass = rfind_keyword(trimmed, "PASS");
    let last_fail = rfind_keyword(trimmed, "FAIL");
    let last_warn = rfind_keyword(trimmed, "WARN");

    let mut candidates: Vec<(&str, usize)> = Vec::new();
    if let Some(p) = last_pass {
        candidates.push(("pass", p));
    }
    if let Some(p) = last_fail {
        candidates.push(("fail", p));
    }
    if let Some(p) = last_warn {
        candidates.push(("warn", p));
    }

    if candidates.is_empty() {
        let truncated: String = trimmed.chars().take(500).collect();
        return ParsedVerdict {
            status: Status::Error,
            evidence: Some(format!(
                "[baton] Could not parse verdict from validator output:\n{truncated}"
            )),
        };
    }

    // Sort by position descending — take the last one
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    let (status_str, pos) = candidates[0];

    let keyword_len = 4; // PASS, FAIL, WARN
    let evidence = trimmed[pos + keyword_len..].trim().to_string();

    match status_str {
        "pass" => ParsedVerdict {
            status: Status::Pass,
            evidence: None,
        },
        "fail" => ParsedVerdict {
            status: Status::Fail,
            evidence: if evidence.is_empty() {
                let truncated: String = trimmed.chars().take(500).collect();
                Some(truncated)
            } else {
                Some(evidence)
            },
        },
        "warn" => ParsedVerdict {
            status: Status::Warn,
            evidence: if evidence.is_empty() {
                let truncated: String = trimmed.chars().take(500).collect();
                Some(truncated)
            } else {
                Some(evidence)
            },
        },
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ═══════════════════════════════════════════════════════════════
    // Internal implementation tests
    // ═══════════════════════════════════════════════════════════════

    // ─── starts_with_keyword ────────────────────────

    #[test]
    fn starts_with_keyword_exact() {
        assert!(starts_with_keyword("PASS", "PASS"));
    }

    #[test]
    fn starts_with_keyword_followed_by_space() {
        assert!(starts_with_keyword("PASS all good", "PASS"));
    }

    #[test]
    fn starts_with_keyword_followed_by_punct() {
        assert!(starts_with_keyword("PASS.", "PASS"));
        assert!(starts_with_keyword("FAIL:", "FAIL"));
        assert!(starts_with_keyword("WARN — issue", "WARN"));
    }

    #[test]
    fn starts_with_keyword_followed_by_alpha() {
        assert!(!starts_with_keyword("PASSWORD", "PASS"));
        assert!(!starts_with_keyword("FAILING", "FAIL"));
        assert!(!starts_with_keyword("WARNING", "WARN"));
    }

    // ─── rfind_keyword ──────────────────────────────

    #[test]
    fn rfind_keyword_basic() {
        assert_eq!(rfind_keyword("PASS", "PASS"), Some(0));
        assert_eq!(rfind_keyword("result: PASS", "PASS"), Some(8));
    }

    #[test]
    fn rfind_keyword_word_boundary() {
        assert_eq!(rfind_keyword("PASSWORD", "PASS"), None);
        assert_eq!(rfind_keyword("BYPASS", "PASS"), None);
    }

    #[test]
    fn rfind_keyword_multiple_occurrences() {
        let text = "first PASS then another PASS";
        assert_eq!(rfind_keyword(text, "PASS"), Some(24));
    }

    #[test]
    fn rfind_keyword_not_found() {
        assert_eq!(rfind_keyword("no verdict here", "PASS"), None);
    }

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn first_line_pass() {
        let v = parse_verdict("PASS");
        assert_eq!(v.status, Status::Pass);
        assert_eq!(v.evidence, None);
    }

    #[test]
    fn first_line_pass_with_comment() {
        let v = parse_verdict("PASS — all good");
        assert_eq!(v.status, Status::Pass);
        assert_eq!(v.evidence, None);
    }

    #[test]
    fn password_does_not_match() {
        let v = parse_verdict("PASSWORD is secure");
        // Should NOT match PASS due to word boundary
        assert_eq!(v.status, Status::Error);
    }

    #[test]
    fn passing_does_not_match() {
        let v = parse_verdict("PASSING all tests");
        assert_eq!(v.status, Status::Error);
    }

    #[test]
    fn first_line_fail_with_reason() {
        let v = parse_verdict("FAIL some reason");
        assert_eq!(v.status, Status::Fail);
        assert_eq!(v.evidence, Some("some reason".into()));
    }

    #[test]
    fn first_line_warn_with_issue() {
        let v = parse_verdict("WARN minor issue");
        assert_eq!(v.status, Status::Warn);
        assert_eq!(v.evidence, Some("minor issue".into()));
    }

    #[test]
    fn reasoning_then_pass() {
        let v = parse_verdict("The code looks good.\nI reviewed everything.\nPASS");
        assert_eq!(v.status, Status::Pass);
    }

    #[test]
    fn both_fail_and_pass_last_wins() {
        let v = parse_verdict("This would FAIL if not for X, but it does PASS");
        assert_eq!(v.status, Status::Pass);
    }

    #[test]
    fn password_then_pass() {
        // "PASSWORD" should be skipped by word boundary, standalone "PASS" matches
        let v = parse_verdict("The PASSWORD check is secure.\nPASS");
        assert_eq!(v.status, Status::Pass);
    }

    #[test]
    fn empty_text() {
        let v = parse_verdict("");
        assert_eq!(v.status, Status::Error);
        assert!(v.evidence.unwrap().contains("empty"));
    }

    #[test]
    fn no_verdict_keyword() {
        let v = parse_verdict("I think the code is fine but I'm not sure.");
        assert_eq!(v.status, Status::Error);
        assert!(v.evidence.unwrap().contains("Could not parse"));
    }

    #[test]
    fn very_long_output_truncated() {
        let long = "x".repeat(1000);
        let v = parse_verdict(&long);
        assert_eq!(v.status, Status::Error);
        let evidence = v.evidence.unwrap();
        assert!(evidence.len() <= 600); // 500 chars + prefix
    }

    #[test]
    fn fail_multiline_evidence() {
        let v = parse_verdict("FAIL\nThe implementation is missing pagination.\nSee section 3.2.");
        assert_eq!(v.status, Status::Fail);
        let ev = v.evidence.unwrap();
        assert!(ev.contains("pagination"));
    }

    #[test]
    fn warn_multiline_evidence() {
        let v = parse_verdict("WARN\nSuspicious pattern in error handling.");
        assert_eq!(v.status, Status::Warn);
        let ev = v.evidence.unwrap();
        assert!(ev.contains("Suspicious"));
    }

    #[test]
    fn failing_does_not_match_fail() {
        let v = parse_verdict("FAILING to meet requirements is not good");
        // "FAILING" should not match "FAIL" due to word boundary
        assert_eq!(v.status, Status::Error);
    }

    #[test]
    fn warning_does_not_match_warn() {
        let v = parse_verdict("WARNING: something happened");
        // "WARNING" should not match "WARN" due to word boundary
        assert_eq!(v.status, Status::Error);
    }

    #[test]
    fn pass_with_leading_whitespace() {
        let v = parse_verdict("  \n  PASS\n");
        assert_eq!(v.status, Status::Pass);
    }

    #[test]
    fn fail_at_end_of_reasoning() {
        let v = parse_verdict(
            "I analyzed the code carefully.\n\
             The function handles edge cases.\n\
             However, section 3.2 is not implemented.\n\
             FAIL — section 3.2 not implemented",
        );
        assert_eq!(v.status, Status::Fail);
        assert!(v.evidence.unwrap().contains("3.2"));
    }

    // ─── Spec coverage (UNTESTED) ──────────────────────

    #[test]
    fn whitespace_only_input() {
        let v = parse_verdict("   \n  \n  ");
        assert_eq!(v.status, Status::Error);
        assert_eq!(
            v.evidence,
            Some("[baton] Validator produced empty output".into())
        );
    }

    #[test]
    fn lowercase_pass_on_first_line() {
        let v = parse_verdict("pass\nLooks good");
        assert_eq!(v.status, Status::Pass);
        assert_eq!(v.evidence, None);
    }

    #[test]
    fn fail_with_no_remaining_lines() {
        let v = parse_verdict("FAIL");
        assert_eq!(v.status, Status::Fail);
        assert_eq!(v.evidence, None);
    }

    #[test]
    fn warn_with_no_remaining_lines() {
        let v = parse_verdict("WARN");
        assert_eq!(v.status, Status::Warn);
        assert_eq!(v.evidence, None);
    }

    #[test]
    fn warn_pass2_winner_with_evidence() {
        let v = parse_verdict("The code has issues\nWARN: minor style problem");
        assert_eq!(v.status, Status::Warn);
        assert_eq!(v.evidence, Some(": minor style problem".into()));
    }

    #[test]
    fn fail_at_end_no_text_after() {
        let v = parse_verdict("The review shows FAIL");
        assert_eq!(v.status, Status::Fail);
        let ev = v.evidence.unwrap();
        assert_eq!(ev, "The review shows FAIL");
    }

    #[test]
    fn warn_at_end_no_text_after() {
        let v = parse_verdict("Minor issues found WARN");
        assert_eq!(v.status, Status::Warn);
        let ev = v.evidence.unwrap();
        assert_eq!(ev, "Minor issues found WARN");
    }

    #[test]
    fn keyword_at_end_with_preceding_boundary() {
        let v = parse_verdict("result: PASS");
        assert_eq!(v.status, Status::Pass);
        assert_eq!(v.evidence, None);
    }

    #[test]
    fn text_not_starting_with_keyword_falls_to_pass2() {
        let v = parse_verdict("Hello PASS");
        assert_eq!(v.status, Status::Pass);
        assert_eq!(v.evidence, None);
    }
}
