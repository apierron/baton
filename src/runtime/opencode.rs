//! OpenCode runtime adapter implementation.
//!
//! Thin wrapper around [`SessionAdapterBase`] for the OpenCode agent runtime.

use crate::error::Result;

use super::session_common::SessionAdapterBase;
use super::{
    CompletionRequest, CompletionResult, HealthResult, RuntimeAdapter, SessionConfig,
    SessionHandle, SessionResult, SessionStatus,
};

// ─── OpenCode adapter ───────────────────────────────────

/// HTTP client adapter for the OpenCode agent runtime.
#[derive(Debug)]
pub struct OpenCodeAdapter {
    pub base: SessionAdapterBase,
}

impl OpenCodeAdapter {
    /// Creates a new adapter from connection parameters.
    pub fn new(
        base_url: String,
        api_key_env: Option<&str>,
        default_model: Option<String>,
        sandbox: bool,
        timeout_seconds: u64,
        max_iterations: u32,
    ) -> Result<Self> {
        Ok(OpenCodeAdapter {
            base: SessionAdapterBase::new(
                base_url,
                api_key_env,
                default_model,
                sandbox,
                timeout_seconds,
                max_iterations,
            )?,
        })
    }
}

impl RuntimeAdapter for OpenCodeAdapter {
    fn health_check(&self) -> Result<HealthResult> {
        self.base.health_check()
    }

    fn create_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        self.base.create_session(config)
    }

    fn poll_status(&self, handle: &SessionHandle) -> Result<SessionStatus> {
        self.base.poll_status(handle)
    }

    fn collect_result(&self, handle: &SessionHandle) -> Result<SessionResult> {
        self.base.collect_result(handle)
    }

    fn cancel(&self, handle: &SessionHandle) -> Result<()> {
        self.base.cancel(handle)
    }

    fn teardown(&self, handle: &SessionHandle) -> Result<()> {
        self.base.teardown(handle)
    }

    fn post_completion(&self, request: CompletionRequest) -> Result<CompletionResult> {
        self.base.post_completion(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::runtime::session_common::session_adapter_tests!(
        OpenCodeAdapter,
        "OPENCODE",
        |server: &httpmock::MockServer| {
            OpenCodeAdapter {
                base: SessionAdapterBase {
                    base_url: server.url(""),
                    api_key: None,
                    default_model: Some("test-model".into()),
                    sandbox: false,
                    timeout_seconds: 30,
                    max_iterations: 10,
                    client: reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(10))
                        .build()
                        .unwrap(),
                },
            }
        }
    );
}
