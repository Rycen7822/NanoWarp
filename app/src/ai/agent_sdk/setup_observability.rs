use enum_iterator::Sequence;
use futures::FutureExt as _;
use std::future::Future;
use std::sync::Arc;
use tracing::Instrument as _;
use warpui::r#async::executor::Background;

/// Traces GUI environment setup and background codebase indexing.
#[derive(Clone)]
pub(crate) struct SetupTracer {
    background: Arc<Background>,
}

impl SetupTracer {
    pub(crate) fn new(background: Arc<Background>) -> Self {
        Self { background }
    }

    pub(crate) async fn record_result<T, E: std::error::Error>(
        &self,
        step: SetupStep,
        future: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        future
            .map(|result| result.inspect_err(|err| tracing::error!(error = %err)))
            .instrument(step.span())
            .await
    }

    pub(crate) fn record_value_detached<T>(
        &self,
        step: SetupStep,
        future: impl Future<Output = T> + Send + 'static,
    ) where
        T: Send + 'static,
    {
        self.background
            .spawn(async move {
                future.instrument(step.span()).await;
            })
            .detach();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Sequence)]
pub(crate) enum SetupStep {
    EnvironmentRepoClone,
    CacheSetup,
    EnvironmentSetupCommands,
    EnvironmentCodebaseIndexing,
}

impl SetupStep {
    fn span(self) -> tracing::Span {
        log::debug!("Starting setup step: {self:?}");
        match self {
            Self::EnvironmentRepoClone => {
                tracing::info_span!("setup_environment_repo_clone", tags.cloud_agent = true)
            }
            Self::CacheSetup => tracing::info_span!("setup_caches", tags.cloud_agent = true),
            Self::EnvironmentSetupCommands => {
                tracing::info_span!("setup_environment_setup_commands", tags.cloud_agent = true)
            }
            Self::EnvironmentCodebaseIndexing => tracing::info_span!(
                "setup_environment_codebase_indexing",
                tags.cloud_agent = true
            ),
        }
    }
}

#[cfg(test)]
#[path = "setup_observability_tests.rs"]
mod tests;
