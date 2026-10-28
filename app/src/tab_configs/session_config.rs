use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(feature = "local_fs")]
use anyhow::Result;

use super::tab_config::{TabConfig, TabConfigPaneNode, TabConfigPaneType};
use crate::app_state::{BranchSnapshot, LeafContents, LeafSnapshot, PaneNodeSnapshot};
use crate::launch_configs::launch_config::SplitDirection;
use crate::themes::theme::AnsiColorIdentifier;

/// The user's selections from the session config modal.
///
/// This is the modal's output — the caller decides what to do with it.
pub struct SessionConfigSelection {
    pub directory: PathBuf,
}

/// Derives a human-readable config name from the directory.
fn config_name(directory: &Path) -> String {
    let repo = directory
        .file_name()
        .and_then(|n| n.to_str())
        .or_else(|| directory.to_str())
        .unwrap_or("untitled");
    format!("New tab: {repo}")
}

/// Builds a `TabConfig` from the given session parameters.
///
/// This is a pure function — it produces the canonical `TabConfig`
/// that can be either rendered directly (via `render_tab_config`) or
/// serialized to disk (via `write_tab_config`).
pub fn build_tab_config(directory: &Path) -> TabConfig {
    TabConfig {
        name: config_name(directory),
        title: None,
        color: None,
        panes: vec![TabConfigPaneNode {
            id: "main".to_string(),
            pane_type: Some(TabConfigPaneType::Terminal),
            split: None,
            children: None,
            is_focused: None,
            directory: Some(directory.to_string_lossy().into_owned()),
            commands: None,
            shell: None,
        }],
        params: HashMap::new(),
        source_path: None,
    }
}

/// Serializes a `TabConfig` to TOML and writes it to an unused path in `dir`.
///
/// Creates `dir` if it doesn't exist. Returns the path of the written file.
/// The filesystem watcher will automatically pick up the new file.
#[cfg(feature = "local_fs")]
pub fn write_tab_config(config: &TabConfig, dir: &Path, base_name: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = crate::user_config::find_unused_toml_path(dir, base_name);
    let toml_string = toml::to_string_pretty(config)?;
    std::fs::write(&path, toml_string)?;
    Ok(path)
}

/// Builds a `TabConfig` from a live tab's pane tree snapshot.
///
/// Walks the `PaneNodeSnapshot` recursively and produces a flat `[[panes]]`
/// array. Non-terminal leaves (code, settings, etc.) are replaced
/// with empty terminal panes to preserve the spatial layout.
pub fn tab_config_from_pane_snapshot(
    snapshot: &PaneNodeSnapshot,
    custom_title: Option<String>,
    color: Option<AnsiColorIdentifier>,
) -> TabConfig {
    let mut panes = Vec::new();
    let mut counter: usize = 0;
    snapshot_to_flat_panes(snapshot, &mut panes, &mut counter);

    TabConfig {
        name: "My Tab Config".to_string(),
        title: custom_title,
        color,
        panes,
        params: HashMap::new(),
        source_path: None,
    }
}

/// Recursively converts a `PaneNodeSnapshot` into flat `TabConfigPaneNode` entries.
/// Returns the ID assigned to the root of this subtree.
fn snapshot_to_flat_panes(
    snapshot: &PaneNodeSnapshot,
    panes: &mut Vec<TabConfigPaneNode>,
    counter: &mut usize,
) -> String {
    match snapshot {
        PaneNodeSnapshot::Branch(BranchSnapshot {
            direction,
            children,
        }) => {
            *counter += 1;
            let my_id = format!("p{counter}");

            // Record where this subtree starts so we can insert the split node
            // before all of its descendants (root-first ordering).
            let insert_pos = panes.len();

            // Recurse into children first to collect their IDs.
            let child_ids: Vec<String> = children
                .iter()
                .map(|(_, child)| snapshot_to_flat_panes(child, panes, counter))
                .collect();

            let split_direction = match direction {
                crate::app_state::SplitDirection::Horizontal => SplitDirection::Horizontal,
                crate::app_state::SplitDirection::Vertical => SplitDirection::Vertical,
            };
            panes.insert(
                insert_pos,
                TabConfigPaneNode {
                    id: my_id.clone(),
                    pane_type: None,
                    split: Some(split_direction),
                    children: Some(child_ids),
                    is_focused: None,
                    directory: None,
                    commands: None,
                    shell: None,
                },
            );

            my_id
        }
        PaneNodeSnapshot::Leaf(LeafSnapshot {
            is_focused,
            custom_vertical_tabs_title: _,
            contents,
        }) => {
            *counter += 1;
            let my_id = format!("p{counter}");

            let (directory, pane_type) = match contents {
                LeafContents::Terminal(terminal) => {
                    (terminal.cwd.clone(), TabConfigPaneType::Terminal)
                }
                // Non-terminal panes become empty terminal panes to preserve layout.
                _ => (None, TabConfigPaneType::Terminal),
            };

            panes.push(TabConfigPaneNode {
                id: my_id.clone(),
                pane_type: Some(pane_type),
                split: None,
                children: None,
                is_focused: if *is_focused { Some(true) } else { None },
                directory,
                commands: None,
                shell: None,
            });

            my_id
        }
    }
}

#[cfg(test)]
#[path = "session_config_tests.rs"]
mod tests;
