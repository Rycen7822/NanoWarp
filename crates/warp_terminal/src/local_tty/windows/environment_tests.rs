use super::*;

#[test]
fn wsl_env_allowlist_includes_terminal_environment() {
    let wslenv = wsl_env_allowlist(false, None, None)
        .to_string_lossy()
        .into_owned();

    assert_eq!(
        wslenv.split(':').collect::<Vec<_>>(),
        vec![
            format!("{HONOR_PS1_NAME}/u"),
            format!("{USE_SSH_WRAPPER_NAME}/u"),
            format!("{SSH_REUSE_CONTROL_MASTER_NAME}/u"),
            format!("{SHELL_DEBUG_MODE_NAME}/u"),
            format!("{TERM_PROGRAM_NAME}/u"),
            format!("{IS_LOCAL_SESSION_NAME}/u"),
            format!("{SSH_SOCKET_DIR}/u"),
            format!("{WARP_CLIENT_VERSION_ENV}/u"),
            format!("{TERMINAL_SESSION_UUID_ENV}/u"),
            format!("{FOCUS_URL_ENV}/u"),
            format!("{PROMPT_NODE_VERSION_ENABLED_NAME}/u"),
        ],
    );
}

#[test]
fn wsl_env_allowlist_includes_initial_working_dir_when_requested() {
    let wslenv = wsl_env_allowlist(true, None, None)
        .to_string_lossy()
        .into_owned();

    assert_eq!(
        wslenv.split(':').collect::<Vec<_>>(),
        vec![
            format!("{HONOR_PS1_NAME}/u"),
            format!("{USE_SSH_WRAPPER_NAME}/u"),
            format!("{SSH_REUSE_CONTROL_MASTER_NAME}/u"),
            format!("{SHELL_DEBUG_MODE_NAME}/u"),
            format!("{TERM_PROGRAM_NAME}/u"),
            format!("{IS_LOCAL_SESSION_NAME}/u"),
            format!("{SSH_SOCKET_DIR}/u"),
            format!("{WARP_CLIENT_VERSION_ENV}/u"),
            format!("{TERMINAL_SESSION_UUID_ENV}/u"),
            format!("{FOCUS_URL_ENV}/u"),
            format!("{PROMPT_NODE_VERSION_ENABLED_NAME}/u"),
            format!("{INITIAL_WORKING_DIR_NAME}/pu"),
        ],
    );
}

#[test]
fn wsl_env_allowlist_keeps_process_overrides_alongside_registry_values() {
    let registry = OsStr::new("COLORTERM/u:TERM_PROGRAM/u");
    let process = OsStr::new("ZDOTDIR/u:HISTFILE/u");
    let wslenv = wsl_env_allowlist(false, Some(registry), Some(process));
    assert!(
        wslenv
            .to_string_lossy()
            .ends_with(":COLORTERM/u:TERM_PROGRAM/u:ZDOTDIR/u:HISTFILE/u")
    );
    let same = wsl_env_allowlist(false, Some(registry), Some(registry));
    assert_eq!(same.to_string_lossy().matches("COLORTERM/u").count(), 1);
}
