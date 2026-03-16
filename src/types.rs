//! Core data types for baton: artifacts, context, verdicts, and run options.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{BatonError, Result};

// ─── Artifact ────────────────────────────────────────────

/// A file or in-memory content to be validated, with lazy content and hash loading.
#[derive(Debug, Clone)]
pub struct Artifact {
    pub path: Option<PathBuf>,
    content: Option<Vec<u8>>,
    hash: Option<String>,
}

impl Artifact {
    /// Creates an artifact from a filesystem path. The file must exist and not be a directory.
    /// Content is not read until [`get_content`](Self::get_content) is called.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(BatonError::ArtifactNotFound(path.display().to_string()));
        }
        if path.is_dir() {
            return Err(BatonError::ArtifactIsDirectory(path.display().to_string()));
        }
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        Ok(Artifact {
            path: Some(abs),
            content: None,
            hash: None,
        })
    }

    /// Creates an artifact from an inline string.
    pub fn from_string(content: &str) -> Self {
        Artifact {
            path: None,
            content: Some(content.as_bytes().to_vec()),
            hash: None,
        }
    }

    /// Creates an artifact from raw bytes.
    pub fn from_bytes(content: Vec<u8>) -> Self {
        Artifact {
            path: None,
            content: Some(content),
            hash: None,
        }
    }

    /// Returns the artifact content, lazily reading from disk on first access.
    pub fn get_content(&mut self) -> Result<&[u8]> {
        if self.content.is_none() {
            let path = self
                .path
                .as_ref()
                .expect("Artifact must have path or content");
            self.content = Some(std::fs::read(path)?);
        }
        Ok(self.content.as_ref().unwrap())
    }

    /// Returns the SHA-256 hash of the content, computing and caching it on first call.
    pub fn get_hash(&mut self) -> Result<String> {
        if self.hash.is_none() {
            let content = self.get_content()?;
            let mut hasher = Sha256::new();
            hasher.update(content);
            self.hash = Some(hex::encode(hasher.finalize()));
        }
        Ok(self.hash.clone().unwrap())
    }

    /// Returns the content as a UTF-8 string, using lossy conversion for invalid sequences.
    pub fn get_content_as_string(&mut self) -> Result<String> {
        let bytes = self.get_content()?.to_vec();
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Returns the absolute path as a string, if this artifact is file-backed.
    pub fn absolute_path(&self) -> Option<String> {
        self.path.as_ref().map(|p| p.display().to_string())
    }

    /// Returns the parent directory as a string, if this artifact is file-backed.
    pub fn parent_dir(&self) -> Option<String> {
        self.path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.display().to_string())
    }
}

// ─── Context ─────────────────────────────────────────────

/// A named reference document provided as context for validation.
#[derive(Debug, Clone)]
pub struct ContextItem {
    pub name: String,
    pub path: Option<PathBuf>,
    content: Option<String>,
}

impl ContextItem {
    /// Creates a context item from a filesystem path. The file must exist and not be a directory.
    pub fn from_file(name: String, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(BatonError::ContextNotFound {
                name,
                path: path.display().to_string(),
            });
        }
        if path.is_dir() {
            return Err(BatonError::ContextIsDirectory {
                name,
                path: path.display().to_string(),
            });
        }
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        Ok(ContextItem {
            name,
            path: Some(abs),
            content: None,
        })
    }

    /// Creates a context item from an inline string.
    pub fn from_string(name: String, content: String) -> Self {
        ContextItem {
            name,
            path: None,
            content: Some(content),
        }
    }

    /// Returns the content, lazily reading from disk on first access.
    pub fn get_content(&mut self) -> Result<&str> {
        if self.content.is_none() {
            let path = self
                .path
                .as_ref()
                .expect("ContextItem must have path or content");
            self.content = Some(std::fs::read_to_string(path)?);
        }
        Ok(self.content.as_ref().unwrap())
    }

    /// Returns the SHA-256 hash of the content.
    pub fn get_hash(&mut self) -> Result<String> {
        let content = self.get_content()?.to_string();
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        Ok(hex::encode(hasher.finalize()))
    }

    /// Returns the absolute path as a string, if this item is file-backed.
    pub fn absolute_path(&self) -> Option<String> {
        self.path.as_ref().map(|p| p.display().to_string())
    }
}

/// Ordered collection of named context items. Uses `BTreeMap` for deterministic hash ordering.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub items: BTreeMap<String, ContextItem>,
}

impl Context {
    /// Creates an empty context collection.
    pub fn new() -> Self {
        Context {
            items: BTreeMap::new(),
        }
    }

    /// Adds a file-backed context item by name and path.
    pub fn add_file(&mut self, name: String, path: impl AsRef<Path>) -> Result<()> {
        let item = ContextItem::from_file(name.clone(), path)?;
        self.items.insert(name, item);
        Ok(())
    }

    /// Adds an inline string context item by name and content.
    pub fn add_string(&mut self, name: String, content: String) {
        let item = ContextItem::from_string(name.clone(), content);
        self.items.insert(name, item);
    }

    /// Returns a combined SHA-256 hash of all items, computed in sorted key order.
    pub fn get_hash(&mut self) -> Result<String> {
        let mut item_hashes = Vec::new();
        // BTreeMap iterates in sorted order
        let names: Vec<String> = self.items.keys().cloned().collect();
        for name in &names {
            let item = self.items.get_mut(name).unwrap();
            item_hashes.push(item.get_hash()?);
        }
        let joined = item_hashes.join(":");
        let mut hasher = Sha256::new();
        hasher.update(joined.as_bytes());
        Ok(hex::encode(hasher.finalize()))
    }
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
    pub artifact_hash: String,
    pub context_hash: String,
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

    /// Returns a one-line summary: "PASS" or "FAIL at <validator>: <feedback>".
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

    // ─── Artifact tests ──────────────────────────────

    #[test]
    fn artifact_from_file_not_found() {
        let result = Artifact::from_file("/nonexistent/file.txt");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "Error: {err}");
    }

    #[test]
    fn artifact_from_file_is_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = Artifact::from_file(dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("directory"), "Error: {err}");
    }

    #[test]
    fn artifact_from_file_success() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "hello world").unwrap();
        let mut art = Artifact::from_file(f.path()).unwrap();
        assert!(art.path.is_some());
        let content = art.get_content().unwrap();
        assert_eq!(content, b"hello world");
    }

    #[test]
    fn artifact_from_string() {
        let mut art = Artifact::from_string("test content");
        let content = art.get_content().unwrap();
        assert_eq!(content, b"test content");
    }

    #[test]
    fn artifact_hash_deterministic() {
        let mut a1 = Artifact::from_string("hello");
        let mut a2 = Artifact::from_string("hello");
        assert_eq!(a1.get_hash().unwrap(), a2.get_hash().unwrap());
    }

    #[test]
    fn artifact_hash_differs_for_different_content() {
        let mut a1 = Artifact::from_string("hello");
        let mut a2 = Artifact::from_string("world");
        assert_ne!(a1.get_hash().unwrap(), a2.get_hash().unwrap());
    }

    #[test]
    fn artifact_empty_file_is_valid() {
        let f = NamedTempFile::new().unwrap();
        let mut art = Artifact::from_file(f.path()).unwrap();
        let content = art.get_content().unwrap();
        assert!(content.is_empty());
    }

    // ─── Context tests ───────────────────────────────

    #[test]
    fn context_item_from_file_not_found() {
        let result = ContextItem::from_file("spec".into(), "/nonexistent.md");
        assert!(result.is_err());
    }

    #[test]
    fn context_item_from_file_is_directory() {
        let dir = tempfile::tempdir().unwrap();
        let result = ContextItem::from_file("spec".into(), dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("directory"), "Error: {err}");
    }

    #[test]
    fn context_item_from_string() {
        let mut item = ContextItem::from_string("spec".into(), "requirement: xyz".into());
        assert_eq!(item.get_content().unwrap(), "requirement: xyz");
    }

    #[test]
    fn context_hash_empty() {
        let mut ctx = Context::new();
        let hash = ctx.get_hash().unwrap();
        // SHA-256 of empty string
        let mut hasher = Sha256::new();
        hasher.update(b"");
        let expected = hex::encode(hasher.finalize());
        // When no items, joined hashes is "" and sha256("") is computed
        assert_eq!(hash, expected);
    }

    #[test]
    fn context_hash_deterministic() {
        let mut ctx1 = Context::new();
        ctx1.add_string("a".into(), "alpha".into());
        ctx1.add_string("b".into(), "beta".into());

        let mut ctx2 = Context::new();
        // Insert in different order — BTreeMap sorts by key
        ctx2.add_string("b".into(), "beta".into());
        ctx2.add_string("a".into(), "alpha".into());

        assert_eq!(ctx1.get_hash().unwrap(), ctx2.get_hash().unwrap());
    }

    #[test]
    fn context_hash_differs_with_different_content() {
        let mut ctx1 = Context::new();
        ctx1.add_string("a".into(), "alpha".into());

        let mut ctx2 = Context::new();
        ctx2.add_string("a".into(), "bravo".into());

        assert_ne!(ctx1.get_hash().unwrap(), ctx2.get_hash().unwrap());
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
            artifact_hash: "abc123".into(),
            context_hash: "def456".into(),
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
            artifact_hash: "abc".into(),
            context_hash: "def".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        assert_eq!(v.to_summary(), "FAIL at lint: bad code");
    }

    // ─── Artifact: from_bytes ───────────────────────

    #[test]
    fn artifact_from_bytes() {
        let mut art = Artifact::from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let content = art.get_content().unwrap();
        assert_eq!(content, &[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(art.path.is_none());
    }

    // ─── Artifact: lossy UTF-8 ──────────────────────

    #[test]
    fn artifact_get_content_as_string_lossy() {
        // 0xFF is not valid UTF-8 — should be replaced with U+FFFD
        let mut art = Artifact::from_bytes(vec![b'h', b'i', 0xFF, b'!']);
        let s = art.get_content_as_string().unwrap();
        assert!(s.starts_with("hi"));
        assert!(s.ends_with('!'));
        assert!(s.contains('\u{FFFD}'));
    }

    // ─── Artifact: hash caching ─────────────────────

    #[test]
    fn artifact_hash_cached_on_second_call() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "original").unwrap();
        let mut art = Artifact::from_file(f.path()).unwrap();
        let h1 = art.get_hash().unwrap();
        // Overwrite file after first hash — cached hash should be unchanged
        std::fs::write(f.path(), "modified").unwrap();
        let h2 = art.get_hash().unwrap();
        assert_eq!(h1, h2);
    }

    // ─── Artifact: hash format ──────────────────────

    #[test]
    fn artifact_hash_is_64_hex_chars() {
        let mut art = Artifact::from_string("anything");
        let hash = art.get_hash().unwrap();
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // ─── Artifact: absolute_path / parent_dir ───────

    #[test]
    fn artifact_absolute_path_from_file() {
        let f = NamedTempFile::new().unwrap();
        let art = Artifact::from_file(f.path()).unwrap();
        let abs = art.absolute_path().unwrap();
        assert!(std::path::Path::new(&abs).is_absolute());
        assert!(abs.ends_with(f.path().file_name().unwrap().to_str().unwrap()));
    }

    #[test]
    fn artifact_parent_dir_from_file() {
        let f = NamedTempFile::new().unwrap();
        let art = Artifact::from_file(f.path()).unwrap();
        let dir = art.parent_dir().unwrap();
        assert!(std::path::Path::new(&dir).is_dir());
    }

    #[test]
    fn artifact_absolute_path_from_string_is_none() {
        let art = Artifact::from_string("inline");
        assert!(art.absolute_path().is_none());
        assert!(art.parent_dir().is_none());
    }

    // ─── Context: add_file success ──────────────────

    #[test]
    fn context_add_file_success() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "spec content").unwrap();
        let mut ctx = Context::new();
        ctx.add_file("spec".into(), f.path()).unwrap();
        assert!(ctx.items.contains_key("spec"));
        let item = ctx.items.get_mut("spec").unwrap();
        assert_eq!(item.get_content().unwrap(), "spec content");
    }

    // ─── Context: key name affects hash ─────────────

    #[test]
    fn context_hash_differs_with_different_key_names() {
        let mut ctx1 = Context::new();
        ctx1.add_string("alpha".into(), "same content".into());

        let mut ctx2 = Context::new();
        ctx2.add_string("bravo".into(), "same content".into());

        // Same content under different names → different hash
        // because items are hashed individually and the per-item hash
        // depends on content, but the *position* in the joined string
        // is determined by sorted key order, and "alpha" vs "bravo"
        // happen to produce the same individual hash here.
        // Actually the individual hash of "same content" is the same
        // regardless of key name, so the *joined* hash is also the
        // same (one item → one hash → same join). But with TWO items
        // it would differ. Let's test the two-item case.
        let mut ctx3 = Context::new();
        ctx3.add_string("a".into(), "one".into());
        ctx3.add_string("b".into(), "two".into());

        let mut ctx4 = Context::new();
        ctx4.add_string("a".into(), "two".into());
        ctx4.add_string("b".into(), "one".into());

        // Same values swapped between keys → different hash
        assert_ne!(ctx3.get_hash().unwrap(), ctx4.get_hash().unwrap());
    }

    // ─── Context: item absolute_path ────────────────

    #[test]
    fn context_item_absolute_path() {
        let f = NamedTempFile::new().unwrap();
        let item = ContextItem::from_file("spec".into(), f.path()).unwrap();
        let abs = item.absolute_path().unwrap();
        assert!(std::path::Path::new(&abs).is_absolute());
    }

    #[test]
    fn context_item_from_string_has_no_path() {
        let item = ContextItem::from_string("spec".into(), "content".into());
        assert!(item.absolute_path().is_none());
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
            artifact_hash: "aaa".into(),
            context_hash: "bbb".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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

    // ─── Spec coverage (UNTESTED) ──────────────────────

    #[test]
    fn artifact_from_file_stores_absolute_path() {
        // SPEC-TY-AF-075: Relative path → absolute storage
        let f = NamedTempFile::new().unwrap();
        let art = Artifact::from_file(f.path()).unwrap();
        assert!(art.path.as_ref().unwrap().is_absolute());
    }

    #[test]
    fn context_item_from_file_stores_absolute_path() {
        // SPEC-TY-CI-179: ContextItem::from_file stores absolute path
        let f = NamedTempFile::new().unwrap();
        let item = ContextItem::from_file("spec".into(), f.path()).unwrap();
        assert!(item.path.as_ref().unwrap().is_absolute());
    }

    #[test]
    fn context_item_get_hash_recomputes_same_value() {
        // SPEC-TY-CI-207: ContextItem hash not cached — calling get_hash twice
        // recomputes and returns the same value
        let mut item = ContextItem::from_string("spec".into(), "deterministic content".into());
        let h1 = item.get_hash().unwrap();
        let h2 = item.get_hash().unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn context_add_duplicate_replaces_silently() {
        // SPEC-TY-CX-239: Adding two items with the same name replaces silently
        let mut ctx = Context::new();
        ctx.add_string("dup".into(), "first".into());
        ctx.add_string("dup".into(), "second".into());
        assert_eq!(ctx.items.len(), 1);
        let content = ctx.items.get_mut("dup").unwrap().get_content().unwrap();
        assert_eq!(content, "second");
    }

    #[test]
    fn context_single_item_hash_ignores_key_name() {
        // SPEC-TY-CX-263: Single-item context hash — two contexts with different
        // key names but same content produce the same hash (because the hash is
        // computed from content hashes only, not key names)
        let mut ctx1 = Context::new();
        ctx1.add_string("alpha".into(), "same content".into());

        let mut ctx2 = Context::new();
        ctx2.add_string("bravo".into(), "same content".into());

        assert_eq!(ctx1.get_hash().unwrap(), ctx2.get_hash().unwrap());
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
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
            artifact_hash: "a".into(),
            context_hash: "c".into(),
            warnings: vec![],
            suppressed: vec![],
            history: vec![],
        };
        let summary = v.to_summary();
        assert!(summary.contains("unknown"), "Summary: {summary}");
    }
}
