use std::fmt::Display;

use regex::Regex;
use serde::{Deserialize, Serialize};
use settings::macros::{define_settings_group, maybe_define_setting, register_settings_events};
use settings::{Setting, SupportedPlatforms};
use warp_errors::report_error;
pub use warp_terminal::model::secrets::RegexDisplayInfo;
use warpui::{AppContext, Entity, ModelContext, SingletonEntity, UpdateModel};

use crate::terminal::safe_mode_settings::SafeModeSettings;
use crate::workspaces::workspace::EnterpriseSecretRegex;

pub const TELEMETRY_ENABLED_DEFAULTS_KEY: &str = "TelemetryEnabled";

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(description = "A custom regex pattern for detecting and redacting secrets.")]
pub struct CustomSecretRegex {
    #[serde(with = "serde_regex")]
    #[schemars(with = "String", description = "The regex pattern to match secrets.")]
    pub pattern: Regex,
    #[serde(default)]
    #[schemars(description = "Optional display name for this secret pattern.")]
    pub name: Option<String>,
}

impl CustomSecretRegex {
    pub fn pattern(&self) -> &Regex {
        &self.pattern
    }
}

impl RegexDisplayInfo for CustomSecretRegex {
    fn pattern(&self) -> &str {
        self.pattern.as_str()
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl RegexDisplayInfo for EnterpriseSecretRegex {
    fn pattern(&self) -> &str {
        &self.pattern
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl Display for CustomSecretRegex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.pattern.as_str())
    }
}

impl PartialEq for CustomSecretRegex {
    /// We do not factor in the name to equality checks --
    /// if the regex is the same, then the regex is the same.
    /// This allows us to avoid adding duplicate regexes.
    fn eq(&self, other: &Self) -> bool {
        self.pattern.as_str() == other.pattern.as_str()
    }
}

impl settings_value::SettingsValue for CustomSecretRegex {}

define_settings_group!(WarpDrivePrivacySettings, settings: [
    is_telemetry_enabled: IsTelemetryEnabled {
        type: bool,
        default: true,
        supported_platforms: SupportedPlatforms::ALL,
        private: false,
        storage_key: "TelemetryEnabled",
        toml_path: "privacy.telemetry_enabled",
        description: "Whether anonymous usage telemetry is collected.",
    },
]);

maybe_define_setting!(CustomSecretRegexList, group: PrivacySettings, {
    type: Vec<CustomSecretRegex>,
    default: Vec::new(),
    supported_platforms: SupportedPlatforms::ALL,
    private: false,
    toml_path: "privacy.custom_secret_regex_list",
    description: "Custom regex patterns for detecting and redacting secrets.",
});

/// Singleton model for managing the user's privacy settings.
pub struct PrivacySettings {
    pub is_telemetry_enabled: bool,
    /// List of user defined secret regexes.
    /// Enterprise-level secret regexes will always take precedence over user-level secrets,
    /// but they both used to support additive behavior.
    /// It's a [Vec<CustomSecretRegex>], but also a user setting.
    pub user_secret_regex_list: CustomSecretRegexList,
    /// List of enterprise-level secret regexes provided by the organization.
    /// These are kept separate from user-level secrets to support additive behavior.
    pub enterprise_secret_regex_list: Vec<CustomSecretRegex>,
    /// Whether or not the user's organization has forced telemetry on, in which case we ignore any
    /// user local settings. If false, we fall back to the user's settings.
    /// This is populated by the server when teams data is fetched.
    pub is_telemetry_force_enabled: bool,
    /// Whether or not the user's organization has enabled enterprise secret redaction.
    /// This is populated by the server when teams data is fetched.
    pub is_enterprise_secret_redaction_enabled: bool,
}

/// A snapshot of a user's [`PrivacySettings`] settings at some point in time.
#[derive(Clone, Copy)]
pub struct PrivacySettingsSnapshot {
    is_telemetry_enabled: bool,
    is_telemetry_force_enabled: bool,
}

impl PrivacySettingsSnapshot {
    pub fn is_telemetry_enabled(&self) -> bool {
        self.is_telemetry_enabled
    }

    pub fn is_telemetry_force_enabled(&self) -> bool {
        self.is_telemetry_force_enabled
    }

    pub fn should_disable_telemetry(&self) -> bool {
        !self.is_telemetry_enabled && !self.is_telemetry_force_enabled
    }

    #[cfg(test)]
    pub fn mock() -> Self {
        Self {
            is_telemetry_enabled: true,
            is_telemetry_force_enabled: true,
        }
    }
}

impl PrivacySettings {
    /// Registers a singleton PrivacySettings model on `app`.
    ///
    /// We expose this function publicly (while keeping the constructor private) to prevent
    /// instantiation another PrivacySettings struct, in the case where a developer might be
    /// unaware that it is registered as a singleton model.
    pub fn register_singleton(ctx: &mut AppContext) {
        let handle = ctx.add_singleton_model(PrivacySettings::new);

        register_settings_events!(
            PrivacySettings,
            user_secret_regex_list,
            CustomSecretRegexList,
            handle,
            ctx
        );
    }

    /// Returns a new PrivacySettings object initialized from locally cached values.
    fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Initialize from `WarpDrivePrivacySettings`, which is the source of truth for these
        // booleans.
        let warp_drive_privacy = WarpDrivePrivacySettings::as_ref(ctx);
        let is_telemetry_enabled = *warp_drive_privacy.is_telemetry_enabled.value();
        // Listen for changes to the local privacy settings.
        ctx.subscribe_to_model(
            &WarpDrivePrivacySettings::handle(ctx),
            |me, _, event, ctx| {
                let privacy_settings = WarpDrivePrivacySettings::as_ref(ctx);
                match event {
                    WarpDrivePrivacySettingsChangedEvent::IsTelemetryEnabled { .. } => {
                        me.set_is_telemetry_enabled(
                            *privacy_settings.is_telemetry_enabled.value(),
                            ctx,
                        );
                    }
                }
            },
        );

        let user_secret_regex_list: CustomSecretRegexList =
            CustomSecretRegexList::new_from_storage(ctx);

        Self {
            is_telemetry_enabled,
            user_secret_regex_list,
            is_telemetry_force_enabled: false,
            is_enterprise_secret_redaction_enabled: false,
            enterprise_secret_regex_list: Vec::new(),
        }
    }

    pub fn is_telemetry_force_enabled(&self) -> bool {
        self.is_telemetry_force_enabled
    }

    pub fn set_is_telemetry_force_enabled(&mut self, is_telemetry_force_enabled: bool) {
        self.is_telemetry_force_enabled = is_telemetry_force_enabled;
    }

    pub fn is_enterprise_secret_redaction_enabled(&self) -> bool {
        self.is_enterprise_secret_redaction_enabled
    }

    pub fn set_enterprise_secret_redaction_settings(
        &mut self,
        enabled: bool,
        enterprise_regexes: Vec<EnterpriseSecretRegex>,
        change_event_reason: ChangeEventReason,
        ctx: &mut ModelContext<Self>,
    ) {
        if enabled {
            // First time: Force enable secret redaction setting (safe mode).
            if !self.is_enterprise_secret_redaction_enabled {
                let safe_mode_settings = SafeModeSettings::handle(ctx);
                ctx.update_model(&safe_mode_settings, |safe_mode_settings, ctx| {
                    let _ = safe_mode_settings.safe_mode_enabled.set_value(true, ctx);
                });
            }

            // Convert EnterpriseSecretRegex to CustomSecretRegex for internal use
            let mut enterprise_secrets = Vec::new();
            for enterprise_regex in enterprise_regexes {
                match Regex::new(&enterprise_regex.pattern) {
                    Ok(regex) => {
                        enterprise_secrets.push(CustomSecretRegex {
                            pattern: regex,
                            name: enterprise_regex.name,
                        });
                    }
                    _ => {
                        report_error!(
                            "Invalid enterprise secret regex pattern",
                            extra: { "pattern" => %enterprise_regex.pattern }
                        );
                    }
                }
            }
            self.enterprise_secret_regex_list = enterprise_secrets;
        } else {
            // Clear enterprise secrets when disabled
            self.enterprise_secret_regex_list.clear();
        }

        self.is_enterprise_secret_redaction_enabled = enabled;

        ctx.emit(PrivacySettingsChangedEvent::CustomSecretRegexList {
            change_event_reason,
        });
        ctx.notify();
    }

    pub fn refresh_to_default(&mut self) {
        // TODO(zach): this seems incorrect - should we also update the values on disk?
        self.is_telemetry_enabled = true;
        self.is_telemetry_force_enabled = false;
        self.is_enterprise_secret_redaction_enabled = false;
    }

    /// Constructor for tests only.
    #[cfg(any(test, feature = "test-util"))]
    pub fn mock(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            is_telemetry_enabled: true,
            user_secret_regex_list: CustomSecretRegexList::new(None),
            is_telemetry_force_enabled: false,
            is_enterprise_secret_redaction_enabled: false,
            enterprise_secret_regex_list: Vec::new(),
        }
    }

    /// Returns a snapshot of the user's privacy settings.
    ///
    /// The returned snapshot is not stateful, thus its values should be used shortly after the
    /// snapshot is returned.
    pub fn get_snapshot(&self, _app: &AppContext) -> PrivacySettingsSnapshot {
        PrivacySettingsSnapshot {
            is_telemetry_enabled: self.is_telemetry_enabled,
            is_telemetry_force_enabled: self.is_telemetry_force_enabled,
        }
    }

    /// Sets `is_telemetry_enabled` to the given value.
    ///
    /// Additionally, this writes the given value to the user's local defaults.
    /// Finally, emits a `PrivacySettingsEvent::UpdateIsTelemetryEnabled` event.
    pub fn set_is_telemetry_enabled(
        &mut self,
        new_value: bool,
        ctx: &mut ModelContext<PrivacySettings>,
    ) {
        let old_value = self.is_telemetry_enabled;
        if new_value != old_value {
            self.is_telemetry_enabled = new_value;

            WarpDrivePrivacySettings::handle(ctx).update(ctx, |settings, ctx| {
                log::info!("Setting is_telemetry_enabled to {new_value}");
                let _ = settings.is_telemetry_enabled.set_value(new_value, ctx);
            });

            ctx.emit(PrivacySettingsChangedEvent::UpdateIsTelemetryEnabled {
                old_value,
                new_value,
            });
            ctx.notify();
        }
    }

    pub fn remove_user_secret_regex(&mut self, idx: &usize, ctx: &mut ModelContext<Self>) {
        let mut new_user_secret_regex_list = self.user_secret_regex_list.to_vec();
        new_user_secret_regex_list.remove(*idx);
        if self
            .user_secret_regex_list
            .set_value(new_user_secret_regex_list, ctx)
            .is_err()
        {
            report_error!("Custom Secret Regex List failed to serialize")
        }
    }

    /// Initializes the custom secret regex list with the default regexes, provided
    /// non matches can be found.
    /// This can be called when a user first enables secret redaction.
    pub fn add_all_recommended_regex(&mut self, ctx: &mut ModelContext<Self>) {
        let mut new_user_secret_regex_list = self.user_secret_regex_list.to_vec();
        let num_existing_regexes = new_user_secret_regex_list.len();

        // Add all the default regexes if they don't already exist
        for default_regex in crate::terminal::model::secrets::regexes::DEFAULT_REGEXES_WITH_NAMES {
            match Regex::new(default_regex.pattern) {
                Ok(regex) => {
                    let custom_regex = CustomSecretRegex {
                        pattern: regex,
                        name: Some(default_regex.name.to_string()),
                    };
                    if !new_user_secret_regex_list.contains(&custom_regex) {
                        new_user_secret_regex_list.push(custom_regex);
                    }
                }
                _ => {
                    report_error!(
                        "Failed to compile default regex",
                        extra: { "pattern" => %default_regex.pattern }
                    );
                }
            }
        }

        if num_existing_regexes == new_user_secret_regex_list.len() {
            return;
        }

        if self
            .user_secret_regex_list
            .set_value(new_user_secret_regex_list, ctx)
            .is_err()
        {
            report_error!("Failed to serialize default regexes to custom secret regex list")
        }

        ctx.notify();
    }
}

/// Events emitted when PrivacySettings is updated.
#[derive(Clone, Copy)]
pub enum PrivacySettingsChangedEvent {
    UpdateIsTelemetryEnabled {
        old_value: bool,
        new_value: bool,
    },
    CustomSecretRegexList {
        change_event_reason: ChangeEventReason,
    },
}

impl Entity for PrivacySettings {
    type Event = PrivacySettingsChangedEvent;
}

impl SingletonEntity for PrivacySettings {}
