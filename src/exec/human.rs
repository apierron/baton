//! Human review validator execution.

use std::collections::BTreeMap;

use crate::placeholder::{resolve_placeholders, ResolutionWarnings};
use crate::types::*;

pub(super) fn execute_human_validator(
    validator: &crate::config::ValidatorConfig,
    inputs: &mut BTreeMap<String, Vec<InputFile>>,
    prior_results: &BTreeMap<String, ValidatorResult>,
) -> ValidatorResult {
    let prompt = validator.prompt.as_deref().unwrap_or("");
    let mut warnings = ResolutionWarnings::new();
    let rendered = resolve_placeholders(prompt, inputs, prior_results, &mut warnings);

    ValidatorResult {
        name: validator.name.clone(),
        status: Status::Fail,
        feedback: Some(format!("[human-review-requested] {rendered}")),
        duration_ms: 0,
        cost: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::execute_validator;
    use crate::test_helpers::ValidatorBuilder;

    // ─── Human validator tests ───────────────────────

    #[test]
    fn human_validator_fails_with_prompt() {
        let v = ValidatorBuilder::human("human", "Please review this change.").build();

        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("[human-review-requested]"));
        assert!(result.feedback.as_ref().unwrap().contains("Please review"));
    }

    // ─── Human validator edge cases ──────────────────────

    #[test]
    fn human_validator_with_placeholders_in_prompt() {
        use std::io::Write as _;
        let mut tmpf = tempfile::NamedTempFile::new().unwrap();
        write!(tmpf, "fn main() {{}}").unwrap();
        let v = ValidatorBuilder::human("human-ph", "Review {file.content} please").build();
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        inputs.insert(
            "file".into(),
            vec![InputFile::new(tmpf.path().to_path_buf())],
        );
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        assert!(result.feedback.as_ref().unwrap().contains("fn main() {}"));
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("[human-review-requested]"));
    }

    #[test]
    fn human_validator_with_empty_prompt() {
        // prompt is None — the builder with "" sets Some(""), which resolves to ""
        let mut v = ValidatorBuilder::human("human-empty", "").build();
        v.prompt = None;
        let mut inputs: BTreeMap<String, Vec<InputFile>> = BTreeMap::new();
        let prior = BTreeMap::new();
        let result = execute_validator(&v, &mut inputs, &prior, None);
        assert_eq!(result.status, Status::Fail);
        // With None prompt, it falls back to "" and renders "[human-review-requested] "
        assert!(result
            .feedback
            .as_ref()
            .unwrap()
            .contains("[human-review-requested]"));
    }
}
