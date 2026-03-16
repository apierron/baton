# module: prompt

Prompt template parsing and resolution. Supports optional TOML frontmatter delimited by `+++` for metadata (description, expected response format). Templates without frontmatter default to expecting a verdict-format response.

## Public functions

| Function              | Purpose                                                        |
|-----------------------|----------------------------------------------------------------|
| `parse_template`      | Parse a prompt template from a file path                       |
| `parse_template_str`  | Parse a prompt template from a string                          |
| `is_file_reference`   | Check if prompt value has a recognized extension                |
| `resolve_prompt_value`| Resolve prompt: file reference or inline string to a template  |

## Types

| Type              | Purpose                                                |
|-------------------|--------------------------------------------------------|
| `PromptTemplate`  | Parsed template: name, description, expects, body     |
| `TemplateExpects`  | Response format: Verdict (default) or Freeform        |

## Design notes

TemplateExpects defaults to Verdict when no frontmatter is present. This matches the common case: most LLM validators are expected to produce a structured pass/fail verdict. The Freeform variant exists for session-mode validators where the LLM interacts with a runtime and the verdict comes from observing the session outcome, not from parsing the LLM's response.

The frontmatter format uses `+++` delimiters (Hugo/Zola-style) rather than `---` (Jekyll-style) to avoid ambiguity with YAML documents and horizontal rules in Markdown prompt bodies. This is a deliberate choice to keep the parser simple: `---` appears frequently in natural prose and Markdown, while `+++` is rare enough to be unambiguous.

parse_template_str takes a `source` parameter separate from `name`. The name is used for the PromptTemplate.name field (derived from file stem for file-backed templates, "inline" for inline prompts). The source is used in error messages to help the user locate the problem (file path for file-backed, "inline" for inline).

---

## parse_template

Parses a prompt template from a file on disk. Derives the template name from the file stem and delegates body parsing to parse_template_str.

SPEC-PR-PT-001: name-from-file-stem
  The template name is derived from the file path's stem (filename without extension). For "spec-compliance.md", the name is "spec-compliance". If the path has no file stem or the stem is not valid UTF-8, the name defaults to an empty string.
  test: prompt::tests::parse_template_from_file

SPEC-PR-PT-002: file-read-error-returns-prompt-error
  When the file cannot be read (does not exist, permission denied, or contains non-UTF-8 bytes), parse_template returns Err(PromptError) with a message containing the file path and the underlying IO error description.
  test: prompt::tests::parse_template_non_utf8_file

SPEC-PR-PT-003: delegates-to-parse-template-str
  After reading the file content, parse_template delegates to parse_template_str with the raw content, the derived name, and the file path display string as the source. All frontmatter parsing, validation, and body extraction are handled by parse_template_str.
  test: IMPLICIT via prompt::tests::parse_template_from_file

---

## parse_template_str

Parses a prompt template from a raw string. Handles frontmatter extraction, TOML parsing, field validation, and body trimming.

The function has two major branches: with-frontmatter and without-frontmatter. The frontmatter branch has multiple sequential validation steps, each of which can fail independently. The error messages include the source parameter to help the user locate the problem.

### parse_template_str: frontmatter detection

SPEC-PR-PS-001: frontmatter-detected-by-prefix
  If the raw string starts with "+++" (the literal three-character sequence), the parser enters frontmatter mode. The prefix must be at the very start of the string with no leading whitespace.
  test: prompt::tests::parse_template_with_frontmatter

SPEC-PR-PS-002: missing-closing-delimiter-is-error
  When the raw string starts with "+++" but no second "+++" is found in the remainder, parse_template_str returns Err(PromptError) with message "opening +++ without closing +++". This catches truncated or malformed templates.
  test: prompt::tests::parse_template_missing_closing_delimiters

SPEC-PR-PS-003: frontmatter-parsed-as-toml
  The text between the opening and closing `+++` delimiters is trimmed and parsed as a TOML document. If TOML parsing fails, returns Err(PromptError) with "frontmatter parse error" and the TOML parser's error message.
  test: prompt::tests::toml_syntax_error_in_frontmatter

SPEC-PR-PS-004: frontmatter-must-be-toml-table
  The parsed TOML value must be a table (key-value pairs at the top level). If the frontmatter parses as a non-table TOML value (e.g., a bare string or array), returns Err(PromptError) with "frontmatter must be a TOML table".
  test: prompt::tests::non_table_toml_frontmatter

In practice, TOML's top-level parsing nearly always produces a table, so SPEC-PR-PS-004 is defensive. It guards against edge cases where the frontmatter might be a bare value, which the toml crate could theoretically accept.

### parse_template_str: frontmatter fields

SPEC-PR-PS-005: expects-field-required
  The frontmatter must contain an 'expects' field with a string value. If the field is missing or is not a string, returns Err(PromptError) with "missing required 'expects' field".
  test: prompt::tests::parse_template_missing_expects

SPEC-PR-PS-006: expects-must-be-verdict-or-freeform
  The 'expects' field value must be exactly "verdict" or "freeform" (case-sensitive). Any other value returns Err(PromptError) with a message listing the valid options and the invalid value received. Parsing delegates to TemplateExpects::from_str.
  test: prompt::tests::parse_template_invalid_expects

SPEC-PR-PS-007: description-is-optional
  The 'description' field is optional. When present and a string, it is stored in PromptTemplate.description as Some(String). When absent, description is None. If 'description' is present but not a string type, it is treated as absent (None).
  test: prompt::tests::parse_template_with_frontmatter

SPEC-PR-PS-008: freeform-expects-accepted
  When expects is "freeform", the template is parsed successfully with TemplateExpects::Freeform. This variant is used for session-mode LLM validators.
  test: prompt::tests::parse_template_freeform_expects

### parse_template_str: no frontmatter

SPEC-PR-PS-009: no-frontmatter-defaults-to-verdict
  When the raw string does not start with "+++", the entire content is treated as the body. TemplateExpects defaults to Verdict and description is None. No TOML parsing occurs.
  test: prompt::tests::parse_template_without_frontmatter

### parse_template_str: body handling

SPEC-PR-PS-010: body-is-trimmed
  The body text is trimmed of leading and trailing whitespace, both in the frontmatter case (text after the closing "+++") and the no-frontmatter case (entire raw string). This prevents accidental blank lines from affecting prompt delivery.
  test: prompt::tests::parse_template_with_frontmatter

SPEC-PR-PS-011: empty-body-is-error
  If the body is empty after trimming, parse_template_str returns Err(PromptError) with "prompt body is empty". This check runs after frontmatter extraction, so a template with valid frontmatter but no body content is rejected. A template consisting only of whitespace is also rejected.
  test: prompt::tests::parse_template_empty_body

SPEC-PR-PS-012: empty-body-without-frontmatter-is-error
  A raw string that is empty or contains only whitespace (no frontmatter) is also rejected with "prompt body is empty". The empty-body check applies uniformly regardless of whether frontmatter was present.
  test: prompt::tests::empty_string_without_frontmatter
  test: prompt::tests::whitespace_only_without_frontmatter

### parse_template_str: extra frontmatter fields

SPEC-PR-PS-013: unknown-frontmatter-fields-ignored
  Frontmatter fields other than 'expects' and 'description' are silently ignored. The parser does not reject unknown keys. This allows forward compatibility with future frontmatter fields.
  test: prompt::tests::unknown_frontmatter_fields_ignored

---

## TemplateExpects

Display and FromStr implementations for the expected response format enum.

SPEC-PR-TE-001: display-verdict
  TemplateExpects::Verdict displays as "verdict".
  test: prompt::tests::template_expects_verdict_display

SPEC-PR-TE-002: display-freeform
  TemplateExpects::Freeform displays as "freeform".
  test: prompt::tests::template_expects_freeform_display

SPEC-PR-TE-003: fromstr-verdict
  Parsing "verdict" produces TemplateExpects::Verdict.
  test: IMPLICIT via prompt::tests::parse_template_with_frontmatter

SPEC-PR-TE-004: fromstr-freeform
  Parsing "freeform" produces TemplateExpects::Freeform.
  test: IMPLICIT via prompt::tests::parse_template_freeform_expects

SPEC-PR-TE-005: fromstr-invalid-returns-error
  Parsing any string other than "verdict" or "freeform" returns Err with a message listing the valid options. The comparison is case-sensitive: "Verdict", "VERDICT", etc. are rejected.
  test: IMPLICIT via prompt::tests::parse_template_invalid_expects

---

## is_file_reference

Checks whether a prompt value string looks like a file reference based on its extension.

The recognized extensions are: .md, .txt, .prompt, .j2. This is a pure string check with no filesystem access. The function exists to distinguish inline prompt strings (e.g., "Check this code for bugs") from file references (e.g., "spec-compliance.md") in the validator config.

SPEC-PR-FR-001: md-extension-recognized
  A string ending in ".md" is recognized as a file reference.
  test: prompt::tests::is_file_reference_md

SPEC-PR-FR-002: txt-extension-recognized
  A string ending in ".txt" is recognized as a file reference.
  test: prompt::tests::is_file_reference_txt

SPEC-PR-FR-003: prompt-extension-recognized
  A string ending in ".prompt" is recognized as a file reference.
  test: prompt::tests::is_file_reference_prompt

SPEC-PR-FR-004: j2-extension-recognized
  A string ending in ".j2" is recognized as a file reference.
  test: prompt::tests::is_file_reference_j2

SPEC-PR-FR-005: no-extension-not-recognized
  A string without a recognized extension is not a file reference. This includes strings with no extension at all and strings with unrecognized extensions (e.g., ".yaml", ".json").
  test: prompt::tests::is_file_reference_no_extension

SPEC-PR-FR-006: extension-check-is-suffix-match
  The extension check uses ends_with, not path-based extension extraction. This means "readme.md" matches, but so does a string like "use file.md" (a sentence ending in .md). In practice this is not a problem because prompt values in baton.toml are either obvious filenames or multi-word sentences that do not end in recognized extensions.
  test: prompt::tests::is_file_reference_suffix_match_edge_cases

---

## resolve_prompt_value

Resolves a prompt value (from validator config) to a PromptTemplate. This is the main entry point used by the execution engine to obtain the parsed prompt for an LLM validator.

The resolution strategy has two branches: file references are loaded from disk (with a search path), and inline strings are parsed directly as template content.

### resolve_prompt_value: file reference resolution

SPEC-PR-RP-001: prompts-dir-searched-first
  When the prompt value is a file reference, resolve_prompt_value first looks for the file in prompts_dir (joined with the prompt value as a relative path). If found, the file is parsed via parse_template.
  test: prompt::tests::resolve_prompt_file_in_prompts_dir

SPEC-PR-RP-002: config-dir-fallback
  If the file is not found in prompts_dir, resolve_prompt_value tries the prompt value as a path. If the prompt value is a relative path, it is resolved relative to config_dir. If it is an absolute path, it is used directly. If found, the file is parsed via parse_template.
  test: prompt::tests::resolve_prompt_config_dir_fallback

The two-stage search (prompts_dir then config_dir) supports the convention of keeping prompt templates in a dedicated prompts/ directory while allowing escape hatches for templates stored elsewhere.

SPEC-PR-RP-003: file-not-found-error
  If the file is not found in either prompts_dir or config_dir, returns Err(PromptError) with "Prompt file not found" and lists both search paths. This helps the user understand where the system looked.
  test: prompt::tests::resolve_prompt_file_not_found

SPEC-PR-RP-004: file-parse-errors-propagated
  If the file is found but fails to parse (invalid frontmatter, empty body, etc.), the parse error from parse_template is propagated unchanged. The search path is not retried on parse failure.
  test: prompt::tests::resolve_prompt_file_with_invalid_frontmatter

### resolve_prompt_value: inline prompt

SPEC-PR-RP-005: inline-prompt-parsed-as-template
  When the prompt value is not a file reference, it is treated as inline prompt text and parsed via parse_template_str with name "inline" and source "inline". This means inline prompts support frontmatter (though this is unusual in practice).
  test: prompt::tests::resolve_prompt_inline

SPEC-PR-RP-006: inline-prompt-defaults-to-verdict
  An inline prompt without frontmatter defaults to TemplateExpects::Verdict, consistent with parse_template_str's no-frontmatter behavior.
  test: prompt::tests::resolve_prompt_inline

SPEC-PR-RP-007: inline-prompt-name-is-inline
  Inline prompts always have name "inline" regardless of content. This distinguishes them from file-backed templates in logging and error messages.
  test: prompt::tests::resolve_prompt_inline_name_is_inline
