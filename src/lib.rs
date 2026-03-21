//! Baton: a composable validation gate for AI agent outputs.
//!
//! Accepts input files to validate, runs a pipeline of validators (script,
//! LLM, or human), produces a structured verdict (pass/fail/error), and
//! persists results in SQLite.
//!
//! # Modules
//!
//! - [`config`] — TOML configuration parsing and validation
//! - [`exec`] — Gate execution engine and validator dispatch
//! - [`types`] — Core data types (InputFile, Invocation, Verdict, Status)
//! - [`history`] — SQLite-based verdict persistence
//! - [`prompt`] — Prompt template parsing with frontmatter support
//! - [`placeholder`] — Template variable resolution
//! - [`verdict_parser`] — Verdict extraction from LLM/agent text output
//! - [`provider`] — HTTP client for OpenAI-compatible LLM provider APIs
//! - [`runtime`] — Runtime adapter abstraction for agent-based validators
//! - [`error`] — Error types

pub mod add;
pub mod config;
pub mod error;
pub mod exec;
pub mod history;
pub mod placeholder;
pub mod prompt;
pub mod provider;
pub mod runtime;
pub mod types;
pub mod verdict_parser;

#[cfg(test)]
pub mod test_helpers;
