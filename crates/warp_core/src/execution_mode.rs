use warpui_core::{Entity, ModelContext, SingletonEntity};

/// Role of the desktop application or one of its SSH workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionMode {
    App,
    RemoteServerProxy,
    RemoteServerDaemon,
}

/// Tracks which process role owns local session state.
#[derive(Clone, Debug)]
pub struct AppExecutionMode {
    mode: ExecutionMode,
}

impl AppExecutionMode {
    pub fn new(mode: ExecutionMode, _ctx: &mut ModelContext<Self>) -> Self {
        Self { mode }
    }

    /// Whether the application can save and restore desktop sessions.
    pub fn can_save_session(&self) -> bool {
        matches!(self.mode, ExecutionMode::App)
    }
}

impl Entity for AppExecutionMode {
    type Event = ();
}

impl SingletonEntity for AppExecutionMode {}
