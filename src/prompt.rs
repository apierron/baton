//! Prompt template parsing and resolution.
//!
//! Supports optional TOML frontmatter delimited by `+++` for metadata
//! (description, expected response format). Templates without frontmatter
//! default to expecting a verdict-format response.

use crate::error::{BatonError, Result};
use std::path::Path;

/// Parsed prompt template from a file with optional TOML frontmatter.
#[derive(Debug, Clone)]
pub struct PromptTemplate {
    pub name: String,
    pub description: Option<String>,
    pub expects: TemplateExpects,
    pub body: String,
}

/// The expected response format from the LLM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateExpects {
    Verdict,
    Freeform,
}

impl std::fmt::Display for TemplateExpects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateExpects::Verdict => write!(f, "verdict"),
            TemplateExpects::Freeform => write!(f, "freeform"),
        }
    }
}

impl std::str::FromStr for TemplateExpects {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "verdict" => Ok(TemplateExpects::Verdict),
            "freeform" => Ok(TemplateExpects::Freeform),
            _ => Err(format!(
                "'expects' must be 'verdict' or 'freeform', got '{s}'"
            )),
        }
    }
}

/// Parse a prompt template from a file path.
pub fn parse_template(file_path: &Path) -> Result<PromptTemplate> {
    let name = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let raw = std::fs::read_to_string(file_path)
        .map_err(|e| BatonError::PromptError(format!("Template {}: {}", file_path.display(), e)))?;

    parse_template_str(&raw, &name, &file_path.display().to_string())
}

/// Parse a prompt template from a string (for testing and inline use).
pub fn parse_template_str(raw: &str, name: &str, source: &str) -> Result<PromptTemplate> {
    let (description, expects, body) = if let Some(rest) = raw.strip_prefix("+++") {
        // Find closing +++
        let end_index = rest.find("+++");
        let end_index = match end_index {
            Some(i) => i,
            None => {
                return Err(BatonError::PromptError(format!(
                    "Template {source}: opening +++ without closing +++"
                )));
            }
        };

        let frontmatter_text = rest[..end_index].trim();
        let body = rest[end_index + 3..].trim().to_string();

        // Parse frontmatter as TOML
        let frontmatter: toml::Value = toml::from_str(frontmatter_text).map_err(|e| {
            BatonError::PromptError(format!("Template {source}: frontmatter parse error: {e}"))
        })?;

        let table = frontmatter.as_table().ok_or_else(|| {
            BatonError::PromptError(format!(
                "Template {source}: frontmatter must be a TOML table"
            ))
        })?;

        let expects_str = table
            .get("expects")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BatonError::PromptError(format!(
                    "Template {source}: frontmatter missing required 'expects' field"
                ))
            })?;

        let expects: TemplateExpects = expects_str
            .parse()
            .map_err(|e: String| BatonError::PromptError(format!("Template {source}: {e}")))?;

        let description = table
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        (description, expects, body)
    } else {
        // No frontmatter
        let body = raw.trim().to_string();
        (None, TemplateExpects::Verdict, body)
    };

    if body.is_empty() {
        return Err(BatonError::PromptError(format!(
            "Template {source}: prompt body is empty"
        )));
    }

    Ok(PromptTemplate {
        name: name.to_string(),
        description,
        expects,
        body,
    })
}

/// Check if a prompt value looks like a file reference (has a recognized extension).
pub fn is_file_reference(prompt_value: &str) -> bool {
    let extensions = [".md", ".txt", ".prompt", ".j2"];
    extensions.iter().any(|ext| prompt_value.ends_with(ext))
}

/// Resolve a prompt value to its content.
/// If it's a file reference, load from prompts_dir or as a literal path.
/// Otherwise, treat as inline text.
pub fn resolve_prompt_value(
    prompt_value: &str,
    prompts_dir: &Path,
    config_dir: &Path,
) -> Result<PromptTemplate> {
    if is_file_reference(prompt_value) {
        // Try prompts_dir first
        let in_prompts_dir = prompts_dir.join(prompt_value);
        if in_prompts_dir.exists() {
            return parse_template(&in_prompts_dir);
        }

        // Try as literal path (absolute or relative to config dir)
        let as_path = Path::new(prompt_value);
        let resolved = if as_path.is_absolute() {
            as_path.to_path_buf()
        } else {
            config_dir.join(prompt_value)
        };

        if resolved.exists() {
            return parse_template(&resolved);
        }

        Err(BatonError::PromptError(format!(
            "Prompt file not found: '{prompt_value}' (searched in '{}' and '{}')",
            prompts_dir.display(),
            config_dir.display()
        )))
    } else {
        // Inline prompt string
        parse_template_str(prompt_value, "inline", "inline")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    // ═══════════════════════════════════════════════════════════════
    // Internal implementation tests
    // NOTE: parse_template_str and is_file_reference are pub but are
    //       low-level helpers; the public entry points are
    //       parse_template and resolve_prompt_value.
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn parse_template_with_frontmatter() {
        let raw = r#"+++
description = "Check spec compliance"
expects = "verdict"
+++

You are a code reviewer.
Check the spec.

{artifact_content}"#;
        let t = parse_template_str(raw, "spec-compliance", "test").unwrap();
        assert_eq!(t.name, "spec-compliance");
        assert_eq!(t.description, Some("Check spec compliance".into()));
        assert_eq!(t.expects, TemplateExpects::Verdict);
        assert!(t.body.contains("code reviewer"));
        assert!(t.body.contains("{artifact_content}"));
    }

    #[test]
    fn parse_template_without_frontmatter() {
        let raw = "You are a reviewer.\nCheck the code.\n{artifact_content}";
        let t = parse_template_str(raw, "simple", "test").unwrap();
        assert_eq!(t.expects, TemplateExpects::Verdict); // default
        assert!(t.description.is_none());
        assert!(t.body.contains("reviewer"));
    }

    #[test]
    fn parse_template_missing_closing_delimiters() {
        let raw = "+++\nexpects = \"verdict\"\nNo closing delimiter";
        let result = parse_template_str(raw, "bad", "test");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("opening +++ without closing +++"),
            "Error: {err}"
        );
    }

    #[test]
    fn parse_template_missing_expects() {
        let raw = "+++\ndescription = \"test\"\n+++\nBody here";
        let result = parse_template_str(raw, "bad", "test");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("expects"), "Error: {err}");
    }

    #[test]
    fn parse_template_invalid_expects() {
        let raw = "+++\nexpects = \"invalid\"\n+++\nBody here";
        let result = parse_template_str(raw, "bad", "test");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("verdict"), "Error: {err}");
    }

    #[test]
    fn parse_template_empty_body() {
        let raw = "+++\nexpects = \"verdict\"\n+++\n";
        let result = parse_template_str(raw, "empty", "test");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "Error: {err}");
    }

    #[test]
    fn parse_template_freeform_expects() {
        let raw = "+++\nexpects = \"freeform\"\n+++\nJust review this.";
        let t = parse_template_str(raw, "review", "test").unwrap();
        assert_eq!(t.expects, TemplateExpects::Freeform);
    }

    #[test]
    fn is_file_reference_md() {
        assert!(is_file_reference("spec-compliance.md"));
    }

    #[test]
    fn is_file_reference_txt() {
        assert!(is_file_reference("check.txt"));
    }

    #[test]
    fn is_file_reference_prompt() {
        assert!(is_file_reference("review.prompt"));
    }

    #[test]
    fn is_file_reference_j2() {
        assert!(is_file_reference("template.j2"));
    }

    #[test]
    fn is_file_reference_no_extension() {
        assert!(!is_file_reference("spec-compliance"));
        assert!(!is_file_reference("Just review this code"));
    }

    // ═══════════════════════════════════════════════════════════════
    // Behavioral contract tests
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn resolve_prompt_inline() {
        let dir = TempDir::new().unwrap();
        let t = resolve_prompt_value("Check this code", dir.path(), dir.path()).unwrap();
        assert_eq!(t.body, "Check this code");
        assert_eq!(t.expects, TemplateExpects::Verdict);
    }

    #[test]
    fn resolve_prompt_file_in_prompts_dir() {
        let dir = TempDir::new().unwrap();
        let prompts_dir = dir.path().join("prompts");
        std::fs::create_dir(&prompts_dir).unwrap();
        let mut f = std::fs::File::create(prompts_dir.join("check.md")).unwrap();
        write!(f, "+++\nexpects = \"verdict\"\n+++\nReview the code.").unwrap();

        let t = resolve_prompt_value("check.md", &prompts_dir, dir.path()).unwrap();
        assert_eq!(t.name, "check");
        assert!(t.body.contains("Review"));
    }

    #[test]
    fn resolve_prompt_file_not_found() {
        let dir = TempDir::new().unwrap();
        let result = resolve_prompt_value("nonexistent.md", dir.path(), dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "Error: {err}");
    }

    #[test]
    fn parse_template_from_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("spec-compliance.md");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            "+++\nexpects = \"verdict\"\ndescription = \"Check spec\"\n+++\nReview {{artifact_content}}"
        )
        .unwrap();

        let t = parse_template(&path).unwrap();
        assert_eq!(t.name, "spec-compliance");
        assert_eq!(t.expects, TemplateExpects::Verdict);
    }

    #[test]
    fn parse_template_non_utf8_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.md");
        std::fs::write(&path, [0xFF, 0xFE, 0x00, 0x01]).unwrap();
        let result = parse_template(&path);
        assert!(result.is_err());
    }

    // ─── Spec coverage (UNTESTED) ──────────────────────

    #[test]
    fn toml_syntax_error_in_frontmatter() {
        let raw = "+++\nnot: [valid toml\n+++\nbody here";
        let result = parse_template_str(raw, "test", "test.md");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("frontmatter parse error"),
            "Expected 'frontmatter parse error', got: {err}"
        );
    }

    #[test]
    fn non_table_toml_frontmatter() {
        // TOML top-level is always a table, so a bare string literal like
        // `"just a string"` is invalid TOML syntax and triggers a parse
        // error before the as_table() check is reached.
        let raw = "+++\n\"just a string\"\n+++\nbody";
        let result = parse_template_str(raw, "test", "test.md");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("frontmatter parse error"),
            "Expected 'frontmatter parse error', got: {err}"
        );
    }

    #[test]
    fn empty_string_without_frontmatter() {
        let result = parse_template_str("", "test", "test.md");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "Expected 'empty' error, got: {err}");
    }

    #[test]
    fn whitespace_only_without_frontmatter() {
        let result = parse_template_str("   \n  ", "test", "test.md");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "Expected 'empty' error, got: {err}");
    }

    #[test]
    fn unknown_frontmatter_fields_ignored() {
        let raw =
            "+++\nfoo = \"bar\"\nunknown_field = 42\nexpects = \"verdict\"\n+++\nSome prompt body";
        let t = parse_template_str(raw, "test", "test.md").unwrap();
        assert_eq!(t.expects, TemplateExpects::Verdict);
        assert_eq!(t.body, "Some prompt body");
    }

    #[test]
    fn template_expects_verdict_display() {
        assert_eq!(format!("{}", TemplateExpects::Verdict), "verdict");
    }

    #[test]
    fn template_expects_freeform_display() {
        assert_eq!(format!("{}", TemplateExpects::Freeform), "freeform");
    }

    #[test]
    fn is_file_reference_suffix_match_edge_cases() {
        assert!(
            is_file_reference("use file.md"),
            "Should match because it ends with .md"
        );
        assert!(
            is_file_reference("review.prompt"),
            "Should match because it ends with .prompt"
        );
    }

    #[test]
    fn resolve_prompt_config_dir_fallback() {
        let prompts_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        // Create the file only in config_dir, not in prompts_dir
        let mut f = std::fs::File::create(config_dir.path().join("fallback.md")).unwrap();
        write!(f, "+++\nexpects = \"verdict\"\n+++\nFallback body").unwrap();

        let t = resolve_prompt_value("fallback.md", prompts_dir.path(), config_dir.path()).unwrap();
        assert_eq!(t.name, "fallback");
        assert_eq!(t.body, "Fallback body");
    }

    #[test]
    fn resolve_prompt_file_with_invalid_frontmatter() {
        let dir = TempDir::new().unwrap();
        let prompts_dir = dir.path().join("prompts");
        std::fs::create_dir(&prompts_dir).unwrap();

        let mut f = std::fs::File::create(prompts_dir.join("bad.md")).unwrap();
        write!(f, "+++\nnot: [valid toml\n+++\nbody here").unwrap();

        let result = resolve_prompt_value("bad.md", &prompts_dir, dir.path());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("frontmatter parse error"),
            "Expected parse error to propagate, got: {err}"
        );
    }

    #[test]
    fn resolve_prompt_inline_name_is_inline() {
        let dir = TempDir::new().unwrap();
        let t =
            resolve_prompt_value("Just some inline prompt text", dir.path(), dir.path()).unwrap();
        assert_eq!(t.name, "inline");
        assert_eq!(t.body, "Just some inline prompt text");
    }
}
