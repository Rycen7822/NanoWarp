// @generated automatically by Diesel CLI.

diesel::table! {
    app (id) {
        id -> Nullable<Integer>,
        active_window_id -> Nullable<Integer>,
    }
}

diesel::table! {
    blocks (id) {
        id -> Nullable<Integer>,
        pane_leaf_uuid -> Binary,
        stylized_command -> Binary,
        stylized_output -> Binary,
        pwd -> Nullable<Text>,
        git_branch -> Nullable<Text>,
        virtual_env -> Nullable<Text>,
        conda_env -> Nullable<Text>,
        exit_code -> Integer,
        did_execute -> Bool,
        completed_ts -> Nullable<Timestamp>,
        start_ts -> Nullable<Timestamp>,
        ps1 -> Nullable<Text>,
        honor_ps1 -> Bool,
        shell -> Nullable<Text>,
        user -> Nullable<Text>,
        host -> Nullable<Text>,
        is_background -> Bool,
        rprompt -> Nullable<Text>,
        prompt_snapshot -> Nullable<Text>,
        block_id -> Text,
        is_local -> Nullable<Bool>,
        git_branch_name -> Nullable<Text>,
    }
}

diesel::table! {
    code_pane_tabs (id) {
        id -> Integer,
        code_pane_id -> Integer,
        tab_index -> Integer,
        local_path -> Nullable<Binary>,
    }
}

diesel::table! {
    code_panes (id) {
        id -> Integer,
        active_tab_index -> Integer,
        source_data -> Nullable<Text>,
    }
}

diesel::table! {
    code_review_panes (id) {
        id -> Integer,
        kind -> Text,
        terminal_uuid -> Binary,
        repo_path -> Text,
    }
}

diesel::table! {
    commands (id) {
        id -> Integer,
        command -> Text,
        exit_code -> Nullable<Integer>,
        start_ts -> Nullable<Timestamp>,
        completed_ts -> Nullable<Timestamp>,
        pwd -> Nullable<Text>,
        shell -> Nullable<Text>,
        username -> Nullable<Text>,
        hostname -> Nullable<Text>,
        session_id -> Nullable<BigInt>,
        git_branch -> Nullable<Text>,
    }
}

diesel::table! {
    ignored_suggestions (id) {
        id -> Integer,
        suggestion -> Text,
        suggestion_type -> Text,
    }
}

diesel::table! {
    notebook_panes (id) {
        id -> Integer,
        kind -> Text,
        local_path -> Nullable<Binary>,
    }
}

diesel::table! {
    pane_branches (id) {
        id -> Integer,
        pane_node_id -> Integer,
        horizontal -> Bool,
    }
}

diesel::table! {
    pane_leaves (pane_node_id, kind) {
        pane_node_id -> Integer,
        kind -> Text,
        is_focused -> Bool,
        custom_vertical_tabs_title -> Nullable<Text>,
    }
}

diesel::table! {
    pane_nodes (id) {
        id -> Integer,
        tab_id -> Integer,
        parent_pane_node_id -> Nullable<Integer>,
        flex -> Nullable<Float>,
        is_leaf -> Bool,
    }
}

diesel::table! {
    panels (id) {
        id -> Integer,
        tab_id -> Integer,
        right_panel -> Nullable<Text>,
    }
}

diesel::table! {
    settings_panes (id) {
        id -> Integer,
        kind -> Text,
        current_page -> Text,
    }
}

diesel::table! {
    tab_groups (id) {
        id -> Integer,
        window_id -> Integer,
        name -> Nullable<Text>,
        color -> Nullable<Text>,
        collapsed -> Bool,
        pinned -> Bool,
    }
}

diesel::table! {
    tabs (id) {
        id -> Integer,
        window_id -> Integer,
        custom_title -> Nullable<Text>,
        color -> Nullable<Text>,
        tab_group_id -> Nullable<Integer>,
        pinned -> Bool,
    }
}

diesel::table! {
    terminal_panes (id) {
        id -> Integer,
        kind -> Text,
        uuid -> Binary,
        cwd -> Nullable<Text>,
        is_active -> Bool,
        shell_launch_data -> Nullable<Text>,
    }
}

diesel::table! {
    windows (id) {
        id -> Integer,
        active_tab_index -> Integer,
        window_width -> Nullable<Float>,
        window_height -> Nullable<Float>,
        origin_x -> Nullable<Float>,
        origin_y -> Nullable<Float>,
        quake_mode -> Bool,
        universal_search_width -> Nullable<Float>,
        fullscreen_state -> Integer,
        vertical_tabs_panel_open -> Nullable<Bool>,
    }
}

diesel::table! {
    workspace_metadata (id) {
        id -> Integer,
        repo_path -> Text,
        navigated_ts -> Nullable<Timestamp>,
        modified_ts -> Nullable<Timestamp>,
        queried_ts -> Nullable<Timestamp>,
    }
}

diesel::joinable!(app -> windows (active_window_id));
diesel::joinable!(code_pane_tabs -> code_panes (code_pane_id));
diesel::joinable!(pane_branches -> pane_nodes (pane_node_id));
diesel::joinable!(pane_leaves -> pane_nodes (pane_node_id));
diesel::joinable!(pane_nodes -> tabs (tab_id));
diesel::joinable!(panels -> tabs (tab_id));
diesel::joinable!(tab_groups -> windows (window_id));
diesel::joinable!(tabs -> tab_groups (tab_group_id));
diesel::joinable!(tabs -> windows (window_id));

diesel::allow_tables_to_appear_in_same_query!(
    app,
    pane_branches,
    pane_leaves,
    pane_nodes,
    panels,
    tab_groups,
    tabs,
    windows,
);
diesel::allow_tables_to_appear_in_same_query!(code_pane_tabs, code_panes,);
