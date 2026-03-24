//! Core data types for baton: input files, invocations, verdicts, and run options.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::error::Result;

// ─── InputFile ────────────────────────────────────────────

/// A file from the input pool, with lazy content loading and cached SHA-256 hashing.
///
/// Content and hash are computed on first access via `&mut self` methods and cached.
#[derive(Debug, Clone)]
pub struct InputFile {
    pub path: PathBuf,
    content: Option<String>,
    hash: Option<String>,
}

impl InputFile {
    /// Creates an InputFile from a path. Does not read the file.
    ///
    /// Content and hash are loaded lazily on first access via
    /// [`get_content`](InputFile::get_content) and [`get_hash`](InputFile::get_hash).
    ///
    /// # Examples
    ///
    /// ```
    /// use baton::types::InputFile;
    /// use std::path::PathBuf;
    ///
    /// let input = InputFile::new(PathBuf::from("src/main.rs"));
    /// assert_eq!(input.path, PathBuf::from("src/main.rs"));
    /// ```
    pub fn new(path: PathBuf) -> Self {
        InputFile {
            path,
            content: None,
            hash: None,
        }
    }

    /// Returns file content, lazily reading from disk on first access.
    pub fn get_content(&mut self) -> Result<&str> {
        if self.content.is_none() {
            self.content = Some(std::fs::read_to_string(&self.path)?);
        }
        Ok(self.content.as_ref().unwrap())
    }

    /// Returns SHA-256 hash of file content, computing and caching on first access.
    pub fn get_hash(&mut self) -> Result<&str> {
        if self.hash.is_none() {
            let content = self.get_content()?;
            let mut hasher = Sha256::new();
            hasher.update(content.as_bytes());
            self.hash = Some(hex::encode(hasher.finalize()));
        }
        Ok(self.hash.as_ref().unwrap())
    }
}

// ─── Invocation ──────────────────────────────────────────

/// A planned execution of a validator against a specific set of input files.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub validator_name: String,
    pub group_key: Option<String>,
    pub inputs: BTreeMap<String, Vec<InputFile>>,
}

// ─── GateResult ──────────────────────────────────────────

/// Result of running all validators in a single gate.
#[derive(Debug, Clone)]
pub struct GateResult {
    pub gate_name: String,
    pub status: Status,
    pub validator_results: Vec<ValidatorResult>,
    pub duration: std::time::Duration,
}

// ─── InvocationResult ────────────────────────────────────

/// Top-level result of a single baton invocation (running one or more gates).
#[derive(Debug, Clone)]
pub struct InvocationResult {
    pub id: String,
    pub gate_results: Vec<GateResult>,
    pub duration: std::time::Duration,
}

// ─── Cost ────────────────────────────────────────────────

/// Token usage and cost metadata from an LLM validator call.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cost {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub model: Option<String>,
    pub estimated_usd: Option<f64>,
}

// ─── ValidatorResult ─────────────────────────────────────

/// Validator-level outcome status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Warn,
    Skip,
    Error,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Pass => write!(f, "pass"),
            Status::Fail => write!(f, "fail"),
            Status::Warn => write!(f, "warn"),
            Status::Skip => write!(f, "skip"),
            Status::Error => write!(f, "error"),
        }
    }
}

impl std::str::FromStr for Status {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pass" => Ok(Status::Pass),
            "fail" => Ok(Status::Fail),
            "warn" => Ok(Status::Warn),
            "skip" => Ok(Status::Skip),
            "error" => Ok(Status::Error),
            _ => Err(format!("Invalid status: '{s}'")),
        }
    }
}

/// The gate-level verdict status (no warn or skip at the gate level)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictStatus {
    Pass,
    Fail,
    Error,
}

impl VerdictStatus {
    /// Maps the verdict to a process exit code: Pass=0, Fail=1, Error=2.
    pub fn exit_code(&self) -> i32 {
        match self {
            VerdictStatus::Pass => 0,
            VerdictStatus::Fail => 1,
            VerdictStatus::Error => 2,
        }
    }
}

impl std::fmt::Display for VerdictStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerdictStatus::Pass => write!(f, "pass"),
            VerdictStatus::Fail => write!(f, "fail"),
            VerdictStatus::Error => write!(f, "error"),
        }
    }
}

/// Outcome of a single validator run, including status, feedback, timing, and cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorResult {
    pub name: String,
    pub status: Status,
    pub feedback: Option<String>,
    pub duration_ms: i64,
    pub cost: Option<Cost>,
}

// ─── Verdict ─────────────────────────────────────────────

/// Final gate-level result containing all validator outcomes and aggregate metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub status: VerdictStatus,
    pub gate: String,
    pub failed_at: Option<String>,
    pub feedback: Option<String>,
    pub duration_ms: i64,
    pub timestamp: DateTime<Utc>,
    pub warnings: Vec<String>,
    pub suppressed: Vec<String>,
    pub history: Vec<ValidatorResult>,
}

impl Verdict {
    /// Serializes the verdict as pretty-printed JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Failed to serialize verdict")
    }

    /// Formats the verdict as a human-readable multi-line string with status icons.
    pub fn to_human(&self) -> String {
        let mut lines = Vec::new();
        for r in &self.history {
            let icon = match r.status {
                Status::Pass => "\u{2713}", // ✓
                Status::Fail => "\u{2717}", // ✗
                Status::Warn => "!",
                Status::Skip => "\u{2014}", // —
                Status::Error => "E",
            };
            let dur = format!("({}ms)", r.duration_ms);
            let status_label = if r.status == Status::Skip {
                " (skipped)".to_string()
            } else {
                String::new()
            };
            lines.push(format!("  {icon} {}{status_label} {dur}", r.name));
            if let Some(ref fb) = r.feedback {
                if r.status != Status::Pass {
                    // Indent feedback
                    for fline in fb.lines().take(5) {
                        lines.push(format!("    {fline}"));
                    }
                }
            }
        }
        let status_upper = self.status.to_string().to_uppercase();
        let failed_info = match &self.failed_at {
            Some(name) => format!(" (failed at: {name})"),
            None => String::new(),
        };
        lines.push(format!("  VERDICT: {status_upper}{failed_info}"));
        lines.join("\n")
    }

    /// Returns a one-line summary: `"PASS"` or `"FAIL at <validator>: <feedback>"`.
    pub fn to_summary(&self) -> String {
        match self.status {
            VerdictStatus::Pass => "PASS".to_string(),
            VerdictStatus::Fail | VerdictStatus::Error => {
                let prefix = self.status.to_string().to_uppercase();
                let at = self.failed_at.as_deref().unwrap_or("unknown");
                let fb = self
                    .feedback
                    .as_deref()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");
                let fb_part = if fb.is_empty() {
                    String::new()
                } else {
                    format!(": {fb}")
                };
                format!("{prefix} at {at}{fb_part}")
            }
        }
    }
}

// ─── RunOptions ──────────────────────────────────────────

/// Runtime options controlling which validators to run and how results are reported.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub run_all: bool,
    pub only: Option<Vec<String>>,
    pub skip: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub timeout: Option<u64>,
    pub log: bool,
    pub suppressed_statuses: Vec<Status>,
}

impl RunOptions {
    /// Creates default run options with logging enabled.
    pub fn new() -> Self {
        RunOptions {
            log: true,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // (all tests in this module exercise public types and methods)
    // ═══════════════════════════════════════════════════════════════

    // ─── InputFile ──────────────────────────────────

    #[test]
    fn input_file_fields() {
        // SPEC-TY-IF-001: InputFile has path, content (Option), hash (Option)
        let path = PathBuf::from("/tmp/test.txt");
        let input = InputFile::new(path.clone());
        assert_eq!(input.path, path);
        // Content and hash are not loaded yet
        assert!(input.content.is_none());
        assert!(input.hash.is_none());
    }

    #[test]
    fn input_file_lazy_content_loading() {
        // SPEC-TY-IF-002: Content loaded on first access, not at construction
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello world").unwrap();

        let mut input = InputFile::new(f.path().to_path_buf());
        // Not loaded yet
        assert!(input.content.is_none());
        // First access loads it
        let content = input.get_content().unwrap();
        assert_eq!(content, "hello world");
        // Second access returns cached value
        let content2 = input.get_content().unwrap();
        assert_eq!(content2, "hello world");
    }

    #[test]
    fn input_file_lazy_hash_computation() {
        // SPEC-TY-IF-003: Hash computed on first access, cached, SHA-256 hex
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "deterministic content").unwrap();

        let mut input = InputFile::new(f.path().to_path_buf());
        assert!(input.hash.is_none());

        let hash = input.get_hash().unwrap().to_string();
        // SHA-256 produces 64 hex chars
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Second call returns same cached value
        let hash2 = input.get_hash().unwrap();
        assert_eq!(hash, hash2);
    }

    #[test]
    fn input_file_hash_is_deterministic() {
        // Same content should produce the same hash
        let mut f1 = NamedTempFile::new().unwrap();
        let mut f2 = NamedTempFile::new().unwrap();
        write!(f1, "same content").unwrap();
        write!(f2, "same content").unwrap();

        let mut i1 = InputFile::new(f1.path().to_path_buf());
        let mut i2 = InputFile::new(f2.path().to_path_buf());
        assert_eq!(i1.get_hash().unwrap(), i2.get_hash().unwrap());
    }

    #[test]
    fn input_file_different_content_different_hash() {
        let mut f1 = NamedTempFile::new().unwrap();
        let mut f2 = NamedTempFile::new().unwrap();
        write!(f1, "content A").unwrap();
        write!(f2, "content B").unwrap();

        let mut i1 = InputFile::new(f1.path().to_path_buf());
        let mut i2 = InputFile::new(f2.path().to_path_buf());
        assert_ne!(i1.get_hash().unwrap(), i2.get_hash().unwrap());
    }

    #[test]
    fn input_file_nonexistent_returns_error() {
        let mut input = InputFile::new(PathBuf::from("/nonexistent/file.txt"));
        assert!(input.get_content().is_err());
        assert!(input.get_hash().is_err());
    }

    // ─── Invocation ─────────────────────────────────

    #[test]
    fn invocation_fields() {
        // SPEC-TY-IN-001: Invocation has validator_name, group_key, inputs
        let inv = Invocation {
            validator_name: "lint".into(),
            group_key: Some("src/main.rs".into()),
            inputs: BTreeMap::new(),
        };
        assert_eq!(inv.validator_name, "lint");
        assert_eq!(inv.group_key, Some("src/main.rs".into()));
        assert!(inv.inputs.is_empty());
    }

    #[test]
    fn invocation_with_input_files() {
        // Invocation can hold multiple named input slots with multiple files each
        let mut inputs = BTreeMap::new();
        inputs.insert(
            "code".into(),
            vec![InputFile::new(PathBuf::from("/tmp/a.py"))],
        );
        inputs.insert(
            "spec".into(),
            vec![InputFile::new(PathBuf::from("/tmp/spec.md"))],
        );

        let inv = Invocation {
            validator_name: "check".into(),
            group_key: None,
            inputs,
        };
        assert_eq!(inv.inputs.len(), 2);
        assert_eq!(inv.inputs["code"].len(), 1);
        assert_eq!(inv.inputs["spec"].len(), 1);
    }

    // ─── GateResult ─────────────────────────────────

    #[test]
    fn gate_result_fields() {
        // SPEC-TY-GR-001: GateResult has gate_name, status, validator_results, duration
        let gr = GateResult {
            gate_name: "code-review".into(),
            status: Status::Pass,
            validator_results: vec![ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: None,
                duration_ms: 50,
                cost: None,
            }],
            duration: std::time::Duration::from_millis(100),
        };
        assert_eq!(gr.gate_name, "code-review");
        assert_eq!(gr.status, Status::Pass);
        assert_eq!(gr.validator_results.len(), 1);
        assert_eq!(gr.duration.as_millis(), 100);
    }

    // ─── InvocationResult ───────────────────────────

    #[test]
    fn invocation_result_fields() {
        // SPEC-TY-IR-001: InvocationResult has id, gate_results, duration
        let ir = InvocationResult {
            id: "test-id-123".into(),
            gate_results: vec![GateResult {
                gate_name: "review".into(),
                status: Status::Fail,
                validator_results: vec![],
                duration: std::time::Duration::from_millis(200),
            }],
            duration: std::time::Duration::from_millis(300),
        };
        assert_eq!(ir.id, "test-id-123");
        assert_eq!(ir.gate_results.len(), 1);
        assert_eq!(ir.gate_results[0].status, Status::Fail);
        assert_eq!(ir.duration.as_millis(), 300);
    }

    // ─── Verdict tests ───────────────────────────────

    #[test]
    fn verdict_status_exit_codes() {
        assert_eq!(VerdictStatus::Pass.exit_code(), 0);
        assert_eq!(VerdictStatus::Fail.exit_code(), 1);
        assert_eq!(VerdictStatus::Error.exit_code(), 2);
    }

    #[test]
    fn status_display() {
        assert_eq!(Status::Pass.to_string(), "pass");
        assert_eq!(Status::Fail.to_string(), "fail");
        assert_eq!(Status::Warn.to_string(), "warn");
        assert_eq!(Status::Skip.to_string(), "skip");
        assert_eq!(Status::Error.to_string(), "error");
    }

    #[test]
    fn status_from_str() {
        assert_eq!("pass".parse::<Status>().unwrap(), Status::Pass);
        assert_eq!("fail".parse::<Status>().unwrap(), Status::Fail);
        assert_eq!("warn".parse::<Status>().unwrap(), Status::Warn);
        assert_eq!("skip".parse::<Status>().unwrap(), Status::Skip);
        assert_eq!("error".parse::<Status>().unwrap(), Status::Error);
        assert!("invalid".parse::<Status>().is_err());
    }

    #[test]
    fn verdict_to_json_roundtrip() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            gate: "test-gate".into(),
            failed_at: None,
            feedback: None,
            duration_ms: 100,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: None,
                duration_ms: 50,
                cost: None,
            }],
        };
        let json = v.to_json();
        let parsed: Verdict = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, VerdictStatus::Pass);
        assert_eq!(parsed.gate, "test-gate");
        assert_eq!(parsed.history.len(), 1);
    }

    #[test]
    fn verdict_to_human() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "code-review".into(),
            failed_at: Some("lint".into()),
            feedback: Some("missing semicolon".into()),
            duration_ms: 200,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![ValidatorResult {
                name: "lint".into(),
                status: Status::Fail,
                feedback: Some("missing semicolon".into()),
                duration_ms: 200,
                cost: None,
            }],
        };
        let human = v.to_human();
        assert!(human.contains("lint"));
        assert!(human.contains("FAIL"));
    }

    #[test]
    fn verdict_to_summary_pass() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            gate: "g".into(),
            failed_at: None,
            feedback: None,
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        assert_eq!(v.to_summary(), "PASS");
    }

    #[test]
    fn verdict_to_summary_fail() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "g".into(),
            failed_at: Some("lint".into()),
            feedback: Some("bad code".into()),
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        assert_eq!(v.to_summary(), "FAIL at lint: bad code");
    }

    // ─── Status: FromStr rejects non-lowercase ──────

    #[test]
    fn status_from_str_rejects_uppercase() {
        assert!("Pass".parse::<Status>().is_err());
        assert!("FAIL".parse::<Status>().is_err());
        assert!("Error".parse::<Status>().is_err());
    }

    #[test]
    fn status_from_str_error_includes_value() {
        let err = "bogus".parse::<Status>().unwrap_err();
        assert!(err.contains("bogus"));
    }

    // ─── VerdictStatus: Display ─────────────────────

    #[test]
    fn verdict_status_display() {
        assert_eq!(VerdictStatus::Pass.to_string(), "pass");
        assert_eq!(VerdictStatus::Fail.to_string(), "fail");
        assert_eq!(VerdictStatus::Error.to_string(), "error");
    }

    // ─── Verdict: JSON roundtrip with all fields ────

    #[test]
    fn verdict_json_roundtrip_full() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "review".into(),
            failed_at: Some("lint".into()),
            feedback: Some("bad style".into()),
            duration_ms: 500,
            timestamp: Utc::now(),

            warnings: vec!["w1".into(), "w2".into()],
            suppressed: vec!["warn".into()],
            history: vec![
                ValidatorResult {
                    name: "lint".into(),
                    status: Status::Fail,
                    feedback: Some("bad style".into()),
                    duration_ms: 300,
                    cost: Some(Cost {
                        input_tokens: Some(100),
                        output_tokens: Some(50),
                        model: Some("gpt-4".into()),
                        estimated_usd: Some(0.01),
                    }),
                },
                ValidatorResult {
                    name: "format".into(),
                    status: Status::Pass,
                    feedback: None,
                    duration_ms: 200,
                    cost: None,
                },
            ],
        };
        let json = v.to_json();
        let parsed: Verdict = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, VerdictStatus::Fail);
        assert_eq!(parsed.failed_at, Some("lint".into()));
        assert_eq!(parsed.feedback, Some("bad style".into()));
        assert_eq!(parsed.warnings, vec!["w1", "w2"]);
        assert_eq!(parsed.suppressed, vec!["warn"]);
        assert_eq!(parsed.history.len(), 2);
        assert_eq!(parsed.history[0].status, Status::Fail);
        let cost = parsed.history[0].cost.as_ref().unwrap();
        assert_eq!(cost.input_tokens, Some(100));
        assert_eq!(cost.model, Some("gpt-4".into()));
        assert!(parsed.history[1].cost.is_none());
    }

    // ─── Verdict: to_human formatting ───────────────

    #[test]
    fn verdict_to_human_skip_label() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            gate: "g".into(),
            failed_at: None,
            feedback: None,
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![ValidatorResult {
                name: "skipped-check".into(),
                status: Status::Skip,
                feedback: None,
                duration_ms: 0,
                cost: None,
            }],
        };
        let human = v.to_human();
        assert!(human.contains("(skipped)"));
        assert!(human.contains("skipped-check"));
    }

    #[test]
    fn verdict_to_human_pass_feedback_not_shown() {
        let v = Verdict {
            status: VerdictStatus::Pass,
            gate: "g".into(),
            failed_at: None,
            feedback: None,
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: Some("all good".into()),
                duration_ms: 0,
                cost: None,
            }],
        };
        let human = v.to_human();
        // Pass feedback is deliberately not displayed
        assert!(!human.contains("all good"));
    }

    #[test]
    fn verdict_to_human_feedback_truncated_to_5_lines() {
        let long_feedback = (1..=10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "g".into(),
            failed_at: Some("lint".into()),
            feedback: Some(long_feedback.clone()),
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![ValidatorResult {
                name: "lint".into(),
                status: Status::Fail,
                feedback: Some(long_feedback),
                duration_ms: 0,
                cost: None,
            }],
        };
        let human = v.to_human();
        assert!(human.contains("line 5"));
        assert!(!human.contains("line 6"));
    }

    #[test]
    fn verdict_to_human_all_status_icons() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "g".into(),
            failed_at: Some("f".into()),
            feedback: None,
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![
                ValidatorResult {
                    name: "p".into(),
                    status: Status::Pass,
                    feedback: None,
                    duration_ms: 0,
                    cost: None,
                },
                ValidatorResult {
                    name: "f".into(),
                    status: Status::Fail,
                    feedback: None,
                    duration_ms: 0,
                    cost: None,
                },
                ValidatorResult {
                    name: "w".into(),
                    status: Status::Warn,
                    feedback: None,
                    duration_ms: 0,
                    cost: None,
                },
                ValidatorResult {
                    name: "s".into(),
                    status: Status::Skip,
                    feedback: None,
                    duration_ms: 0,
                    cost: None,
                },
                ValidatorResult {
                    name: "e".into(),
                    status: Status::Error,
                    feedback: None,
                    duration_ms: 0,
                    cost: None,
                },
            ],
        };
        let human = v.to_human();
        assert!(human.contains("\u{2713}")); // ✓ pass
        assert!(human.contains("\u{2717}")); // ✗ fail
        assert!(human.contains("!")); // warn
        assert!(human.contains("\u{2014}")); // — skip
        assert!(human.contains("E")); // error
    }

    // ─── Verdict: to_summary edge cases ─────────────

    #[test]
    fn verdict_to_summary_error() {
        let v = Verdict {
            status: VerdictStatus::Error,
            gate: "g".into(),
            failed_at: Some("broken".into()),
            feedback: Some("internal error".into()),
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        assert_eq!(v.to_summary(), "ERROR at broken: internal error");
    }

    #[test]
    fn verdict_to_summary_fail_no_feedback() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "g".into(),
            failed_at: Some("lint".into()),
            feedback: None,
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        assert_eq!(v.to_summary(), "FAIL at lint");
    }

    #[test]
    fn verdict_to_summary_multiline_feedback_uses_first_line() {
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "g".into(),
            failed_at: Some("lint".into()),
            feedback: Some("first line\nsecond line\nthird".into()),
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        assert_eq!(v.to_summary(), "FAIL at lint: first line");
    }

    // ─── RunOptions ─────────────────────────────────

    #[test]
    fn run_options_new_enables_logging() {
        let opts = RunOptions::new();
        assert!(opts.log);
        assert!(!opts.run_all);
        assert!(opts.only.is_none());
        assert!(opts.skip.is_none());
        assert!(opts.tags.is_none());
        assert!(opts.timeout.is_none());
        assert!(opts.suppressed_statuses.is_empty());
    }

    #[test]
    fn run_options_default_disables_logging() {
        let opts = RunOptions::default();
        assert!(!opts.log);
    }

    #[test]
    fn verdict_to_human_no_trailing_newline() {
        // SPEC-TY-VD-428: to_human has no trailing newline
        let v = Verdict {
            status: VerdictStatus::Pass,
            gate: "g".into(),
            failed_at: None,
            feedback: None,
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![ValidatorResult {
                name: "lint".into(),
                status: Status::Pass,
                feedback: None,
                duration_ms: 10,
                cost: None,
            }],
        };
        let human = v.to_human();
        assert!(!human.ends_with('\n'));
    }

    #[test]
    fn verdict_to_summary_fail_no_failed_at_uses_unknown() {
        // SPEC-TY-VD-454: When failed_at is None for a Fail verdict,
        // to_summary uses "unknown"
        let v = Verdict {
            status: VerdictStatus::Fail,
            gate: "g".into(),
            failed_at: None,
            feedback: Some("something broke".into()),
            duration_ms: 0,
            timestamp: Utc::now(),

            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        let summary = v.to_summary();
        assert!(summary.contains("unknown"), "Summary: {summary}");
    }
}
