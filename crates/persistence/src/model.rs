//! These types are named after the database tables, and are used to represent specific queries.

use chrono::NaiveDateTime;
use diesel::prelude::*;

use super::schema::{
    app, blocks, code_pane_tabs, code_panes, commands, ignored_suggestions, notebook_panes,
    pane_branches, pane_leaves, pane_nodes, settings_panes, tab_groups, tabs, terminal_panes,
    windows, workspace_metadata,
};

#[derive(Insertable)]
#[diesel(table_name = app)]
pub struct NewApp {
    pub active_window_id: Option<i32>,
}

#[derive(Identifiable, Queryable)]
pub struct Window {
    pub id: i32,
    pub active_tab_index: i32,
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
    pub origin_x: Option<f32>,
    pub origin_y: Option<f32>,
    pub quake_mode: bool,
    pub universal_search_width: Option<f32>,
    pub fullscreen_state: i32,
    pub vertical_tabs_panel_open: Option<bool>,
}

#[derive(Clone, Identifiable, Queryable, AsChangeset)]
#[diesel(table_name = workspace_metadata)]
pub struct WorkspaceMetadata {
    pub id: i32,
    pub repo_path: String,
    pub navigated_ts: Option<NaiveDateTime>,
}

#[derive(Clone, Insertable, AsChangeset)]
#[diesel(table_name = workspace_metadata)]
pub struct NewWorkspaceMetadata {
    pub repo_path: String,
    pub navigated_ts: Option<NaiveDateTime>,
}

#[derive(Insertable)]
#[diesel(table_name = windows)]
pub struct NewWindow {
    pub active_tab_index: i32,
    pub window_width: Option<f32>,
    pub window_height: Option<f32>,
    pub origin_x: Option<f32>,
    pub origin_y: Option<f32>,
    pub quake_mode: bool,
    pub universal_search_width: Option<f32>,
    pub fullscreen_state: i32,
    pub vertical_tabs_panel_open: Option<bool>,
}

#[derive(Identifiable, Queryable, Associations)]
#[diesel(belongs_to(Window))]
pub struct Tab {
    pub id: i32,
    pub window_id: i32,
    pub custom_title: Option<String>,
    pub color: Option<String>,
    pub tab_group_id: Option<i32>,
    pub pinned: bool,
}

#[derive(Insertable)]
#[diesel(table_name = tabs)]
pub struct NewTab {
    pub window_id: i32,
    pub custom_title: Option<String>,
    pub color: Option<String>,
    pub tab_group_id: Option<i32>,
    pub pinned: bool,
}

/// Persisted form of a tab group. `name` is optional — untitled groups omit
/// it and the UI falls back to a default label.
#[derive(Identifiable, Queryable, Associations)]
#[diesel(belongs_to(Window))]
#[diesel(table_name = tab_groups)]
pub struct TabGroup {
    pub id: i32,
    pub window_id: i32,
    pub name: Option<String>,
    pub color: Option<String>,
    pub collapsed: bool,
    pub pinned: bool,
}

#[derive(Insertable)]
#[diesel(table_name = tab_groups)]
pub struct NewTabGroup {
    pub window_id: i32,
    pub name: Option<String>,
    pub color: Option<String>,
    pub collapsed: bool,
    pub pinned: bool,
}

/// The panes data model includes pane_nodes, pane_leaves and pane_branches.
/// In addition, each kind of pane has a table for its specific data (i.e. the cwd for terminal panes).
/// The pane_nodes table specifically keeps the node data so it is responsible for
/// keeping track of the tree relationships.
/// The pane_leaves table keeps info about a given pane (i.e. what kind of pane it is).
/// The pane_branches table keeps info about a branch in the tree (e.g. whether
/// the branch splits horizontally or vertically).
#[derive(Identifiable, Queryable)]
#[diesel(table_name = pane_leaves)]
#[diesel(primary_key(pane_node_id, kind))]
pub struct PaneLeaf {
    pub pane_node_id: i32,
    pub kind: String,
    pub is_focused: bool,
    pub custom_vertical_tabs_title: Option<String>,
}

#[derive(Identifiable, Queryable, Selectable)]
#[diesel(table_name = terminal_panes)]
#[diesel(primary_key(id))]
pub struct TerminalPane {
    pub id: i32,
    // This is hardcoded in the database, and not used in the app, but Diesel requires it so that
    // fields are in the same order as the table's columns.
    pub kind: String,
    pub uuid: Vec<u8>,
    pub cwd: Option<String>,
    pub is_active: bool,
    /// This is serialized JSON data for a ShellLaunchData struct.
    pub shell_launch_data: Option<String>,
}

#[derive(Identifiable, Queryable, Selectable)]
#[diesel(table_name = notebook_panes)]
#[diesel(primary_key(id))]
pub struct NotebookPane {
    pub id: i32,
    // This is hardcoded in the database, and not used in the app, but Diesel requires it so that
    // fields are in the same order as the table's columns.
    pub kind: String,
    pub local_path: Option<Vec<u8>>,
}

#[derive(Identifiable, Queryable, Selectable)]
#[diesel(table_name = code_panes)]
#[diesel(primary_key(id))]
pub struct CodePane {
    pub id: i32,
    pub active_tab_index: i32,
    pub source_data: Option<String>,
}

#[derive(Identifiable, Queryable, Selectable)]
#[diesel(table_name = code_pane_tabs)]
#[diesel(primary_key(id))]
pub struct CodePaneTab {
    pub id: i32,
    pub code_pane_id: i32,
    pub tab_index: i32,
    pub local_path: Option<Vec<u8>>,
}

#[derive(Identifiable, Queryable, Selectable)]
#[diesel(table_name = settings_panes)]
#[diesel(primary_key(id))]
pub struct SettingsPane {
    pub id: i32,
    pub kind: String,
    pub current_page: String,
}

/// Subset of the [`terminal_panes`] table needed to retrieve per-session block lists.
///
/// The true primary key of the table is [`terminal_panes::id`]. However, Diesel's associations API
/// requires matching on the primary key, so this view pretends that [`terminal_panes::uuid`] is
/// the primary key. This is safe because the UUID is _also_ unique across all panes.
#[derive(Identifiable, Selectable, Queryable)]
#[diesel(table_name = terminal_panes)]
#[diesel(primary_key(uuid))]
pub struct TerminalSession {
    pub uuid: Vec<u8>,
}

#[derive(Queryable)]
pub struct PaneBranch {
    #[allow(dead_code)]
    pub id: i32,
    #[allow(dead_code)]
    pub pane_node_id: i32,
    pub horizontal: bool,
}

#[derive(Queryable)]
pub struct PaneNode {
    pub id: i32,
    #[allow(dead_code)]
    pub tab_id: i32,
    #[allow(dead_code)]
    pub parent_pane_node_id: Option<i32>,
    pub flex: Option<f32>,
    pub is_leaf: bool,
}

#[derive(Insertable)]
#[diesel(table_name = pane_leaves)]
pub struct NewPane {
    pub pane_node_id: i32,
    pub kind: String,
    pub is_focused: bool,
    pub custom_vertical_tabs_title: Option<String>,
}

/// The [`pane_leaves::kind`] value for terminal panes.
pub const TERMINAL_PANE_KIND: &str = "terminal";

/// The [`pane_leaves::kind`] value for notebook panes.
pub const NOTEBOOK_PANE_KIND: &str = "notebook";

/// The [`pane_leaves::kind`] value for code panes.
pub const CODE_PANE_KIND: &str = "code";

/// The [`pane_leaves::kind`] value for settings panes.
pub const SETTINGS_PANE_KIND: &str = "settings";

#[derive(Insertable)]
#[diesel(table_name = terminal_panes)]
pub struct NewTerminalPane {
    pub id: i32,
    pub uuid: Vec<u8>,
    pub cwd: Option<String>,
    pub is_active: bool,
    /// This is serialized JSON data for a ShellLaunchData struct.
    pub shell_launch_data: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = notebook_panes)]
pub struct NewNotebookPane {
    pub id: i32,
    pub local_path: Option<Vec<u8>>,
}

#[derive(Insertable)]
#[diesel(table_name = code_panes)]
pub struct NewCodePane {
    pub id: i32,
    pub active_tab_index: i32,
    pub source_data: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = code_pane_tabs)]
pub struct NewCodePaneTab {
    pub code_pane_id: i32,
    pub tab_index: i32,
    pub local_path: Option<Vec<u8>>,
}

#[derive(Insertable)]
#[diesel(table_name = settings_panes)]
pub struct NewSettingsPane {
    pub id: i32,
    pub current_page: String,
}

#[derive(Insertable)]
#[diesel(table_name = pane_branches)]
pub struct NewPaneBranch {
    pub pane_node_id: i32,
    pub horizontal: bool,
}

#[derive(Insertable)]
#[diesel(table_name = pane_nodes)]
pub struct NewPaneNode {
    pub tab_id: i32,
    pub parent_pane_node_id: Option<i32>,
    pub flex: Option<f32>,
    pub is_leaf: bool,
}

#[derive(Insertable)]
#[diesel(table_name = blocks)]
pub struct NewBlock<'a> {
    pub block_id: &'a str,
    // Note that there is no pane leaf UUID foreign key relationship because there's no good way to
    // enforce it: when we remove a pane and subsequently create a new snapshot, the old blocks
    // will now violate the constraint. While sqlite does have deferred constraints, it doesn't
    // work well with ON DELETE CASCADE (i.e. the cascade happens on the delete, not after the
    // transaction commit).
    pub pane_leaf_uuid: Vec<u8>,
    pub stylized_command: &'a Vec<u8>,
    pub stylized_output: &'a Vec<u8>,
    pub pwd: Option<&'a String>,
    pub git_branch: Option<&'a String>,
    pub git_branch_name: Option<&'a String>,
    pub virtual_env: Option<&'a String>,
    pub conda_env: Option<&'a String>,
    pub exit_code: i32,
    pub did_execute: bool,
    pub is_background: bool,
    pub completed_ts: Option<NaiveDateTime>,
    pub start_ts: Option<NaiveDateTime>,
    pub ps1: Option<&'a String>,
    pub rprompt: Option<&'a String>,
    pub honor_ps1: bool,
    pub shell: Option<&'a str>,
    pub user: Option<&'a str>,
    pub host: Option<&'a str>,
    pub prompt_snapshot: Option<&'a String>,
    pub is_local: Option<bool>,
}

#[derive(Identifiable, Queryable, Selectable, Associations)]
#[diesel(table_name = blocks)]
#[diesel(belongs_to(TerminalSession, foreign_key = pane_leaf_uuid))]
pub struct Block {
    pub id: Option<i32>,
    pub pane_leaf_uuid: Vec<u8>,
    pub stylized_command: Vec<u8>,
    pub stylized_output: Vec<u8>,
    pub pwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_branch_name: Option<String>,
    pub virtual_env: Option<String>,
    pub conda_env: Option<String>,
    pub exit_code: i32,
    pub did_execute: bool,
    pub completed_ts: Option<NaiveDateTime>,
    pub start_ts: Option<NaiveDateTime>,
    pub ps1: Option<String>,
    pub honor_ps1: bool,
    pub shell: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
    pub is_background: bool,
    pub rprompt: Option<String>,
    /// JSON-serialized representation of the Warp prompt snapshot (Context Chips). Note that this
    /// is different from PS1 and RPROMPT1
    pub prompt_snapshot: Option<String>,
    pub block_id: String,
    pub is_local: Option<bool>,
}

#[derive(Insertable)]
#[diesel(table_name = commands)]
pub struct NewCommand {
    pub command: String,
    pub exit_code: Option<i32>,
    pub start_ts: Option<NaiveDateTime>,
    pub completed_ts: Option<NaiveDateTime>,
    pub pwd: Option<String>,
    pub shell: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub session_id: Option<i64>,
    pub git_branch: Option<String>,
}

#[derive(Identifiable, Queryable, Default, Clone)]
#[diesel(table_name = commands)]
pub struct Command {
    pub id: i32,
    pub command: String,
    pub exit_code: Option<i32>,
    pub start_ts: Option<NaiveDateTime>,
    pub completed_ts: Option<NaiveDateTime>,
    pub pwd: Option<String>,
    pub shell: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub session_id: Option<i64>,
    pub git_branch: Option<String>,
}

#[derive(Debug, Insertable)]
#[diesel(table_name = ignored_suggestions)]
pub struct NewIgnoredSuggestion {
    pub suggestion: String,
    pub suggestion_type: String,
}
