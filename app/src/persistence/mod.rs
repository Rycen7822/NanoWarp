#![cfg_attr(not(feature = "local_fs"), allow(dead_code))]

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        mod block_list;
        mod sqlite;
        pub mod commands;
    }
}

pub use persistence::model;
#[cfg_attr(not(feature = "local_fs"), expect(unused_imports))]
pub use persistence::schema;

#[cfg(feature = "integration_tests")]
pub mod testing;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;

use chrono::{DateTime, Local};
use instant::Instant;
use lsp::supported_servers::LSPServerType;
#[cfg(any(feature = "local_fs", feature = "integration_tests"))]
pub use sqlite::database_file_path_for_current_scope;
// Only re-exported for integration tests (via `integration_testing::persistence`);
// in-crate code should resolve paths through `database_file_path_for_current_scope`.
#[cfg(any(feature = "local_fs", feature = "integration_tests"))]
#[cfg_attr(not(feature = "integration_tests"), expect(unused_imports))]
pub use sqlite::database_file_path_for_scope;
#[cfg(any(feature = "local_fs", feature = "integration_tests"))]
pub use sqlite::establish_ro_connection;
use warp_core::command::ExitCode;
use warp_errors::report_error;
use warpui::{Entity, SingletonEntity};

use crate::app_state::AppState;
use crate::code::persisted_workspace::{
    EnablementState, WorkspaceMetadata as CodeWorkspaceMetadata,
};
use crate::suggestions::ignored_suggestions_model::SuggestionType;
use crate::terminal::history::PersistedCommand;
use crate::terminal::model::block::SerializedBlock;
use crate::terminal::model::session::SessionId;

#[derive(Clone)]
pub enum PersistenceScope {
    /// The GUI app (and other launch modes that share its database).
    App,
    RemoteServerDaemon {
        identity_key: String,
    },
}

/// The [`PersistenceScope`] this process's persistence was initialized with.
///
/// Set once by [`initialize`]. Code that opens ad-hoc read-only connections
/// should resolve the database path through [`current_scope`] (or
/// `database_file_path_for_current_scope`) rather than hardcoding a scope, so
/// it reads the same database as the writer regardless of which front-end
/// this process is running.
static CURRENT_SCOPE: OnceLock<PersistenceScope> = OnceLock::new();

/// Returns the scope [`initialize`] was called with, defaulting to
/// [`PersistenceScope::App`] when persistence has not been initialized (e.g.
/// tests that construct models directly).
pub fn current_scope() -> PersistenceScope {
    CURRENT_SCOPE
        .get()
        .cloned()
        .unwrap_or(PersistenceScope::App)
}

/// Initializes the persistence "subsystem".
///
/// Returns the previously-persisted data, if any, and handles for
/// writing updated data to persist, if the persistence subsystem is
/// available.
#[tracing::instrument(name = "persistence::initialize", skip_all)]
#[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
pub fn initialize(scope: PersistenceScope) -> (Option<Box<PersistedData>>, Option<WriterHandles>) {
    // Record the scope for ad-hoc read-only connections; keep the first value
    // if this is ever called more than once in a process (e.g. tests).
    let _ = CURRENT_SCOPE.set(scope.clone());
    cfg_if::cfg_if! {
        if #[cfg(feature = "local_fs")] {
            sqlite::initialize(scope)
        } else {
            (None, None)
        }
    }
}

/// Holds interfaces to the writer thread.
pub struct WriterHandles {
    pub handle: JoinHandle<()>,
    pub sender: SyncSender<ModelEvent>,
}

/// Model for interacting with the writer thread.
pub struct PersistenceWriter {
    thread_handle: Option<JoinHandle<()>>,
    model_event_sender: Option<SyncSender<ModelEvent>>,
}

impl PersistenceWriter {
    pub fn new(handle: Option<WriterHandles>) -> Self {
        let (thread_handle, model_event_sender) = match handle {
            Some(handle) => (Some(handle.handle), Some(handle.sender)),
            None => (None, None),
        };
        Self {
            thread_handle,
            model_event_sender,
        }
    }

    /// Sending half for sending model updates to the persistence writer thread.
    pub fn sender(&self) -> Option<SyncSender<ModelEvent>> {
        self.model_event_sender.clone()
    }

    /// Synchronously terminate the SQLite writer thread.
    pub fn terminate(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            let start = Instant::now();
            let Some(sender) = self.sender() else {
                report_error!("Model event sender should exist if thread handle is set");
                return;
            };
            if let Err(err) = sender.send(ModelEvent::Terminate) {
                report_error!(
                    anyhow::Error::new(err).context("Could not terminate SQLite writer thread")
                );
            }
            if handle.join().is_err() {
                // If crash reporting is enabled, Sentry will have already handled the panic.
                report_error!("SQLite writer thread panicked");
            }
            log::info!("Shut down SQLite writer in {:?}", start.elapsed());
        }
    }
}

impl Drop for PersistenceWriter {
    fn drop(&mut self) {
        self.terminate();
    }
}

impl Entity for PersistenceWriter {
    type Event = ();
}

impl SingletonEntity for PersistenceWriter {}

/// Local session restoration, command history, and code-workspace state.
pub struct PersistedData {
    /// Session restoration data.
    pub app_state: Option<AppState>,

    pub command_history: Vec<PersistedCommand>,
    pub workspace_metadata: Vec<CodeWorkspaceMetadata>,
    pub workspace_language_servers: HashMap<PathBuf, HashMap<LSPServerType, EnablementState>>,
    pub ignored_suggestions: Vec<(String, SuggestionType)>,
}

#[derive(Clone, Debug)]
pub struct BlockCompleted {
    pub pane_id: Vec<u8>,
    /// Indicates if the block was created locally (e.g. not in a remote session)
    pub is_local: bool,
    pub block: Arc<SerializedBlock>,
}

#[derive(Debug)]
pub struct StartedCommandMetadata {
    pub command: String,
    pub start_ts: Option<DateTime<Local>>,
    pub pwd: Option<String>,
    pub shell: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub session_id: Option<SessionId>,
    pub git_branch: Option<String>,
}

#[derive(Debug)]
pub struct FinishedCommandMetadata {
    pub exit_code: ExitCode,
    pub start_ts: DateTime<Local>,
    pub completed_ts: DateTime<Local>,
    pub session_id: SessionId,
}

#[derive(Debug)]
pub enum ModelEvent {
    SaveBlock(BlockCompleted),
    DeleteBlocks(Vec<u8>),
    Snapshot(AppState),
    InsertCommand {
        metadata: StartedCommandMetadata,
    },
    UpdateFinishedCommand {
        metadata: FinishedCommandMetadata,
    },
    /// Close the SQLite writer thread when the app is about to quit.
    Terminate,
    UpsertWorkspaceMetadata {
        metadata: Box<CodeWorkspaceMetadata>,
    },
    AddIgnoredSuggestion {
        suggestion: String,
        suggestion_type: SuggestionType,
    },
    RemoveIgnoredSuggestion {
        suggestion: String,
        suggestion_type: SuggestionType,
    },
}
