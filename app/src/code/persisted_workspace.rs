use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(feature = "local_fs")]
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use anyhow::Context;
use chrono::{DateTime, Days, Utc};
use itertools::Itertools;
#[cfg(feature = "local_fs")]
use lsp::LspEvent;
use lsp::supported_servers::LSPServerType;
#[cfg(feature = "local_fs")]
use lsp::{LspManagerModel, LspServerConfig};
#[cfg(feature = "local_fs")]
use repo_metadata::RepoMetadataModel;
use serde::{Deserialize, Serialize};
#[cfg(feature = "local_fs")]
use warp_core::channel::ChannelState;
#[cfg(feature = "local_fs")]
use warp_core::context_flag::ContextFlag;
use warp_errors::report_if_error;
#[cfg(feature = "local_fs")]
use warp_util::standardized_path::StandardizedPath;
#[cfg(feature = "local_fs")]
use warpui::windowing::WindowManager;
use warpui::{Entity, ModelContext, SingletonEntity};

#[cfg(feature = "local_fs")]
use crate::code::language_server_shutdown_manager::LanguageServerShutdownManager;
use crate::persistence::ModelEvent;
#[cfg(feature = "local_fs")]
use crate::server::network_logging::NetworkLogModel;
#[cfg(feature = "local_fs")]
use crate::terminal::local_shell::LocalShellState;
#[cfg(feature = "local_fs")]
use crate::{view_components::DismissibleToast, workspace::ToastStack};

/// Represents whether an LSP server is enabled or disabled for a workspace.
///
/// This is also used in underlying sqlite type persistence. We should be careful
/// not to rename an existing variant, as it will break persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnablementState {
    Yes,
    No,
    /// Server was detected as available for a repo but not yet explicitly
    /// enabled/disabled by the user. Entries with this state live only in
    /// memory and are never persisted to SQLite.
    Suggested,
}

/// Describes an LSP operation to be executed after capturing the interactive shell PATH.
#[cfg(feature = "local_fs")]
pub enum LspTask {
    /// Spawn LSP servers for a file path.
    Spawn { file_path: PathBuf },
}

pub struct Workspace {
    metadata: WorkspaceMetadata,
    language_servers: HashMap<LSPServerType, EnablementState>,
}

impl Workspace {
    /// Returns `true` if this workspace has been persisted to SQLite.
    ///
    /// A workspace created solely from available-server detection will have
    /// all metadata timestamps set to `None` and is considered non-persisted.
    fn is_persisted(&self) -> bool {
        let persisted = self.metadata.navigated_ts.is_some()
            || self.metadata.modified_ts.is_some()
            || self.metadata.queried_ts.is_some();

        if !persisted {
            debug_assert!(
                self.language_servers
                    .values()
                    .all(|s| *s == EnablementState::Suggested),
                "non-persisted workspace has Yes/No server state; persist metadata first"
            );
        }

        persisted
    }
}

/// Manages a set of code workspaces that the app recognizes. These workspaces define
/// the scope of repo-based features such as LSP.
pub struct PersistedWorkspace {
    workspaces: HashMap<PathBuf, Workspace>,
    model_event_sender: Option<SyncSender<ModelEvent>>,
    #[cfg(feature = "local_fs")]
    http_client: Arc<http_client::Client>,
}

#[derive(Debug, Clone)]
pub enum PersistedWorkspaceEvent {
    /// Emitted when the user explicitly adds a repo via a picker (e.g. the tab-config
    /// params modal's repo dropdown). Subscribers can use this to refresh their list.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    WorkspaceAdded { path: PathBuf },
}

impl Entity for PersistedWorkspace {
    type Event = PersistedWorkspaceEvent;
}

impl SingletonEntity for PersistedWorkspace {}

impl PersistedWorkspace {
    #[cfg(test)]
    pub fn new_for_test(_ctx: &mut ModelContext<Self>) -> Self {
        Self {
            workspaces: HashMap::new(),
            model_event_sender: None,
            #[cfg(feature = "local_fs")]
            http_client: Arc::new(http_client::Client::new_external()),
        }
    }

    pub fn new(
        metadata: Vec<WorkspaceMetadata>,
        workspace_language_servers: HashMap<PathBuf, HashMap<LSPServerType, EnablementState>>,
        model_event_sender: Option<SyncSender<ModelEvent>>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        #[cfg(not(feature = "local_fs"))]
        let _ = ctx;
        let metadata: HashMap<PathBuf, Workspace> = metadata
            .into_iter()
            .map(|metadata| {
                let path = metadata.path.clone();
                let language_servers = workspace_language_servers
                    .get(&path)
                    .cloned()
                    .unwrap_or_default();

                (
                    path,
                    Workspace {
                        metadata,
                        language_servers,
                    },
                )
            })
            .collect();

        #[cfg(feature = "local_fs")]
        let http_client = {
            let mut client = http_client::Client::new_external();
            if ContextFlag::NetworkLogConsole.is_enabled() {
                NetworkLogModel::handle(ctx).update(ctx, |model, model_ctx| {
                    model.install_on_clients([&mut client], model_ctx);
                });
            }
            Arc::new(client)
        };

        Self {
            workspaces: metadata,
            model_event_sender,
            #[cfg(feature = "local_fs")]
            http_client,
        }
    }

    pub fn root_for_workspace<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.ancestors()
            .find(|&path| self.workspaces.contains_key(path))
    }

    /// Returns the enabled lsp servers for a given repo path.
    #[cfg(feature = "local_fs")]
    pub fn enabled_lsp_servers(
        &self,
        path: &Path,
    ) -> Option<impl Iterator<Item = LSPServerType> + use<'_>> {
        let root = self.root_for_workspace(path)?;

        self.workspaces.get(root).map(|workspace| {
            workspace
                .language_servers
                .iter()
                .filter_map(|(server_type, state)| {
                    if *state == EnablementState::Yes {
                        Some(*server_type)
                    } else {
                        None
                    }
                })
        })
    }

    /// Explicitly registers a directory as a workspace, as if the user had navigated there.
    ///
    /// Creates or updates the entry with `navigated_ts = now`, persists to SQLite,
    /// starts full repo-metadata indexing, and emits
    /// [`PersistedWorkspaceEvent::WorkspaceAdded`] so subscribers can refresh their UI.
    pub fn user_added_workspace(&mut self, path: PathBuf, ctx: &mut ModelContext<Self>) {
        let now = Utc::now();

        match self.workspaces.get_mut(&path) {
            Some(workspace) => {
                workspace.metadata.navigated_ts = Some(now);
            }
            None => {
                self.workspaces.insert(
                    path.clone(),
                    Workspace {
                        metadata: WorkspaceMetadata {
                            path: path.clone(),
                            navigated_ts: Some(now),
                            modified_ts: None,
                            queried_ts: None,
                        },
                        language_servers: HashMap::new(),
                    },
                );
            }
        }

        self.persist_workspace_metadata(&path);
        #[cfg(feature = "local_fs")]
        match StandardizedPath::from_local_canonicalized(&path) {
            Ok(path) => {
                if let Err(error) = RepoMetadataModel::handle(ctx).update(ctx, |model, ctx| {
                    model.index_local_directory_path(&path, ctx)
                }) {
                    log::warn!("Failed to start full repo metadata indexing for {path}: {error}");
                }
            }
            Err(error) => {
                log::warn!(
                    "Failed to canonicalize user-added workspace {} for full repo metadata indexing: {error}",
                    path.display()
                );
            }
        }
        ctx.emit(PersistedWorkspaceEvent::WorkspaceAdded { path });
    }

    pub fn workspaces<'a>(&'a self) -> impl Iterator<Item = WorkspaceMetadata> + use<'a> {
        self.workspaces
            .values()
            .filter(|workspace| workspace.is_persisted())
            .map(|workspace| workspace.metadata.clone())
            .sorted_by(WorkspaceMetadata::most_recently_touched)
            .dedup_by(|a, b| a.path == b.path)
    }

    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    pub fn navigated_to_path(&mut self, directory: &PathBuf) {
        if let Some(workspace) = self.workspaces.get_mut(directory) {
            workspace.metadata.navigated_ts = Some(Utc::now());
            self.persist_workspace_metadata(directory);
        }
    }

    pub fn workspace_for_path(&self, root_path: &Path) -> Option<WorkspaceMetadata> {
        self.workspaces
            .get(root_path)
            .map(|workspace| workspace.metadata.clone())
    }

    fn persist_workspace_metadata(&self, path: &PathBuf) {
        log::info!("Saving workspace metadata for {path:?} to SQLite");

        if let Some(single_metadata) = self.workspace_for_path(path) {
            self.save_to_db(vec![ModelEvent::UpsertWorkspaceMetadata {
                metadata: Box::new(single_metadata),
            }]);
        }
    }

    fn save_to_db(&self, events: impl IntoIterator<Item = ModelEvent>) {
        let model_event_sender = self.model_event_sender.clone();
        if let Some(model_event_sender) = &model_event_sender {
            for event in events {
                report_if_error!(
                    model_event_sender
                        .send(event)
                        .with_context(|| "Unable to save workspace metadata to sqlite")
                );
            }
        }
    }

    /// Starts all enabled LSP servers for the given file path.
    /// This looks up the workspace root and starts any servers that are enabled but not yet running.
    #[cfg(feature = "local_fs")]
    fn handle_spawn_lsp(
        &self,
        file_path: &Path,
        path_env_var: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(workspace_root) = self.root_for_workspace(file_path) else {
            return;
        };

        let Some(servers) = self.enabled_lsp_servers(workspace_root) else {
            return;
        };

        let supported_servers = servers.collect::<Vec<LSPServerType>>();

        if supported_servers.is_empty() {
            return;
        }

        let mut new_servers_available_to_start = false;
        let workspace_root = workspace_root.to_path_buf();

        for server in supported_servers {
            if LspManagerModel::as_ref(ctx).server_registered_and_started(
                &workspace_root,
                server,
                ctx,
            ) {
                continue;
            }

            log::info!(
                "Starting {} LSP server for {}",
                server.binary_name(),
                workspace_root.display()
            );
            let log_relative_path =
                crate::code::lsp_logs::relative_log_path(server, &workspace_root);
            let http_client = self.http_client.clone();
            let config = LspServerConfig::new(
                server,
                workspace_root.clone(),
                path_env_var.clone(),
                ChannelState::app_id().application_name().to_string(),
                http_client,
            )
            .with_log_relative_path(log_relative_path);

            LspManagerModel::handle(ctx).update(ctx, |manager, m_ctx| {
                manager.register(workspace_root.clone(), config, m_ctx);
            });
            new_servers_available_to_start = true;
        }

        if !new_servers_available_to_start {
            return;
        }

        let lsp_manager_handle = LspManagerModel::handle(ctx);
        lsp_manager_handle.update(ctx, |manager, m_ctx| {
            manager.start_all(workspace_root.clone(), m_ctx);
        });

        // Subscribe to LSP server events to show error toast on failure.
        let workspace_root_display = workspace_root.display().to_string();
        let servers = lsp_manager_handle
            .as_ref(ctx)
            .servers_for_workspace(&workspace_root)
            .cloned()
            .unwrap_or_default();

        for server in servers {
            let workspace_root_display = workspace_root_display.clone();
            ctx.subscribe_to_model(&server, move |_me, _, event, ctx| {
                if let LspEvent::Failed(e) = event
                    && let Some(window_id) = WindowManager::as_ref(ctx).active_window()
                {
                    ToastStack::handle(ctx).update(ctx, |toast_stack, ctx| {
                        let toast = DismissibleToast::error(format!(
                            "Failed to start LSP server for {workspace_root_display} with error {e}",
                        ));
                        toast_stack.add_ephemeral_toast(toast, window_id, ctx);
                    });
                }
            });
        }

        // Once we start a LSP server, also start the garbage collection process if it is not active.
        LanguageServerShutdownManager::handle(ctx).update(ctx, |shutdown_manager, ctx| {
            if !shutdown_manager.has_in_progress_scan() {
                shutdown_manager.schedule_next_scan(ctx);
            }
        });
    }

    /// Executes an LSP task after capturing the interactive shell PATH.
    /// This is the main entry point for LSP operations that need the full PATH.
    #[cfg(feature = "local_fs")]
    pub fn execute_lsp_task(&mut self, task: LspTask, ctx: &mut ModelContext<Self>) {
        let LspTask::Spawn { file_path } = task;
        let has_servers = self
            .root_for_workspace(&file_path)
            .and_then(|root| self.enabled_lsp_servers(root))
            .is_some_and(|mut servers| servers.next().is_some());
        if !has_servers {
            return;
        }

        // Get a future for the interactive PATH
        let path_future = LocalShellState::handle(ctx).update(ctx, |shell_state, ctx| {
            shell_state.get_interactive_path_env_var(ctx)
        });

        ctx.spawn(path_future, move |me, path_env_var, ctx| {
            me.handle_spawn_lsp(&file_path, path_env_var, ctx);
        });
    }
}

/// Public-facing metadata persisted in SQLite
#[derive(Debug, Default, Clone)]
pub struct WorkspaceMetadata {
    pub path: PathBuf,
    pub navigated_ts: Option<DateTime<Utc>>,
    pub modified_ts: Option<DateTime<Utc>>,
    pub queried_ts: Option<DateTime<Utc>>,
}

impl WorkspaceMetadata {
    /// Surface most recently navigated first
    pub fn most_recently_navigated(a: &Self, b: &Self) -> std::cmp::Ordering {
        match (a.navigated_ts, b.navigated_ts) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.path.cmp(&b.path),
        }
    }

    /// Surface most recently touched first
    pub fn most_recently_touched(a: &Self, b: &Self) -> std::cmp::Ordering {
        match (a.last_touched(), b.last_touched()) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.path.cmp(&b.path),
        }
    }

    /// The most recent time this codebase index was navigated to, queried or modified.
    pub fn last_touched(&self) -> Option<DateTime<Utc>> {
        let mut last_access_time: Option<DateTime<Utc>> = None;
        if let Some(nav_ts) = self.navigated_ts {
            last_access_time = last_access_time
                .map(|old_time| old_time.max(nav_ts))
                .or(Some(nav_ts));
        }
        if let Some(mod_ts) = self.modified_ts {
            last_access_time = last_access_time
                .map(|old_time| old_time.max(mod_ts))
                .or(Some(mod_ts));
        }
        if let Some(query_ts) = self.queried_ts {
            last_access_time = last_access_time
                .map(|old_time| old_time.max(query_ts))
                .or(Some(query_ts));
        }
        last_access_time
    }

    pub fn is_expired(&self, current_time: DateTime<Utc>, shelf_life_days: u64) -> bool {
        let Some(last_touch) = self.last_touched() else {
            return true;
        };
        last_touch
            .checked_add_days(Days::new(shelf_life_days))
            .unwrap_or_default()
            < current_time
    }
}

impl From<WorkspaceMetadata> for persistence::model::NewWorkspaceMetadata {
    fn from(value: WorkspaceMetadata) -> Self {
        Self {
            repo_path: value.path.to_string_lossy().into_owned(),
            navigated_ts: value.navigated_ts.map(|utc_dt| utc_dt.naive_utc()),
            modified_ts: value.modified_ts.map(|utc_dt| utc_dt.naive_utc()),
            queried_ts: value.queried_ts.map(|utc_dt| utc_dt.naive_utc()),
        }
    }
}

impl From<persistence::model::WorkspaceMetadata> for WorkspaceMetadata {
    fn from(value: persistence::model::WorkspaceMetadata) -> Self {
        Self {
            path: PathBuf::from(value.repo_path),
            navigated_ts: value.navigated_ts.map(|naive_ts| naive_ts.and_utc()),
            modified_ts: value.modified_ts.map(|naive_ts| naive_ts.and_utc()),
            queried_ts: value.queried_ts.map(|naive_ts| naive_ts.and_utc()),
        }
    }
}
