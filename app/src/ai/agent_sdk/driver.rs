use std::time::Duration;

use warp_errors::{ErrorExt, register_error};

#[cfg(feature = "local_fs")]
pub(crate) mod cache_setup;
pub(crate) mod environment;
#[cfg(feature = "local_fs")]
pub(crate) mod environment_checkout;
mod environment_checkout_protocol;
mod failure_output;
pub(crate) mod harness;
mod snapshot;
pub(crate) mod terminal;

pub(crate) use snapshot::upload_snapshot_for_handoff;

pub(crate) const WARP_DRIVE_SYNC_TIMEOUT: Duration = Duration::from_secs(60);
/// Signals to Claude child-harness hooks that Warp already owns the background
/// message-listener lifecycle, so the plugin should reuse the shared state
/// files instead of spawning and cleaning up its own listener.
///
/// When this variable is absent, the Claude plugin falls back to its legacy
/// self-managed listener path so older Warp builds and standalone plugin
/// invocations keep working.
pub(crate) const OZ_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV: &str =
    "OZ_MESSAGE_LISTENER_MANAGED_EXTERNALLY";
/// Warp-branded name for the same signal, injected alongside the `OZ_` one with the same value.
pub(crate) const WARP_MESSAGE_LISTENER_MANAGED_EXTERNALLY_ENV: &str =
    "WARP_MESSAGE_LISTENER_MANAGED_EXTERNALLY";
/// Optional root directory for the per-session Claude message-listener state
/// that Warp and the Claude hook scripts share.
pub(crate) const OZ_MESSAGE_LISTENER_STATE_ROOT_ENV: &str = "OZ_MESSAGE_LISTENER_STATE_ROOT";
/// Warp-branded name for the same state root, injected alongside the `OZ_` one.
pub(crate) const WARP_MESSAGE_LISTENER_STATE_ROOT_ENV: &str = "WARP_MESSAGE_LISTENER_STATE_ROOT";
// Keep exporting the legacy `OZ_PARENT_*` names to child hooks until the
// external Claude plugin has fully migrated to the canonical
// `OZ_MESSAGE_LISTENER_*` names.
const LEGACY_OZ_PARENT_LISTENER_MANAGED_EXTERNALLY_ENV: &str =
    "OZ_PARENT_LISTENER_MANAGED_EXTERNALLY";
const LEGACY_OZ_PARENT_STATE_ROOT_ENV: &str = "OZ_PARENT_STATE_ROOT";

#[derive(Debug, thiserror::Error)]
pub enum AgentDriverError {
    #[error("Invalid runtime state - please file a bug report.")]
    InvalidRuntimeState,
    #[error(
        "The shell exited during setup command `{command}`, so the run could not continue. Check the setup commands for this environment."
    )]
    SetupCommandExitedShell { command: String },
    #[error("Harness '{harness}' setup failed: {reason}")]
    HarnessSetupFailed { harness: String, reason: String },
}

impl ErrorExt for AgentDriverError {
    fn is_actionable(&self) -> bool {
        matches!(self, Self::InvalidRuntimeState)
    }
}
register_error!(AgentDriverError);

impl From<warpui::ModelDropped> for AgentDriverError {
    fn from(_: warpui::ModelDropped) -> Self {
        AgentDriverError::InvalidRuntimeState
    }
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
