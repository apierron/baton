use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::{BatonError, Result};

// ─── Artifact ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Artifact {
    pub path: Option<PathBuf>,
    content: Option<Vec<u8>>,
    hash: Option<String>,
}

impl Artifact {
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

    pub fn from_string(content: &str) -> Self {
        Artifact {
            path: None,
            content: Some(content.as_bytes().to_vec()),
            hash: None,
        }
    }

    pub fn from_bytes(content: Vec<u8>) -> Self {
        Artifact {
            path: None,
            content: Some(content),
            hash: None,
        }
    }

    pub fn get_content(&mut self) -> Result<&[u8]> {
        if self.content.is_none() {
            let path = self.path.as_ref().expect("Artifact must have path or content");
            self.content = Some(std::fs::read(path)?);
        }
        Ok(self.content.as_ref().unwrap())
    }

    pub fn get_hash(&mut self) -> Result<String> {
        if self.hash.is_none() {
            let content = self.get_content()?;
            let mut hasher = Sha256::new();
            hasher.update(content);
            self.hash = Some(hex::encode(hasher.finalize()));
        }
        Ok(self.hash.clone().unwrap())
    }

    pub fn get_content_as_string(&mut self) -> Result<String> {
        let bytes = self.get_content()?.to_vec();
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn absolute_path(&self) -> Option<String> {
        self.path.as_ref().map(|p| p.display().to_string())
    }

    pub fn parent_dir(&self) -> Option<String> {
        self.path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.display().to_string())
    }
}

// ─── Context ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContextItem {
    pub name: String,
    pub path: Option<PathBuf>,
    content: Option<String>,
}

impl ContextItem {
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

    pub fn from_string(name: String, content: String) -> Self {
        ContextItem {
            name,
            path: None,
            content: Some(content),
        }
    }

    pub fn get_content(&mut self) -> Result<&str> {
        if self.content.is_none() {
            let path = self.path.as_ref().expect("ContextItem must have path or content");
            self.content = Some(std::fs::read_to_string(path)?);
        }
        Ok(self.content.as_ref().unwrap())
    }

    pub fn get_hash(&mut self) -> Result<String> {
        let content = self.get_content()?.to_string();
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        Ok(hex::encode(hasher.finalize()))
    }

    pub fn absolute_path(&self) -> Option<String> {
        self.path.as_ref().map(|p| p.display().to_string())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Context {
    pub items: BTreeMap<String, ContextItem>,
}

impl Context {
    pub fn new() -> Self {
        Context {
            items: BTreeMap::new(),
        }
    }

    pub fn add_file(&mut self, name: String, path: impl AsRef<Path>) -> Result<()> {
        let item = ContextItem::from_file(name.clone(), path)?;
        self.items.insert(name, item);
        Ok(())
    }

    pub fn add_string(&mut self, name: String, content: String) {
        let item = ContextItem::from_string(name.clone(), content);
        self.items.insert(name, item);
    }

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cost {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub model: Option<String>,
    pub estimated_usd: Option<f64>,
}

// ─── ValidatorResult ─────────────────────────────────────

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorResult {
    pub name: String,
    pub status: Status,
    pub feedback: Option<String>,
    pub duration_ms: i64,
    pub cost: Option<Cost>,
}

// ─── Verdict ─────────────────────────────────────────────

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
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("Failed to serialize verdict")
    }

    pub fn to_human(&self) -> String {
        let mut lines = Vec::new();
        for r in &self.history {
            let icon = match r.status {
                Status::Pass => "\u{2713}",  // ✓
                Status::Fail => "\u{2717}",  // ✗
                Status::Warn => "!",
                Status::Skip => "\u{2014}",  // —
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
            history: vec![
                ValidatorResult {
                    name: "lint".into(),
                    status: Status::Fail,
                    feedback: Some("missing semicolon".into()),
                    duration_ms: 200,
                    cost: None,
                },
            ],
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
}
