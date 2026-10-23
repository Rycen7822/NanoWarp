use serde::Serialize;
use serde_json::{Value, json};
use strum_macros::{EnumDiscriminants, EnumIter};
use warp_core::features::FeatureFlag;
use warp_core::telemetry::{EnablementState, TelemetryEvent, TelemetryEventDesc};

use crate::server::ids::ServerId;

/// The entry point through which Cloud Mode was entered.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudModeEntryPoint {
    /// User clicked "New Cloud Agent Tab" or similar action to create a dedicated Cloud Mode tab.
    NewTab,
    /// User entered Cloud Mode from an existing local terminal session (e.g., via keyboard shortcut or command).
    LocalSession,
    /// User entered Cloud Mode through the Oz launch modal.
    /// User re-entered Cloud Mode by clicking on an ambient agent entry block.
    EntryBlock,
}

/// Telemetry events for client interactions with cloud agents.
#[derive(Debug, EnumDiscriminants)]
#[strum_discriminants(derive(EnumIter))]
pub enum CloudAgentTelemetryEvent {
    /// User entered Cloud Mode.
    EnteredCloudMode { entry_point: CloudModeEntryPoint },
    /// User opened the environment selector menu.
    EnvironmentSelectorOpened,
    /// User selected an environment from the environment selector.
    EnvironmentSelected {
        /// The ID of the selected environment, if available.
        environment_id: Option<ServerId>,
    },
    /// Ambient agent failed to dispatch or encountered an error during subscription.
    DispatchFailed {
        /// Error message describing the failure.
        error: String,
    },
}

impl TelemetryEvent for CloudAgentTelemetryEvent {
    fn name(&self) -> &'static str {
        CloudAgentTelemetryEventDiscriminants::from(self).name()
    }

    fn payload(&self) -> Option<Value> {
        match self {
            CloudAgentTelemetryEvent::EnteredCloudMode { entry_point } => Some(json!({
                "entry_point": entry_point,
            })),
            CloudAgentTelemetryEvent::EnvironmentSelectorOpened => None,
            CloudAgentTelemetryEvent::EnvironmentSelected { environment_id } => Some(json!({
                "environment_id": environment_id.map(|id| id.to_string()),
            })),
            CloudAgentTelemetryEvent::DispatchFailed { error } => Some(json!({
                "error": error,
            })),
        }
    }

    fn description(&self) -> &'static str {
        CloudAgentTelemetryEventDiscriminants::from(self).description()
    }

    fn enablement_state(&self) -> EnablementState {
        CloudAgentTelemetryEventDiscriminants::from(self).enablement_state()
    }

    fn contains_ugc(&self) -> bool {
        false
    }

    fn event_descs() -> impl Iterator<Item = Box<dyn TelemetryEventDesc>> {
        warp_core::telemetry::enum_events::<Self>()
    }
}

impl TelemetryEventDesc for CloudAgentTelemetryEventDiscriminants {
    fn name(&self) -> &'static str {
        match self {
            Self::EnteredCloudMode => "AmbientAgent.CloudMode.Entered",
            Self::EnvironmentSelectorOpened => "AmbientAgent.CloudMode.EnvironmentSelector.Opened",
            Self::EnvironmentSelected => "AmbientAgent.CloudMode.EnvironmentSelector.Selected",
            Self::DispatchFailed => "AmbientAgent.DispatchFailed",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::EnteredCloudMode => "User entered cloud agent view",
            Self::EnvironmentSelectorOpened => "User opened the environment selector menu",
            Self::EnvironmentSelected => "User selected an environment from the selector",
            Self::DispatchFailed => "Ambient agent failed to dispatch or encountered an error",
        }
    }

    fn enablement_state(&self) -> EnablementState {
        EnablementState::Flag(FeatureFlag::CloudMode)
    }
}

warp_core::register_telemetry_event!(CloudAgentTelemetryEvent);
