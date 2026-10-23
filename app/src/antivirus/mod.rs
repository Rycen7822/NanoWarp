//! Module containing utilities to query the currently running antivirus / EDR software on the
//! user's machine.

mod telemetry;
#[cfg(windows)]
mod windows;

use warpui::{Entity, ModelContext, SingletonEntity};

/// Singleton model that reports the currently running antivirus software.
#[derive(Debug, Clone)]
pub struct AntivirusInfo(Option<String>);

impl AntivirusInfo {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        #[cfg(windows)]
        _ctx.spawn(async move { Self::scan().await }, Self::on_scan_complete);

        Self(None)
    }

    /// Returns the currently running antivirus software if any.
    /// Returns [`None`] until the scan finishes.
    ///
    /// ## Platform-specific
    /// This function always returns `None` on non-Windows platforms.
    pub fn get(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

impl Entity for AntivirusInfo {
    type Event = ();
}

impl SingletonEntity for AntivirusInfo {}
