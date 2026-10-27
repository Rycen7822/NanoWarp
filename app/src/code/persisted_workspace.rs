use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::SyncSender;

use anyhow::Context;
use chrono::{DateTime, Days, Utc};
use itertools::Itertools;
#[cfg(feature = "local_fs")]
use repo_metadata::RepoMetadataModel;
use warp_errors::report_if_error;
#[cfg(feature = "local_fs")]
use warp_util::standardized_path::StandardizedPath;
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::persistence::ModelEvent;

pub struct Workspace {
    metadata: WorkspaceMetadata,
}

impl Workspace {
    /// Returns `true` if this workspace has been persisted to SQLite.
    fn is_persisted(&self) -> bool {
        self.metadata.navigated_ts.is_some()
            || self.metadata.modified_ts.is_some()
            || self.metadata.queried_ts.is_some()
    }
}

/// Tracks recently used directories and their persisted metadata.
pub struct PersistedWorkspace {
    workspaces: HashMap<PathBuf, Workspace>,
    model_event_sender: Option<SyncSender<ModelEvent>>,
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
        }
    }

    pub fn new(
        metadata: Vec<WorkspaceMetadata>,
        model_event_sender: Option<SyncSender<ModelEvent>>,
    ) -> Self {
        Self {
            workspaces: metadata
                .into_iter()
                .map(|metadata| (metadata.path.clone(), Workspace { metadata }))
                .collect(),
            model_event_sender,
        }
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
