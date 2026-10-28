pub mod util;

#[cfg_attr(not(target_family = "wasm"), path = "native.rs")]
#[cfg_attr(target_family = "wasm", path = "wasm.rs")]
mod imp;

#[cfg(feature = "local_fs")]
use std::path::Path;
use std::path::PathBuf;

pub(crate) use imp::load_tab_configs;
pub use imp::{load_launch_configs, load_theme_configs};
use lazy_static::lazy_static;
use warp_core::ui::theme::WarpTheme;
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::launch_configs::launch_config::LaunchConfig;
use crate::tab_configs::{TabConfig, TabConfigError};
use crate::themes::theme::{ThemeKind, WarpThemeConfig};

lazy_static! {
    pub static ref LAUNCH_CONFIG_COMMENT: String = format!(
        "# Warp Launch Configuration
#
#
# Use this to start a certain configuration of windows, tabs, and panes.
# Open the launch configuration palette to access and open any launch configuration.
#
# This file defines your launch configuration.
# More on how to do so here:
# https://docs.warp.dev/terminal/sessions/launch-configurations
#
# All launch configurations are stored under {}.
# Edit them anytime!
#
# You can also add commands that run on-start for your launch configurations like so:
# ---
# name: Example with Command
# windows:
#  - tabs:
#      - layout:
#          cwd: /Users/warp-user/project
#          commands:
#            - exec: code .
",
        warp_core::paths::home_relative_path(&crate::user_config::launch_configs_dir())
    );
}

#[derive(Clone)]
pub enum WarpConfigUpdateEvent {
    Themes,
    LaunchConfigs,
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    TabConfigs,
    /// Emitted when one or more tab config files failed to parse.
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    TabConfigErrors(Vec<TabConfigError>),
    /// The settings file (`settings.toml`) was created, modified, or deleted.
    #[cfg_attr(not(feature = "local_fs"), expect(dead_code))]
    Settings,
    /// One or more settings in `settings.toml` could not be loaded.
    #[cfg_attr(not(feature = "local_fs"), expect(dead_code))]
    SettingsErrors(crate::settings::SettingsFileError),
    /// A previously-errored settings reload succeeded with no errors.
    #[cfg_attr(not(feature = "local_fs"), expect(dead_code))]
    SettingsErrorsCleared,
}

/// Singleton model containing user configurable themes, launch configs and tab configs.
///
/// Emits events when entities are changed, which are detected via filesystem
/// watchers on the user's `data_dir()` (themes, launch configs,
/// tab configs, etc.) and, on platforms where it differs, `config_local_dir()`
/// (`settings.toml`, `keybindings.yaml`, `user_preferences.json`).
#[derive(Default)]
pub struct WarpConfig {
    launch_configs: Vec<LaunchConfig>,
    tab_configs: Vec<TabConfig>,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    tab_config_errors: Vec<TabConfigError>,
    theme_config: WarpThemeConfig,
}

/// Platform-independent parts of WarpConfig.
///
/// Additional platform-dependent functionality can be found in impl blocks
/// in native.rs and wasm.rs.
impl WarpConfig {
    #[cfg(test)]
    pub fn mock(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            theme_config: WarpThemeConfig::new(),
            ..Default::default()
        }
    }

    pub fn launch_configs(&self) -> &Vec<LaunchConfig> {
        &self.launch_configs
    }

    pub fn tab_configs(&self) -> &Vec<TabConfig> {
        &self.tab_configs
    }

    pub fn theme_config(&self) -> &WarpThemeConfig {
        &self.theme_config
    }

    /// Saving the newly created launch configuration to the WarpConfig that we currently
    /// have.
    pub fn append_launch_config(
        &mut self,
        launch_config: &LaunchConfig,
        ctx: &mut ModelContext<Self>,
    ) {
        if !self.launch_configs.contains(launch_config) {
            self.launch_configs.push(launch_config.to_owned());
            ctx.emit(WarpConfigUpdateEvent::LaunchConfigs);
        }
    }

    pub fn update_theme_config(
        &mut self,
        theme_config: WarpThemeConfig,
        ctx: &mut ModelContext<Self>,
    ) {
        self.theme_config = theme_config;
        ctx.emit(WarpConfigUpdateEvent::Themes);
    }

    pub fn add_new_theme_to_config(
        &mut self,
        theme_name: ThemeKind,
        theme: WarpTheme,
        ctx: &mut ModelContext<Self>,
    ) {
        self.theme_config.add_new_theme(theme_name, theme);
        ctx.emit(WarpConfigUpdateEvent::Themes);
    }

    /// Eagerly removes a tab config by its source path and emits a `TabConfigs` event.
    /// (Used after deleting the file on disk so the menu updates immediately
    /// rather than waiting for the filesystem watcher.)
    #[cfg(feature = "local_fs")]
    pub fn remove_tab_config_by_path(&mut self, path: &Path, ctx: &mut ModelContext<Self>) {
        let before = self.tab_configs.len();
        self.tab_configs
            .retain(|c| c.source_path.as_deref() != Some(path));
        if self.tab_configs.len() != before {
            ctx.emit(WarpConfigUpdateEvent::TabConfigs);
        }
    }
}

/// Returns the base directory in which all of the user's data is stored.
fn base_dir() -> PathBuf {
    warp_core::paths::data_dir()
}

/// Returns the path to the directory containing the user's custom themes.
pub fn themes_dir() -> PathBuf {
    warp_core::paths::themes_dir()
}

/// Returns the path to the directory containing the user's launch
/// configurations.
pub fn launch_configs_dir() -> PathBuf {
    base_dir().join("launch_configurations")
}

/// Returns the path to the directory containing the user's tab configs.
pub fn tab_configs_dir() -> PathBuf {
    base_dir().join("tab_configs")
}

/// Returns a path for a new tab config file that does not yet exist in `dir`.
/// Tries `my_tab_config.toml`, then `my_tab_config_1.toml`, `my_tab_config_2.toml`, etc.
#[cfg(feature = "local_fs")]
pub(crate) fn find_unused_tab_config_path(dir: &Path) -> PathBuf {
    find_unused_toml_path(dir, "my_tab_config")
}

/// Returns a `.toml` path in `dir` that does not yet exist.
///
/// Tries `{base_name}.toml`, then `{base_name}_1.toml`, `{base_name}_2.toml`, etc.
#[cfg(feature = "local_fs")]
pub(crate) fn find_unused_toml_path(dir: &Path, base_name: &str) -> PathBuf {
    let base = dir.join(format!("{base_name}.toml"));
    if !base.exists() {
        return base;
    }
    let mut n = 1u32;
    loop {
        let candidate = dir.join(format!("{base_name}_{n}.toml"));
        if !candidate.exists() {
            return candidate;
        }
        n = n.saturating_add(1);
    }
}

impl Entity for WarpConfig {
    type Event = WarpConfigUpdateEvent;
}

impl SingletonEntity for WarpConfig {}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
