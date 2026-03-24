#![doc = include_str!("../README.md")]
#![doc(html_root_url = "https://apierron.github.io/baton/baton/")]

pub mod commands;
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
