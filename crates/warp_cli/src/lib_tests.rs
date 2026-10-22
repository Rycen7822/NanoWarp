use std::ffi::OsString;

use clap::Parser;

use super::*;
use crate::agent::Harness;

#[test]
fn identifies_worker_subcommands() {
    assert!(is_worker_invocation("minidump-server"));
    #[cfg(unix)]
    assert!(is_worker_invocation(&terminal_server_subcommand()));
    assert!(!is_worker_invocation("--prompt"));
    assert!(Args::try_parse_from(["nanowarp", "agent", "run", "--prompt", "test"]).is_err());
}

#[test]
fn oz_and_warp_env_var_constants_name_the_same_variables() {
    for (oz_name, warp_name) in [
        (OZ_RUN_ID_ENV, WARP_RUN_ID_ENV),
        (OZ_PARENT_RUN_ID_ENV, WARP_PARENT_RUN_ID_ENV),
        (OZ_CLI_ENV, WARP_CLI_ENV),
        (OZ_HARNESS_ENV, WARP_HARNESS_ENV),
    ] {
        let suffix = oz_name
            .strip_prefix("OZ_")
            .unwrap_or_else(|| panic!("{oz_name} should be OZ_-prefixed"));
        assert_eq!(
            warp_name,
            format!("WARP_{suffix}"),
            "{warp_name} does not correspond to {oz_name}"
        );
    }
}

#[test]
#[serial_test::serial]
fn help_hides_api_key_env_value() {
    const API_KEY: &str = "warp-cli-test-api-key-NOT-REAL";

    let previous_api_key = set_env_var("WARP_API_KEY", API_KEY);

    let mut command = <Args as clap::CommandFactory>::command();
    let help = command.render_long_help().to_string();
    let args = Args::try_parse_from(["nanowarp"]).expect("API key env var should parse");

    restore_env_var("WARP_API_KEY", previous_api_key);

    assert!(
        help.contains("WARP_API_KEY"),
        "help should identify the API key environment variable:\n{help}"
    );
    assert!(
        !help.contains(API_KEY),
        "help should not reveal the API key environment value:\n{help}"
    );
    assert_eq!(args.api_key().map(String::as_str), Some(API_KEY));
}

#[test]
fn harness_parse_orchestration_harness_accepts_aliases() {
    assert_eq!(
        Harness::parse_orchestration_harness("pi"),
        Some(Harness::Pi)
    );
    assert_eq!(
        Harness::parse_orchestration_harness("claude-code"),
        Some(Harness::Claude)
    );
    assert_eq!(
        Harness::parse_orchestration_harness("open_code"),
        Some(Harness::OpenCode)
    );
}

#[test]
fn harness_parse_local_child_harness_rejects_oz() {
    assert_eq!(Harness::parse_local_child_harness("oz"), None);
    assert_eq!(Harness::parse_local_child_harness("pi"), None);
    assert_eq!(
        Harness::parse_local_child_harness("opencode"),
        Some(Harness::OpenCode)
    );
}

#[test]
fn harness_parse_orchestration_harness_accepts_codex() {
    assert_eq!(
        Harness::parse_orchestration_harness("codex"),
        Some(Harness::Codex)
    );
}

#[test]
fn harness_parse_local_child_harness_accepts_codex() {
    assert_eq!(
        Harness::parse_local_child_harness("codex"),
        Some(Harness::Codex)
    );
}

#[test]
#[serial_test::serial]
fn hidden_server_overrides_parse_from_env() {
    let previous_server_root = set_env_var(SERVER_ROOT_URL_OVERRIDE_ENV, "http://localhost:8080");
    let previous_ws = set_env_var(WS_SERVER_URL_OVERRIDE_ENV, "ws://localhost:8082/graphql/v2");
    let previous_session_sharing = set_env_var(
        SESSION_SHARING_SERVER_URL_OVERRIDE_ENV,
        "ws://127.0.0.1:8081",
    );

    let args = Args::try_parse_from(["nanowarp"]).unwrap();

    restore_env_var(SERVER_ROOT_URL_OVERRIDE_ENV, previous_server_root);
    restore_env_var(WS_SERVER_URL_OVERRIDE_ENV, previous_ws);
    restore_env_var(
        SESSION_SHARING_SERVER_URL_OVERRIDE_ENV,
        previous_session_sharing,
    );

    assert_eq!(args.server_root_url(), Some("http://localhost:8080"));
    assert_eq!(args.ws_server_url(), Some("ws://localhost:8082/graphql/v2"));
    assert_eq!(
        args.session_sharing_server_url(),
        Some("ws://127.0.0.1:8081")
    );
}

fn set_env_var(name: &str, value: &str) -> Option<OsString> {
    let previous = std::env::var_os(name);
    // Safety: tests that mutate process environment are marked `serial` so we
    // do not race with other environment readers/writers in this crate.
    unsafe { std::env::set_var(name, value) };
    previous
}

fn restore_env_var(name: &str, previous: Option<OsString>) {
    match previous {
        // Safety: tests that mutate process environment are marked `serial` so
        // we do not race with other environment readers/writers in this crate.
        Some(value) => unsafe { std::env::set_var(name, value) },
        // Safety: tests that mutate process environment are marked `serial` so
        // we do not race with other environment readers/writers in this crate.
        None => unsafe { std::env::remove_var(name) },
    }
}
