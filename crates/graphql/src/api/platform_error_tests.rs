use super::*;

fn response_with_metadata(
    metadata: Vec<(&str, &str)>,
    debug: Option<&str>,
) -> PlatformErrorInfoResponse {
    PlatformErrorInfoResponse {
        error_message: "GitHub is temporarily unavailable.".to_string(),
        code: PlatformErrorCode::ResourceUnavailable,
        http_status: 503,
        user_facing_messages: vec![
            PlatformErrorMessage {
                format: PlatformErrorMessageFormat::PlainText,
                message: "GitHub is temporarily unavailable.".to_string(),
            },
            PlatformErrorMessage {
                format: PlatformErrorMessageFormat::SlackMrkdwn,
                message: "*GitHub* is temporarily unavailable.".to_string(),
            },
            PlatformErrorMessage {
                format: PlatformErrorMessageFormat::Markdown,
                message: "**GitHub** is temporarily unavailable.".to_string(),
            },
        ],
        detail: Some("Repository access could not be resolved.".to_string()),
        retryable: true,
        is_user_error: false,
        metadata: metadata
            .into_iter()
            .map(|(key, value)| PlatformErrorMetadataResponse {
                key: key.to_string(),
                value: value.to_string(),
            })
            .collect(),
        debug: debug.map(str::to_string),
        metrics_category: "dependency_unavailable".to_string(),
        trace_id: Some("0123456789abcdef".to_string()),
    }
}

#[test]
fn response_decodes_platform_error_info() {
    let response = response_with_metadata(
        vec![("resource", "installation"), ("provider", "github")],
        Some("request-id=example"),
    );

    let info = PlatformErrorInfo::from(response);
    assert_eq!(
        info.error_message.as_deref(),
        Some("GitHub is temporarily unavailable.")
    );

    assert_eq!(info.code, PlatformErrorCode::ResourceUnavailable);
    assert_eq!(info.http_status, Some(503));
    assert_eq!(
        info.user_facing_messages[&PlatformErrorMessageFormat::PlainText],
        "GitHub is temporarily unavailable."
    );
    assert_eq!(
        info.user_facing_messages[&PlatformErrorMessageFormat::SlackMrkdwn],
        "*GitHub* is temporarily unavailable."
    );
    assert_eq!(
        info.user_facing_messages[&PlatformErrorMessageFormat::Markdown],
        "**GitHub** is temporarily unavailable."
    );
    assert_eq!(
        info.detail.as_deref(),
        Some("Repository access could not be resolved.")
    );
    assert!(info.retryable);
    assert_eq!(info.is_user_error, Some(false));
    assert_eq!(info.metadata["provider"], "github");
    assert_eq!(info.metadata["resource"], "installation");
    assert_eq!(info.debug.as_deref(), Some("request-id=example"));
    assert_eq!(
        info.metrics_category.as_deref(),
        Some("dependency_unavailable")
    );
    assert_eq!(info.trace_id.as_deref(), Some("0123456789abcdef"));
}

#[test]
fn duplicate_metadata_keys_decode_with_the_last_value() {
    let response =
        response_with_metadata(vec![("provider", "github"), ("provider", "gitlab")], None);

    let info = PlatformErrorInfo::from(response);

    assert_eq!(
        info.metadata,
        BTreeMap::from([("provider".to_string(), "gitlab".to_string())])
    );
}

#[test]
fn optional_debug_is_preserved_when_absent() {
    let info = PlatformErrorInfo::from(response_with_metadata(Vec::new(), None));

    assert_eq!(info.debug, None);
}

#[test]
fn duplicate_message_formats_decode_with_the_last_value() {
    let mut response = response_with_metadata(Vec::new(), None);
    response.user_facing_messages = vec![
        PlatformErrorMessage {
            format: PlatformErrorMessageFormat::PlainText,
            message: "first".to_string(),
        },
        PlatformErrorMessage {
            format: PlatformErrorMessageFormat::PlainText,
            message: "last".to_string(),
        },
    ];

    let info = PlatformErrorInfo::from(response);

    assert_eq!(
        info.user_facing_messages,
        BTreeMap::from([(PlatformErrorMessageFormat::PlainText, "last".to_string())])
    );
}
