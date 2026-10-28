pub(crate) mod action_sidecar;
pub mod branch_picker;
pub mod params_modal;
pub(crate) mod remove_confirmation_dialog;
pub mod repo_picker;
pub mod session_config;
pub mod session_config_modal;
pub mod session_config_rendering;
pub mod tab_config;

pub use params_modal::{TabConfigParamsModal, TabConfigParamsModalEvent};
pub use tab_config::{
    TabConfig, TabConfigError, TabConfigParam, TabConfigParamType, render_tab_config,
};
use warp_core::ui::theme::Fill;

/// Optional visual overrides for BranchPicker / RepoPicker dropdowns.
pub struct PickerStyle {
    pub width: f32,
    pub background: Option<Fill>,
}
