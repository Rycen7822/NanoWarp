use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mockito::Matcher;
use warp_graphql::ai::PlatformErrorCode;
use warp_graphql::platform_error::{PlatformErrorInfo, PlatformErrorMessageFormat};
use warp_server_client::base_client::TEAM_UID_HEADER;

use super::super::ServerApi;
use super::{
    AgentSource, AmbientAgentTaskState, Artifact, ArtifactDownloadResponse, ExecutionLocation,
    ListRunsResponse, TaskListFilter, TaskStatusUpdate, agent_task_status_message_input,
    build_list_agent_runs_url,
};
use crate::server::ids::ServerId;
use crate::server::team_scope::RequestTeamScope;
use crate::workspaces::user_workspaces::{TeamContextForOperation, TeamlessScopeForTest};

#[test]
fn task_status_message_input_preserves_full_platform_error() {
    let input = agent_task_status_message_input(TaskStatusUpdate {
        message: "Repository access failed.".to_string(),
        error_code: Some(PlatformErrorCode::ResourceUnavailable),
        platform_error: Some(Box::new(PlatformErrorInfo {
            error_message: Some("GitHub is temporarily unavailable.".to_string()),
            code: PlatformErrorCode::ResourceUnavailable,
            http_status: Some(503),
            user_facing_messages: std::collections::BTreeMap::from([(
                PlatformErrorMessageFormat::PlainText,
                "GitHub is temporarily unavailable.".to_string(),
            )]),
            detail: Some("Repository access could not be resolved.".to_string()),
            retryable: true,
            is_user_error: Some(false),
            metadata: std::collections::BTreeMap::from([(
                "provider".to_string(),
                "github".to_string(),
            )]),
            debug: Some("request-id=dogfood-only".to_string()),
            metrics_category: Some("dependency_unavailable".to_string()),
            trace_id: Some("0123456789abcdef".to_string()),
        })),
    });
    let error = input.error.unwrap();

    assert_eq!(
        error.error_message.as_deref(),
        Some("GitHub is temporarily unavailable.")
    );
    assert_eq!(error.code, PlatformErrorCode::ResourceUnavailable);
    assert_eq!(error.http_status, Some(503));
    let messages = error.user_facing_messages.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].format, PlatformErrorMessageFormat::PlainText);
    assert_eq!(messages[0].message, "GitHub is temporarily unavailable.");
    assert_eq!(
        error.detail.as_deref(),
        Some("Repository access could not be resolved.")
    );
    assert!(error.retryable);
    assert_eq!(error.is_user_error, Some(false));
    assert_eq!(error.metadata[0].key, "provider");
    assert_eq!(error.metadata[0].value, "github");
    assert_eq!(error.debug.as_deref(), Some("request-id=dogfood-only"));
    assert_eq!(
        error.metrics_category.as_deref(),
        Some("dependency_unavailable")
    );
    assert_eq!(error.trace_id.as_deref(), Some("0123456789abcdef"));
}

#[test]
fn list_agent_runs_sends_selected_team_header() {
    let team_uid = ServerId::from(123);
    let scope = RequestTeamScope::from_scope(&TeamContextForOperation::new_for_test(team_uid));
    let _request = {
        let mut server = warp_core::channel::ChannelState::mock_server();
        server
            .mock("GET", "/api/v1/agent/runs")
            .match_header(TEAM_UID_HEADER, team_uid.to_string().as_str())
            .with_status(200)
            .with_body(r#"{"runs":[]}"#)
            .create()
    };
    let server_api = ServerApi::new_for_test();

    block_on(
        server_api.get_public_api_with_team_scope::<serde_json::Value>("agent/runs", Some(scope)),
    )
    .unwrap();
}

#[test]
fn list_agent_runs_omits_team_header_for_teamless_scope() {
    let scope = RequestTeamScope::from_scope(&TeamlessScopeForTest);
    let _request = {
        let mut server = warp_core::channel::ChannelState::mock_server();
        server
            .mock("GET", "/api/v1/agent/runs")
            .match_header(TEAM_UID_HEADER, Matcher::Missing)
            .with_status(200)
            .with_body(r#"{"runs":[]}"#)
            .create()
    };
    let server_api = ServerApi::new_for_test();

    block_on(
        server_api.get_public_api_with_team_scope::<serde_json::Value>("agent/runs", Some(scope)),
    )
    .unwrap();
}

#[test]
fn test_deserialize_file_artifact_download_response() {
    let json = r#"{
        "artifact_uid": "artifact-123",
        "artifact_type": "FILE",
        "created_at": "2024-01-15T10:30:00Z",
        "data": {
            "download_url": "https://storage.example.com/report.txt",
            "expires_at": "2024-01-15T11:30:00Z",
            "content_type": "text/plain",
            "filepath": "outputs/report.txt",
            "filename": "report.txt",
            "description": "daily summary",
            "size_bytes": 42
        }
    }"#;

    let artifact: ArtifactDownloadResponse = serde_json::from_str(json).unwrap();

    let ArtifactDownloadResponse::File { common, data } = artifact;
    assert_eq!(common.artifact_uid, "artifact-123");
    assert_eq!(common.created_at.to_rfc3339(), "2024-01-15T10:30:00+00:00");
    assert_eq!(data.download_url, "https://storage.example.com/report.txt");
    assert_eq!(data.expires_at.to_rfc3339(), "2024-01-15T11:30:00+00:00");
    assert_eq!(data.content_type, "text/plain");
    assert_eq!(data.filepath, "outputs/report.txt");
    assert_eq!(data.filename, "report.txt");
    assert_eq!(data.description.as_deref(), Some("daily summary"));
    assert_eq!(data.size_bytes, Some(42));
}

#[test]
fn test_deserialize_pull_request_artifact() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://github.com/org/repo/pull/42",
            "branch": "feature-branch"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::PullRequest {
        url,
        branch,
        repo,
        number,
    } = &artifact
    else {
        panic!("expected PullRequest artifact");
    };
    assert_eq!(url, "https://github.com/org/repo/pull/42");
    assert_eq!(branch, "feature-branch");
    assert_eq!(*repo, Some("repo".to_string()));
    assert_eq!(*number, Some(42));
}

#[test]
fn test_deserialize_pull_request_non_github_url() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://gitlab.com/org/repo/merge_requests/42",
            "branch": "feature-branch"
        }
    }"#;

    let artifact: Artifact = serde_json::from_str(json).unwrap();

    let Artifact::PullRequest { repo, number, .. } = &artifact else {
        panic!("expected PullRequest artifact");
    };
    assert_eq!(*repo, None);
    assert_eq!(*number, None);
}

#[test]
fn test_deserialize_list_tasks_response_with_artifacts() {
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Test Task",
                "state": "SUCCEEDED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true,
                "artifacts": [
                    {
                        "created_at": "2024-01-15T10:20:00Z",
                        "artifact_type": "PLAN",
                        "data": {
                            "document_uid": "doc-1",
                            "notebook_uid": "xyz1234567890123456789",
                            "title": "Plan Title"
                        }
                    },
                    {
                        "created_at": "2024-01-15T10:25:00Z",
                        "artifact_type": "PULL_REQUEST",
                        "data": {
                            "url": "https://github.com/org/repo/pull/1",
                            "branch": "main"
                        }
                    },
                    {
                        "created_at": "2024-01-15T10:27:00Z",
                        "artifact_type": "FILE",
                        "data": {
                            "artifact_uid": "artifact-file-1",
                            "filepath": "outputs/report.txt",
                            "filename": "report.txt",
                            "mime_type": "text/plain",
                            "description": "Daily summary",
                            "size_bytes": 42
                        }
                    }
                ]
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    assert_eq!(response.runs.len(), 1);
    let task = &response.runs[0];
    assert_eq!(
        task.task_id.to_string(),
        "550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(task.artifacts.len(), 2);

    // Check the retained PullRequest artifact
    let Artifact::PullRequest {
        url,
        branch,
        repo,
        number,
        ..
    } = &task.artifacts[0]
    else {
        panic!("expected PullRequest artifact");
    };
    assert_eq!(url, "https://github.com/org/repo/pull/1");
    assert_eq!(branch, "main");
    assert_eq!(*repo, Some("repo".to_string()));
    assert_eq!(*number, Some(1));

    let Artifact::File {
        artifact_uid,
        filepath,
        filename,
        mime_type,
        description,
        size_bytes,
    } = &task.artifacts[1]
    else {
        panic!("expected File artifact");
    };
    assert_eq!(artifact_uid, "artifact-file-1");
    assert_eq!(filepath, "outputs/report.txt");
    assert_eq!(filename, "report.txt");
    assert_eq!(mime_type, "text/plain");
    assert_eq!(*description, Some("Daily summary".to_string()));
    assert_eq!(*size_bytes, Some(42));
}

#[test]
fn test_deserialize_list_tasks_response_empty_artifacts() {
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440001",
                "title": "Test Task",
                "state": "INPROGRESS",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true,
                "artifacts": []
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    assert_eq!(response.runs.len(), 1);
    assert!(response.runs[0].artifacts.is_empty());
}

#[test]
fn test_deserialize_list_tasks_response_missing_artifacts_field() {
    // Server may not include artifacts field at all for older responses
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440002",
                "title": "Test Task",
                "state": "QUEUED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    assert_eq!(response.runs.len(), 1);
    assert!(response.runs[0].artifacts.is_empty());
}

#[test]
fn test_deserialize_artifacts_skips_invalid_items() {
    // deserialize_artifacts should skip invalid items and keep valid ones
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Test Task",
                "state": "SUCCEEDED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true,
                "artifacts": [
                    {
                        "created_at": "2024-01-15T10:20:00Z",
                        "artifact_type": "PLAN",
                        "data": {
                            "document_uid": "valid-doc",
                            "notebook_uid": "validnotebook123456789",
                            "title": "Valid Plan"
                        }
                    },
                    {
                        "created_at": "2024-01-15T10:25:00Z",
                        "artifact_type": "UNKNOWN_TYPE",
                        "data": {
                            "some_field": "value"
                        }
                    },
                    {
                        "created_at": "2024-01-15T10:30:00Z",
                        "artifact_type": "PULL_REQUEST",
                        "data": {
                            "url": "https://github.com/org/repo/pull/1",
                            "branch": "main"
                        }
                    }
                ]
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    assert_eq!(response.runs.len(), 1);
    // Retired and unknown artifacts are skipped; ordinary artifacts remain.
    assert_eq!(response.runs[0].artifacts.len(), 1);
    assert!(matches!(
        response.runs[0].artifacts[0],
        Artifact::PullRequest { .. }
    ));
}

#[test]
fn test_deserialize_artifacts_all_invalid_returns_empty() {
    // When all artifacts are invalid, result should be empty vec
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Test Task",
                "state": "SUCCEEDED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true,
                "artifacts": [
                    {
                        "created_at": "2024-01-15T10:20:00Z",
                        "artifact_type": "UNKNOWN_TYPE",
                        "data": {}
                    }
                ]
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    assert_eq!(response.runs.len(), 1);
    assert!(response.runs[0].artifacts.is_empty());
}

#[test]
fn test_deserialize_artifact_missing_data_field() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PLAN"
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_invalid_pr_data() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "PULL_REQUEST",
        "data": {
            "url": "https://github.com/org/repo/pull/1"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("missing field"));
}

#[test]
fn test_deserialize_artifact_unknown_variant() {
    let json = r#"{
        "created_at": "2024-01-15T10:30:00Z",
        "artifact_type": "UNKNOWN_TYPE",
        "data": {
            "some_field": "value"
        }
    }"#;

    let result = serde_json::from_str::<Artifact>(json);
    assert!(result.is_err());
    let error_msg = result.unwrap_err().to_string();
    assert!(error_msg.contains("unknown variant"));
}

// ---------------------------------------------------------------------------------------------------------------------
//  Tests for resilient task list deserialization (skipping malformed tasks while tolerating unknown states)
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn test_deserialize_list_tasks_skips_invalid_task() {
    // One valid task and one invalid task (missing required field)
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Valid Task",
                "state": "SUCCEEDED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true
            },
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440001",
                "title": "Invalid Task",
                "state": "INPROGRESS"
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    // Should only have the valid task
    assert_eq!(response.runs.len(), 1);
    assert_eq!(
        response.runs[0].task_id.to_string(),
        "550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(response.runs[0].title, "Valid Task");
}

#[test]
fn test_deserialize_list_tasks_error_and_blocked_states() {
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Errored Task",
                "state": "ERROR",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": false
            },
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440001",
                "title": "Blocked Task",
                "state": "BLOCKED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": false
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.runs.len(), 2);
    assert_eq!(response.runs[0].state, AmbientAgentTaskState::Error);
    assert_eq!(response.runs[1].state, AmbientAgentTaskState::Blocked);
}

#[test]
fn test_deserialize_list_tasks_all_tasks_invalid_returns_empty() {
    // All tasks are missing required fields
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Missing State"
            },
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440001",
                "state": "SUCCEEDED"
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    // Should return empty list, not fail
    assert_eq!(response.runs.len(), 0);
}

#[test]
fn test_deserialize_list_tasks_invalid_state_enum() {
    // Task with an unknown state enum value
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Valid Task",
                "state": "SUCCEEDED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true
            },
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440001",
                "title": "Task with Invalid State",
                "state": "INVALID_STATE",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    // Unknown states should deserialize to AmbientAgentTaskState::Unknown.
    assert_eq!(response.runs.len(), 2);
    assert_eq!(response.runs[0].title, "Valid Task");
    assert_eq!(response.runs[1].title, "Task with Invalid State");
    assert_eq!(response.runs[1].state, AmbientAgentTaskState::Unknown);
}

#[test]
fn test_deserialize_list_tasks_corrupted_json_in_middle() {
    // Mix of valid and completely malformed JSON
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "First Valid Task",
                "state": "SUCCEEDED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true
            },
            {
                "task_id": 12345,
                "title": 999,
                "state": true
            },
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440002",
                "title": "Second Valid Task",
                "state": "INPROGRESS",
                "prompt": "test prompt 2",
                "created_at": "2024-01-15T11:00:00Z",
                "updated_at": "2024-01-15T11:30:00Z",
                "is_sandbox_running": false
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    // Should have both valid tasks, malformed one skipped
    assert_eq!(response.runs.len(), 2);
    assert_eq!(response.runs[0].title, "First Valid Task");
    assert_eq!(response.runs[1].title, "Second Valid Task");
}

#[test]
fn test_deserialize_list_runs_null_returns_empty() {
    let response: ListRunsResponse = serde_json::from_str(
        r#"{
            "as_of": "2026-10-20T20:19:11.750Z",
            "page_info": {"has_next_page": false},
            "runs": null,
            "team_uid": null
        }"#,
    )
    .unwrap();

    assert!(response.runs.is_empty());
}

#[test]
fn test_deserialize_list_runs_rejects_non_array() {
    assert!(serde_json::from_str::<ListRunsResponse>(r#"{"runs": {}}"#).is_err());
}

#[test]
fn test_deserialize_list_runs_requires_runs_field() {
    assert!(serde_json::from_str::<ListRunsResponse>(r#"{}"#).is_err());
}

#[test]
fn test_deserialize_list_tasks_empty_tasks_array() {
    // Empty tasks array should work fine
    let json = r#"{
        "runs": []
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.runs.len(), 0);
}

#[test]
fn test_deserialize_list_tasks_all_tasks_valid() {
    // Ensure we don't break the happy path
    let json = r#"{
        "runs": [
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440000",
                "title": "Task 1",
                "state": "SUCCEEDED",
                "prompt": "test prompt",
                "created_at": "2024-01-15T10:00:00Z",
                "updated_at": "2024-01-15T10:30:00Z",
                "is_sandbox_running": true
            },
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440001",
                "title": "Task 2",
                "state": "INPROGRESS",
                "prompt": "test prompt 2",
                "created_at": "2024-01-15T11:00:00Z",
                "updated_at": "2024-01-15T11:30:00Z",
                "is_sandbox_running": false
            },
            {
                "task_id": "550e8400-e29b-41d4-a716-446655440002",
                "title": "Task 3",
                "state": "FAILED",
                "prompt": "test prompt 3",
                "created_at": "2024-01-15T12:00:00Z",
                "updated_at": "2024-01-15T12:30:00Z",
                "is_sandbox_running": false
            }
        ]
    }"#;

    let response: ListRunsResponse = serde_json::from_str(json).unwrap();

    // All tasks should be present
    assert_eq!(response.runs.len(), 3);
    assert_eq!(response.runs[0].title, "Task 1");
    assert_eq!(response.runs[1].title, "Task 2");
    assert_eq!(response.runs[2].title, "Task 3");
}

// ---------------------------------------------------------------------------------------------------------------------
//  We test roundtripping serialize and deserialize since we use this for persisting artifacts for local conversations.
// ---------------------------------------------------------------------------------------------------------------------

#[test]
fn test_artifact_pr_serialize_deserialize_roundtrip() {
    let original = Artifact::PullRequest {
        url: "https://github.com/org/repo/pull/42".to_string(),
        branch: "feature-branch".to_string(),
        repo: Some("repo".to_string()),
        number: Some(42),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    // repo/number are re-derived from URL on deserialize, so should match
    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_file_serialize_deserialize_roundtrip() {
    let original = Artifact::File {
        artifact_uid: "artifact-file-1".to_string(),
        filepath: "outputs/report.txt".to_string(),
        filename: "report.txt".to_string(),
        mime_type: "text/plain".to_string(),
        description: Some("Daily summary".to_string()),
        size_bytes: Some(42),
    };

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Artifact = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn test_artifact_vec_serialize_deserialize_roundtrip() {
    let original = vec![
        Artifact::PullRequest {
            url: "https://github.com/org/repo/pull/1".to_string(),
            branch: "main".to_string(),
            repo: Some("repo".to_string()),
            number: Some(1),
        },
        Artifact::File {
            artifact_uid: "artifact-file-1".to_string(),
            filepath: "outputs/report.txt".to_string(),
            filename: "report.txt".to_string(),
            mime_type: "text/plain".to_string(),
            description: Some("Daily summary".to_string()),
            size_bytes: Some(42),
        },
    ];

    let serialized = serde_json::to_string(&original).unwrap();
    let deserialized: Vec<Artifact> = serde_json::from_str(&serialized).unwrap();

    assert_eq!(original, deserialized);
}

#[test]
fn build_list_agent_runs_url_empty_filter() {
    let url = build_list_agent_runs_url(10, &TaskListFilter::default());
    assert_eq!(url, "agent/runs?limit=10");
}

#[test]
fn build_list_agent_runs_url_all_fields() {
    let filter = TaskListFilter {
        creator_uid: Some("user-uid".to_string()),
        updated_after: Some(Utc.with_ymd_and_hms(2026, 4, 3, 12, 30, 0).unwrap()),
        created_after: Some(Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap()),
        created_before: Some(Utc.with_ymd_and_hms(2026, 4, 2, 0, 0, 0).unwrap()),
        states: Some(vec![
            AmbientAgentTaskState::Failed,
            AmbientAgentTaskState::Error,
        ]),
        source: Some(AgentSource::AgentWebhook),
        execution_location: Some(ExecutionLocation::Remote),
        skill_spec: Some("owner/repo:SKILL.md".to_string()),
        config_name: Some("nightly".to_string()),
        model_id: Some("claude-4-5".to_string()),
        search_query: Some("oz run".to_string()),
        cursor: Some("abcd==".to_string()),
    };

    let url = build_list_agent_runs_url(42, &filter);
    assert_eq!(
        url,
        "agent/runs?limit=42\
         &creator=user-uid\
         &updated_after=2026-04-03T12%3A30%3A00%2B00%3A00\
         &created_after=2026-04-01T00%3A00%3A00%2B00%3A00\
         &created_before=2026-04-02T00%3A00%3A00%2B00%3A00\
         &state=FAILED\
         &state=ERROR\
         &source=API\
         &execution_location=REMOTE\
         &skill_spec=owner%2Frepo%3ASKILL.md\
         &name=nightly\
         &model_id=claude-4-5\
         &q=oz%20run\
         &cursor=abcd%3D%3D"
    );
}

#[test]
fn build_list_agent_runs_url_repeats_state_filter() {
    let filter = TaskListFilter {
        states: Some(vec![
            AmbientAgentTaskState::Queued,
            AmbientAgentTaskState::InProgress,
            AmbientAgentTaskState::Succeeded,
        ]),
        ..TaskListFilter::default()
    };
    let url = build_list_agent_runs_url(5, &filter);
    assert_eq!(
        url,
        "agent/runs?limit=5&state=QUEUED&state=INPROGRESS&state=SUCCEEDED"
    );
}

#[test]
fn build_list_agent_runs_url_skips_unknown_state() {
    // The deserializer keeps `Unknown` for forward compatibility, but we shouldn't send it to
    // the server as a filter value.
    let filter = TaskListFilter {
        states: Some(vec![
            AmbientAgentTaskState::Unknown,
            AmbientAgentTaskState::Succeeded,
        ]),
        ..TaskListFilter::default()
    };
    let url = build_list_agent_runs_url(1, &filter);
    assert_eq!(url, "agent/runs?limit=1&state=SUCCEEDED");
}

#[test]
fn build_list_agent_runs_url_routes_to_runs_not_tasks() {
    let url = build_list_agent_runs_url(10, &TaskListFilter::default());
    assert!(url.starts_with("agent/runs?"));
    assert!(!url.starts_with("agent/tasks"));
}

/// Assert that shared `Artifact`s serialize to the expected server wire format.
/// If `Artifact` serialization changes, this test will catch it.
#[test]
fn pull_request_artifact_serializes_to_expected_wire_format() {
    let artifact = Artifact::PullRequest {
        url: "https://github.com/org/repo/pull/42".to_string(),
        branch: "feature-branch".to_string(),
        repo: Some("repo".to_string()),
        number: Some(42),
    };
    let json = serde_json::to_value(&artifact).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "artifact_type": "PULL_REQUEST",
            "data": {
                "url": "https://github.com/org/repo/pull/42",
                "branch": "feature-branch"
            }
        })
    );
}

/// An `EXTERNAL_REFERENCE` artifact with only the required fields omits `title`
/// and `metadata` from the wire format, matching the server's
/// `ExternalReferenceArtifactData` schema.
#[test]
fn external_reference_artifact_serializes_to_expected_wire_format() {
    let artifact = Artifact::ExternalReference {
        reference_type: "LINEAR_ISSUE".to_string(),
        url: "https://linear.app/warpdotdev/issue/REMOTE-2253".to_string(),
        title: None,
        metadata: None,
    };
    let json = serde_json::to_value(&artifact).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "artifact_type": "EXTERNAL_REFERENCE",
            "data": {
                "reference_type": "LINEAR_ISSUE",
                "url": "https://linear.app/warpdotdev/issue/REMOTE-2253"
            }
        })
    );
}

/// An `EXTERNAL_REFERENCE` artifact includes `title` and `metadata` when present.
#[test]
fn external_reference_artifact_includes_optional_title_and_metadata() {
    let artifact = Artifact::ExternalReference {
        reference_type: "GITHUB_PR".to_string(),
        url: "https://github.com/warpdotdev/warp/pull/1".to_string(),
        title: Some("My pull request".to_string()),
        metadata: Some(serde_json::json!({"key": "val"})),
    };
    let json = serde_json::to_value(&artifact).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "artifact_type": "EXTERNAL_REFERENCE",
            "data": {
                "reference_type": "GITHUB_PR",
                "url": "https://github.com/warpdotdev/warp/pull/1",
                "title": "My pull request",
                "metadata": {"key": "val"}
            }
        })
    );
}

/// An `EXTERNAL_REFERENCE` artifact round-trips through deserialize, so run
/// responses carrying it no longer fall through to the unknown/skip path.
#[test]
fn external_reference_artifact_round_trips() {
    let artifact = Artifact::ExternalReference {
        reference_type: "LINEAR_ISSUE".to_string(),
        url: "https://linear.app/warpdotdev/issue/REMOTE-2253".to_string(),
        title: Some("Add report-external-reference CLI subcommand".to_string()),
        metadata: Some(serde_json::json!({"estimate": "S"})),
    };
    let json = serde_json::to_value(&artifact).unwrap();
    let deserialized: Artifact = serde_json::from_value(json).unwrap();
    assert_eq!(artifact, deserialized);
}
