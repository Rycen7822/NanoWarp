-- Initial local schema for NanoWarp. This does not import Warp databases.
-- Add migrations for future changes after release; do not rewrite a shipped baseline.

CREATE TABLE windows (
    id INTEGER PRIMARY KEY NOT NULL,
    active_tab_index INTEGER NOT NULL CHECK (active_tab_index >= 0),
    window_width FLOAT CHECK (window_width >= 0),
    window_height FLOAT CHECK (window_height >= 0),
    origin_x FLOAT,
    origin_y FLOAT,
    quake_mode BOOLEAN NOT NULL DEFAULT FALSE,
    universal_search_width FLOAT CHECK (universal_search_width >= 0),
    fullscreen_state INTEGER NOT NULL DEFAULT 0,
    vertical_tabs_panel_open BOOLEAN,
    CONSTRAINT Bound_integrity CHECK (
        COALESCE(window_width, window_height, origin_x, origin_y) IS NOT NULL
        OR COALESCE(window_width, window_height, origin_x, origin_y) IS NULL
    )
);

CREATE TABLE app (
    id INTEGER PRIMARY KEY,
    active_window_id INTEGER REFERENCES windows(id)
);

CREATE TABLE tab_groups (
    id INTEGER PRIMARY KEY NOT NULL,
    window_id INTEGER NOT NULL,
    name TEXT,
    color TEXT,
    collapsed BOOLEAN NOT NULL,
    pinned BOOLEAN NOT NULL DEFAULT FALSE,
    FOREIGN KEY (window_id) REFERENCES windows(id)
);

CREATE TABLE tabs (
    id INTEGER PRIMARY KEY NOT NULL,
    window_id INTEGER NOT NULL,
    custom_title TEXT,
    color TEXT,
    tab_group_id INTEGER REFERENCES tab_groups(id),
    pinned BOOLEAN NOT NULL DEFAULT FALSE,
    FOREIGN KEY (window_id) REFERENCES windows(id)
);

CREATE TABLE pane_nodes (
    id INTEGER PRIMARY KEY NOT NULL,
    tab_id INTEGER NOT NULL REFERENCES tabs(id),
    parent_pane_node_id INTEGER REFERENCES pane_nodes(id),
    flex FLOAT,
    is_leaf BOOLEAN NOT NULL,
    CONSTRAINT root_or_has_parent CHECK (
        parent_pane_node_id IS NULL AND flex IS NULL
        OR parent_pane_node_id IS NOT NULL AND flex IS NOT NULL
    )
);

CREATE TABLE pane_branches (
    id INTEGER PRIMARY KEY NOT NULL,
    pane_node_id INTEGER NOT NULL UNIQUE REFERENCES pane_nodes(id),
    horizontal BOOLEAN NOT NULL
);

CREATE TABLE pane_leaves (
    pane_node_id INTEGER NOT NULL UNIQUE REFERENCES pane_nodes(id),
    kind TEXT NOT NULL,
    is_focused BOOLEAN NOT NULL DEFAULT FALSE,
    custom_vertical_tabs_title TEXT,
    PRIMARY KEY (pane_node_id, kind)
);

CREATE TABLE terminal_panes (
    id INTEGER PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'terminal' CHECK (kind = 'terminal'),
    uuid BLOB NOT NULL UNIQUE,
    cwd TEXT,
    is_active BOOLEAN NOT NULL DEFAULT FALSE,
    shell_launch_data TEXT,
    FOREIGN KEY (id, kind) REFERENCES pane_leaves(pane_node_id, kind)
);

CREATE TABLE notebook_panes (
    id INTEGER PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'notebook' CHECK (kind = 'notebook'),
    local_path BLOB,
    FOREIGN KEY (id, kind) REFERENCES pane_leaves(pane_node_id, kind)
);

CREATE TABLE code_panes (
    id INTEGER PRIMARY KEY NOT NULL,
    active_tab_index INTEGER NOT NULL DEFAULT 0,
    source_data TEXT
);

CREATE TABLE code_pane_tabs (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    code_pane_id INTEGER NOT NULL,
    tab_index INTEGER NOT NULL,
    local_path BLOB,
    FOREIGN KEY (code_pane_id) REFERENCES code_panes(id) ON DELETE CASCADE,
    UNIQUE (code_pane_id, tab_index)
);

CREATE TABLE settings_panes (
    id INTEGER PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL DEFAULT 'settings' CHECK (kind = 'settings'),
    current_page TEXT NOT NULL DEFAULT 'Appearance',
    FOREIGN KEY (id, kind) REFERENCES pane_leaves(pane_node_id, kind)
);

CREATE TABLE blocks (
    id INTEGER PRIMARY KEY,
    pane_leaf_uuid BLOB NOT NULL,
    stylized_command BLOB NOT NULL,
    stylized_output BLOB NOT NULL,
    pwd TEXT,
    git_branch TEXT,
    virtual_env TEXT,
    conda_env TEXT,
    exit_code INTEGER NOT NULL,
    did_execute BOOLEAN NOT NULL,
    completed_ts DATETIME,
    start_ts DATETIME,
    ps1 TEXT,
    honor_ps1 BOOLEAN NOT NULL DEFAULT FALSE,
    shell TEXT,
    user TEXT,
    host TEXT,
    is_background BOOLEAN NOT NULL DEFAULT FALSE,
    rprompt TEXT,
    prompt_snapshot TEXT,
    block_id TEXT NOT NULL DEFAULT '',
    is_local BOOLEAN,
    git_branch_name TEXT
);

CREATE TABLE commands (
    id INTEGER NOT NULL PRIMARY KEY,
    command TEXT NOT NULL,
    exit_code INTEGER,
    start_ts DATETIME,
    completed_ts DATETIME,
    pwd TEXT,
    shell TEXT,
    username TEXT,
    hostname TEXT,
    session_id BIGINTEGER,
    git_branch TEXT
);

CREATE TABLE workspace_metadata (
    id INTEGER NOT NULL PRIMARY KEY,
    repo_path TEXT NOT NULL UNIQUE,
    navigated_ts DATETIME,
    modified_ts DATETIME,
    queried_ts DATETIME
);

CREATE TABLE ignored_suggestions (
    id INTEGER NOT NULL PRIMARY KEY,
    suggestion TEXT NOT NULL,
    suggestion_type TEXT NOT NULL,
    UNIQUE (suggestion, suggestion_type)
);
