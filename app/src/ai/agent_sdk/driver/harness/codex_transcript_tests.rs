use std::fs;

use tempfile::TempDir;
use uuid::Uuid;

use super::*;

/// Walk `sessions_root` for `session_id`'s rollout and assemble an envelope.
/// Minimal SessionMeta line in the same shape codex writes (codex
/// `protocol/src/protocol.rs::RolloutItem`): `{type, payload}`.
fn session_meta_line(uuid: Uuid, cwd: &str, timestamp: &str, cli_version: &str) -> String {
    serde_json::json!({
        "type": "session_meta",
        "payload": {
            "id": uuid.to_string(),
            "timestamp": timestamp,
            "cwd": cwd,
            "originator": "test",
            "cli_version": cli_version,
        },
    })
    .to_string()
}

#[test]
#[serial_test::serial]
fn codex_sessions_root_honors_codex_home_env() {
    let tmp = TempDir::new().unwrap();
    let prev = std::env::var(CODEX_HOME_ENV).ok();
    // TODO: Audit that the environment access only happens in single-threaded code.
    unsafe { std::env::set_var(CODEX_HOME_ENV, tmp.path()) };

    let root = codex_sessions_root().unwrap();

    match prev {
        // TODO: Audit that the environment access only happens in single-threaded code.
        Some(v) => unsafe { std::env::set_var(CODEX_HOME_ENV, v) },
        // TODO: Audit that the environment access only happens in single-threaded code.
        None => unsafe { std::env::remove_var(CODEX_HOME_ENV) },
    }
    assert_eq!(root, tmp.path().join(CODEX_SESSIONS_SUBDIR));
}

#[test]
fn write_envelope_uses_session_meta_timestamp_for_path() {
    let tmp = TempDir::new().unwrap();
    let uuid = Uuid::new_v4();
    let envelope = CodexTranscriptEnvelope {
        cwd: "/work".into(),
        session_id: uuid,
        codex_version: Some("0.55.0".to_string()),
        session_start_timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-04-30T01:54:20.000Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ),
        entries: vec![
            serde_json::from_str::<serde_json::Value>(&session_meta_line(
                uuid,
                "/work",
                "2026-04-30T01:54:20.000Z",
                "0.55.0",
            ))
            .unwrap(),
        ],
    };

    let path = write_envelope(&envelope, tmp.path()).unwrap();

    let expected = tmp
        .path()
        .join("2026")
        .join("04")
        .join("30")
        .join(format!("rollout-2026-04-30T01-54-20-{uuid}.jsonl"));
    assert_eq!(path, expected);
    assert!(path.exists());
}

#[test]
fn write_envelope_round_trip_preserves_entries() {
    let tmp = TempDir::new().unwrap();
    let uuid = Uuid::new_v4();
    let entries = vec![
        serde_json::from_str::<serde_json::Value>(&session_meta_line(
            uuid,
            "/work",
            "2026-04-30T01:54:20.000Z",
            "0.55.0",
        ))
        .unwrap(),
        serde_json::json!({"type": "event_msg", "payload": {"x": 1}}),
    ];
    let original = CodexTranscriptEnvelope {
        cwd: "/work".into(),
        session_id: uuid,
        codex_version: Some("0.55.0".to_string()),
        session_start_timestamp: Some(
            chrono::DateTime::parse_from_rfc3339("2026-04-30T01:54:20.000Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ),
        entries: entries.clone(),
    };

    let path = write_envelope(&original, tmp.path()).unwrap();
    assert_eq!(fs::read(path).unwrap(), entries_to_jsonl(&entries).unwrap());
}

#[test]
fn write_envelope_falls_back_to_today_when_timestamp_missing() {
    let tmp = TempDir::new().unwrap();
    let uuid = Uuid::new_v4();
    // No SessionMeta first line — just one event entry.
    let envelope = CodexTranscriptEnvelope {
        cwd: "/work".into(),
        session_id: uuid,
        codex_version: None,
        session_start_timestamp: None,
        entries: vec![serde_json::json!({"type": "event_msg"})],
    };

    let path = write_envelope(&envelope, tmp.path()).unwrap();

    // The written file stays under sessions_root and identifies the session.
    assert!(path.starts_with(tmp.path()));
    assert!(path.exists());
    assert!(
        path.file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with(&format!("-{uuid}.jsonl"))
    );
}
