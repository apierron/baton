//! Initialize a new baton project with config and starter prompts.
//!
//! Creates `baton.toml`, the `.baton/` directory structure, and optionally
//! starter prompt templates in `prompts/`. Supports interactive, flag-driven,
//! and non-TTY (defaults) modes.

// Config templates embedded from defaults/ at compile time.
const DEFAULT_BASE_CONFIG: &str = include_str!("../../defaults/configs/base.toml");
const DEFAULT_GENERIC_CONFIG: &str = include_str!("../../defaults/configs/generic.toml");
const DEFAULT_RUST_CONFIG: &str = include_str!("../../defaults/configs/rust.toml");
const DEFAULT_PYTHON_CONFIG: &str = include_str!("../../defaults/configs/python.toml");

const DEFAULT_PROMPT_SPEC: &str = include_str!("../../defaults/prompts/spec-compliance.md");
const DEFAULT_PROMPT_ADVERSARIAL: &str =
    include_str!("../../defaults/prompts/adversarial-review.md");
const DEFAULT_PROMPT_DOC: &str = include_str!("../../defaults/prompts/doc-completeness.md");

const VALID_PROFILES: &[&str] = &["generic", "rust", "python"];

/// Choices resolved from flags or interactive prompts.
struct InitChoices {
    include_code_validators: bool,
    profile: String,
    include_prompts: bool,
}

/// Run interactive prompts to determine init choices.
fn prompt_init_choices() -> std::result::Result<InitChoices, String> {
    use dialoguer::{Confirm, Select};

    let include_code = Confirm::new()
        .with_prompt("Include code validators?")
        .default(true)
        .interact()
        .map_err(|e| format!("Interactive prompt failed: {e}"))?;

    let profile = if include_code {
        let options = &["Rust", "Python", "Generic"];
        let idx = Select::new()
            .with_prompt("Which language?")
            .items(options)
            .default(0)
            .interact()
            .map_err(|e| format!("Interactive prompt failed: {e}"))?;
        match idx {
            0 => "rust",
            1 => "python",
            _ => "generic",
        }
        .to_string()
    } else {
        String::new()
    };

    let include_prompts = Confirm::new()
        .with_prompt("Include starter prompt templates?")
        .default(true)
        .interact()
        .map_err(|e| format!("Interactive prompt failed: {e}"))?;

    Ok(InitChoices {
        include_code_validators: include_code,
        profile,
        include_prompts,
    })
}

/// Initializes a new baton project: creates `baton.toml`, `.baton/` directory,
/// and optionally starter prompt templates in `prompts/`.
pub fn cmd_init(minimal: bool, prompts_only: bool, profile: Option<&str>) -> i32 {
    // Validate explicit profile if provided
    if let Some(p) = profile {
        if !VALID_PROFILES.contains(&p) {
            eprintln!(
                "Error: unknown profile \"{p}\". Valid profiles: {}",
                VALID_PROFILES.join(", ")
            );
            return 1;
        }
    }

    // Resolve choices: explicit flags, interactive, or defaults
    let choices = if profile.is_some() || minimal || prompts_only {
        // Explicit flags — use them directly
        InitChoices {
            include_code_validators: true,
            profile: profile.unwrap_or("generic").to_string(),
            include_prompts: !minimal,
        }
    } else if atty::is(atty::Stream::Stdin) {
        // Interactive mode
        match prompt_init_choices() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Error: {e}");
                return 1;
            }
        }
    } else {
        // Non-TTY, no flags — use defaults
        InitChoices {
            include_code_validators: true,
            profile: "generic".to_string(),
            include_prompts: true,
        }
    };

    if !prompts_only {
        // Check if baton.toml already exists
        if std::path::Path::new("baton.toml").exists() {
            eprintln!("Error: baton.toml already exists. Will not overwrite.");
            return 1;
        }

        // Create .baton directory structure
        let baton_dir = std::path::Path::new(".baton");
        if baton_dir.exists() {
            eprintln!("Warning: .baton/ already exists. Creating missing subdirectories.");
        }
        for subdir in &["logs", "tmp"] {
            let dir = baton_dir.join(subdir);
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("Error creating {}: {e}", dir.display());
                return 1;
            }
        }

        // Build config content
        let starter_config = if choices.include_code_validators {
            let profile_config = match choices.profile.as_str() {
                "rust" => DEFAULT_RUST_CONFIG,
                "python" => DEFAULT_PYTHON_CONFIG,
                _ => DEFAULT_GENERIC_CONFIG,
            };
            format!("{}\n{}", DEFAULT_BASE_CONFIG, profile_config)
        } else {
            DEFAULT_BASE_CONFIG.to_string()
        };

        if let Err(e) = std::fs::write("baton.toml", starter_config) {
            eprintln!("Error writing baton.toml: {e}");
            return 1;
        }
        if choices.include_code_validators {
            eprintln!("Created baton.toml (profile: {})", choices.profile);
        } else {
            eprintln!("Created baton.toml (base only, no validators)");
        }
        eprintln!("Created .baton/");
    }

    if choices.include_prompts || prompts_only {
        // Create prompts directory with starter templates
        let prompts_dir = std::path::Path::new("prompts");
        if let Err(e) = std::fs::create_dir_all(prompts_dir) {
            eprintln!("Error creating prompts/: {e}");
            return 1;
        }

        let templates = [
            ("spec-compliance.md", DEFAULT_PROMPT_SPEC),
            ("adversarial-review.md", DEFAULT_PROMPT_ADVERSARIAL),
            ("doc-completeness.md", DEFAULT_PROMPT_DOC),
        ];

        for (name, content) in &templates {
            let path = prompts_dir.join(name);
            if !path.exists() {
                if let Err(e) = std::fs::write(&path, content) {
                    eprintln!("Error writing {}: {e}", path.display());
                    return 1;
                }
                eprintln!("Created prompts/{name}");
            }
        }
    }

    eprintln!("baton project initialized.");
    0
}
