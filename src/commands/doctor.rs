//! Comprehensive health check for baton installation and project.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::config::{validate_config, BatonConfig, ValidatorType};
use crate::prompt;

use super::{detect_install_method, load_config};

// ─── Types ───────────────────────────────────────────────

enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

struct CheckResult {
    status: CheckStatus,
    message: String,
}

impl CheckResult {
    fn ok(msg: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Ok,
            message: msg.into(),
        }
    }
    fn warn(msg: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Warn,
            message: msg.into(),
        }
    }
    fn fail(msg: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Fail,
            message: msg.into(),
        }
    }
    fn skip(msg: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Skip,
            message: msg.into(),
        }
    }
}

struct DoctorSummary {
    ok: usize,
    warn: usize,
    fail: usize,
}

impl DoctorSummary {
    fn new() -> Self {
        Self {
            ok: 0,
            warn: 0,
            fail: 0,
        }
    }

    fn record(&mut self, results: &[CheckResult]) {
        for r in results {
            match r.status {
                CheckStatus::Ok => self.ok += 1,
                CheckStatus::Warn => self.warn += 1,
                CheckStatus::Fail => self.fail += 1,
                CheckStatus::Skip => {}
            }
        }
    }

    fn print(&self) {
        eprintln!();
        eprintln!(
            "Summary: {} ok, {} warn, {} fail",
            self.ok, self.warn, self.fail
        );
    }
}

// ─── Output helpers ──────────────────────────────────────

fn print_section(num: usize, total: usize, name: &str) {
    eprintln!();
    eprintln!("[{num}/{total}] {name}");
}

fn print_results(results: &[CheckResult]) {
    for r in results {
        let tag = match r.status {
            CheckStatus::Ok => "[ok]  ",
            CheckStatus::Warn => "[warn]",
            CheckStatus::Fail => "[fail]",
            CheckStatus::Skip => "[skip]",
        };
        eprintln!("  {tag} {}", r.message);
    }
}

fn record_and_print(summary: &mut DoctorSummary, results: Vec<CheckResult>) {
    summary.record(&results);
    print_results(&results);
}

// ─── Entry point ─────────────────────────────────────────

/// Runs a comprehensive health check across installation, config, project
/// structure, prompt templates, environment variables, and runtimes.
pub fn cmd_doctor(config_path: Option<&PathBuf>, offline: bool) -> i32 {
    let mut summary = DoctorSummary::new();
    let total = 6;

    // Section 1: Installation
    print_section(1, total, "Installation");
    record_and_print(&mut summary, check_installation());

    // Section 2: Configuration
    print_section(2, total, "Configuration");
    let (config_opt, results) = check_configuration(config_path);
    record_and_print(&mut summary, results);

    // Sections 3-6 require config
    match config_opt {
        Some((config, _path)) => {
            print_section(3, total, "Project Structure");
            record_and_print(&mut summary, check_project_structure(&config));

            print_section(4, total, "Prompt Templates");
            record_and_print(&mut summary, check_prompt_templates(&config));

            print_section(5, total, "Environment");
            record_and_print(&mut summary, check_environment(&config));

            print_section(6, total, "Runtimes");
            record_and_print(
                &mut summary,
                if offline {
                    skip_runtimes(&config)
                } else {
                    check_runtimes(&config)
                },
            );
        }
        None => {
            for (i, name) in [
                (3, "Project Structure"),
                (4, "Prompt Templates"),
                (5, "Environment"),
                (6, "Runtimes"),
            ] {
                print_section(i, total, name);
                record_and_print(
                    &mut summary,
                    vec![CheckResult::skip("Requires valid configuration")],
                );
            }
        }
    }

    summary.print();

    if summary.fail > 0 {
        1
    } else {
        0
    }
}

// ─── Section 1: Installation ─────────────────────────────

fn check_installation() -> Vec<CheckResult> {
    let version = env!("CARGO_PKG_VERSION");
    let (method, exe_path) = detect_install_method();
    vec![CheckResult::ok(format!(
        "baton {version} ({method}, {})",
        exe_path.display()
    ))]
}

// ─── Section 2: Configuration ────────────────────────────

fn check_configuration(
    config_path: Option<&PathBuf>,
) -> (Option<(BatonConfig, PathBuf)>, Vec<CheckResult>) {
    let (config, config_file) = match load_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            return (None, vec![CheckResult::fail(format!("{e}"))]);
        }
    };

    let mut results = vec![CheckResult::ok(format!(
        "baton.toml found: {}",
        config_file.display()
    ))];

    let validation = validate_config(&config);
    if validation.errors.is_empty() && validation.warnings.is_empty() {
        results.push(CheckResult::ok("Config validates (0 errors, 0 warnings)"));
    } else {
        for w in &validation.warnings {
            results.push(CheckResult::warn(format!("Config warning: {w}")));
        }
        for e in &validation.errors {
            results.push(CheckResult::fail(format!("Config error: {e}")));
        }
    }

    (Some((config, config_file)), results)
}

// ─── Section 3: Project Structure ────────────────────────

fn check_project_structure(config: &BatonConfig) -> Vec<CheckResult> {
    let mut results = Vec::new();

    // Check directories
    for (name, path) in [
        ("prompts_dir", &config.defaults.prompts_dir),
        ("log_dir", &config.defaults.log_dir),
        ("tmp_dir", &config.defaults.tmp_dir),
    ] {
        if path.is_dir() {
            results.push(CheckResult::ok(format!(
                "{name}: {} (exists)",
                path.display()
            )));
        } else {
            results.push(CheckResult::fail(format!(
                "{name}: {} (missing or not a directory)",
                path.display()
            )));
        }
    }

    // Check history_db
    let db_path = &config.defaults.history_db;
    if db_path.exists() {
        results.push(CheckResult::ok(format!(
            "history_db: {} (exists)",
            db_path.display()
        )));
    } else if let Some(parent) = db_path.parent() {
        if parent.is_dir() {
            // Check if parent is writable by attempting to check metadata
            match std::fs::metadata(parent) {
                Ok(meta) => {
                    if !meta.permissions().readonly() {
                        results.push(CheckResult::warn(format!(
                            "history_db: {} (will be created on first run)",
                            db_path.display()
                        )));
                    } else {
                        results.push(CheckResult::fail(format!(
                            "history_db: {} (parent directory is read-only)",
                            db_path.display()
                        )));
                    }
                }
                Err(e) => {
                    results.push(CheckResult::fail(format!(
                        "history_db: {} (cannot check parent: {e})",
                        db_path.display()
                    )));
                }
            }
        } else {
            results.push(CheckResult::fail(format!(
                "history_db: {} (parent directory missing)",
                db_path.display()
            )));
        }
    } else {
        results.push(CheckResult::fail(format!(
            "history_db: {} (invalid path)",
            db_path.display()
        )));
    }

    results
}

// ─── Section 4: Prompt Templates ─────────────────────────

fn check_prompt_templates(config: &BatonConfig) -> Vec<CheckResult> {
    let mut results = Vec::new();
    let mut checked: BTreeSet<String> = BTreeSet::new();

    for gate in config.gates.values() {
        for validator in &gate.validators {
            if validator.validator_type != ValidatorType::Llm {
                continue;
            }
            let prompt_value = match &validator.prompt {
                Some(p) => p,
                None => continue,
            };
            if !prompt::is_file_reference(prompt_value) {
                continue;
            }
            if !checked.insert(prompt_value.clone()) {
                continue; // Already checked this prompt file
            }

            match prompt::resolve_prompt_value(
                prompt_value,
                &config.defaults.prompts_dir,
                &config.config_dir,
            ) {
                Ok(template) => {
                    results.push(CheckResult::ok(format!(
                        "{prompt_value} (resolves, expects: {})",
                        template.expects
                    )));
                }
                Err(e) => {
                    results.push(CheckResult::fail(format!("{prompt_value}: {e}")));
                }
            }
        }
    }

    if results.is_empty() {
        results.push(CheckResult::ok("No prompt file references to check"));
    }

    results
}

// ─── Section 5: Environment ──────────────────────────────

fn check_environment(config: &BatonConfig) -> Vec<CheckResult> {
    let mut results = Vec::new();

    for (rname, runtime) in &config.runtimes {
        let env_var = match &runtime.api_key_env {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        match std::env::var(env_var) {
            Ok(val) if !val.is_empty() => {
                results.push(CheckResult::ok(format!(
                    "{env_var} (set, runtime '{rname}')"
                )));
            }
            _ => {
                results.push(CheckResult::fail(format!(
                    "{env_var} (not set, runtime '{rname}')"
                )));
            }
        }
    }

    if results.is_empty() {
        results.push(CheckResult::ok("No environment variables to check"));
    }

    results
}

// ─── Section 6: Runtimes ─────────────────────────────────

fn check_runtimes(config: &BatonConfig) -> Vec<CheckResult> {
    if config.runtimes.is_empty() {
        return vec![CheckResult::ok("No runtimes configured")];
    }

    let mut results = Vec::new();
    for (rname, runtime_config) in &config.runtimes {
        let adapter = match crate::runtime::create_adapter(rname, runtime_config) {
            Ok(a) => a,
            Err(e) => {
                results.push(CheckResult::fail(format!("{rname}: {e}")));
                continue;
            }
        };

        match adapter.health_check() {
            Ok(health) if health.reachable => {
                let version_info = health
                    .version
                    .as_ref()
                    .map(|v| format!(", version {v}"))
                    .unwrap_or_default();
                results.push(CheckResult::ok(format!(
                    "{rname}: reachable ({}{version_info})",
                    runtime_config.runtime_type
                )));
            }
            Ok(health) => {
                let msg = health.message.as_deref().unwrap_or("unknown error");
                results.push(CheckResult::fail(format!("{rname}: not reachable ({msg})")));
            }
            Err(e) => {
                results.push(CheckResult::fail(format!("{rname}: {e}")));
            }
        }
    }

    results
}

fn skip_runtimes(config: &BatonConfig) -> Vec<CheckResult> {
    if config.runtimes.is_empty() {
        return vec![CheckResult::ok("No runtimes configured")];
    }

    config
        .runtimes
        .keys()
        .map(|name| CheckResult::skip(format!("{name}: Skipped (--offline)")))
        .collect()
}
