use std::collections::HashMap;

use futures::executor::block_on;
use mockito::{Matcher, Server};

use super::*;

#[test]
fn encode_crc32c_base64_matches_spec_example() {
    assert_eq!(encode_crc32c_base64(0x1234_5678), "EjRWeA==");
}

#[test]
fn upload_to_target_replays_headers_for_byte_uploads() {
    block_on(async {
        let mut server = Server::new();
        let mock = server
            .mock("POST", "/upload")
            .match_header("x-test-header", "expected-header")
            .match_header("x-warp-os-category", Matcher::Missing)
            .match_header("x-warp-os-name", Matcher::Missing)
            .match_header("x-warp-client-id", Matcher::Missing)
            .match_header("x-warp-client-version", Matcher::Missing)
            .match_header("x-oz-api-source", Matcher::Missing)
            .match_header("authorization", "AWS4-HMAC-SHA256 test-signature")
            .match_header("proxy-authorization", Matcher::Missing)
            .match_query(Matcher::Exact(
                "X-Amz-Signature=test%2Bsignature&x-id=PutObject".into(),
            ))
            .match_body("serialized body")
            .with_status(200)
            .create();

        let client = http_client::Client::new_external();
        let target = UploadTarget {
            url: format!(
                "{}/upload?X-Amz-Signature=test%2Bsignature&x-id=PutObject",
                server.url()
            ),
            method: "POST".to_string(),
            headers: HashMap::from([
                ("x-test-header".to_string(), "expected-header".to_string()),
                (
                    "authorization".to_string(),
                    "AWS4-HMAC-SHA256 test-signature".to_string(),
                ),
            ]),
            fields: Vec::new(),
        };

        upload_to_target(&client, &target, b"serialized body".to_vec())
            .await
            .unwrap();

        mock.assert();
    });
}

/// Mockito matcher that captures the raw request body so individual fields of
/// a multipart payload can be asserted without depending on reqwest's boundary.
fn matches_multipart_field(name: &'static str, expected: &'static str) -> Matcher {
    // mockito matches on the body as a raw byte slice; we look for the form-data
    // disposition header and the expected value within the same part.
    Matcher::Regex(format!(
        "name=\"{name}\"\r\n\r\n{expected}\r\n",
        name = regex::escape(name),
        expected = regex::escape(expected)
    ))
}

#[test]
fn upload_to_target_builds_multipart_post_with_static_crc_and_data_fields() {
    block_on(async {
        let mut server = Server::new();
        let body = b"multipart artifact payload";
        let expected_crc32c = encode_crc32c_base64(CRC32C.checksum(body));
        let expected_crc32c_escaped = expected_crc32c.clone();

        let mock = server
            .mock("POST", "/upload")
            .match_header(
                "content-type",
                Matcher::Regex("^multipart/form-data; boundary=.+".to_string()),
            )
            .match_body(Matcher::AllOf(vec![
                matches_multipart_field("key", "presigned/object/key"),
                matches_multipart_field("Content-Type", "application/octet-stream"),
                Matcher::Regex(format!(
                    r#"name="x-amz-checksum-crc32c"\r\n\r\n{crc}\r\n"#,
                    crc = regex::escape(&expected_crc32c_escaped)
                )),
                Matcher::Regex(r#"name="file"[\s\S]*multipart artifact payload"#.to_string()),
            ]))
            .with_status(204)
            .create();

        let client = http_client::Client::new_for_test();
        let target = UploadTarget {
            url: format!("{}/upload", server.url()),
            method: "POST".to_string(),
            headers: HashMap::new(),
            fields: vec![
                UploadField {
                    name: "key".to_string(),
                    value: UploadFieldValue::Static {
                        value: "presigned/object/key".to_string(),
                    },
                },
                UploadField {
                    name: "Content-Type".to_string(),
                    value: UploadFieldValue::Static {
                        value: "application/octet-stream".to_string(),
                    },
                },
                UploadField {
                    name: "x-amz-checksum-crc32c".to_string(),
                    value: UploadFieldValue::ContentCrc32C,
                },
                UploadField {
                    name: "file".to_string(),
                    value: UploadFieldValue::ContentData,
                },
            ],
        };

        upload_to_target(&client, &target, body.to_vec())
            .await
            .unwrap();

        mock.assert();
    });
}

#[test]
fn multipart_post_requires_content_data_field() {
    block_on(async {
        let target = UploadTarget {
            url: "https://example.com/upload".to_string(),
            method: "POST".to_string(),
            headers: HashMap::new(),
            fields: vec![UploadField {
                name: "key".to_string(),
                value: UploadFieldValue::Static {
                    value: "presigned/object/key".to_string(),
                },
            }],
        };
        let normalized = NormalizedUploadTarget::from(&target);

        let err = build_multipart_form(&normalized, b"missing content data".to_vec())
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("Multipart upload must contain exactly one ContentData field")
        );
    });
}

/// The server marshals its unset Go maps and slices as `null`, which a POST
/// target routinely carries for `headers`.
#[test]
fn upload_target_deserializes_null_headers_and_fields_as_empty() {
    use super::UploadTarget;

    let target: UploadTarget = serde_json::from_value(serde_json::json!({
        "url": "https://example.com/upload",
        "method": "PUT",
        "headers": null,
        "fields": null
    }))
    .unwrap();

    assert!(target.headers.is_empty());
    assert!(target.fields.is_empty());
}

/// Each `kind` here is a discriminator from the `UploadFieldValue` schema in
/// warp-server's `public_api/openapi.yaml`. A mismatch fails the whole target,
/// so every presigned POST upload would break.
#[test]
fn upload_field_value_deserializes_every_server_kind() {
    use super::{UploadFieldValue, UploadTarget};

    let target: UploadTarget = serde_json::from_value(serde_json::json!({
        "url": "https://example.com/upload",
        "method": "POST",
        "headers": {},
        "fields": [
            {"name": "key", "value": {"kind": "static", "value": "object/key"}},
            {"name": "x-amz-checksum-crc32c", "value": {"kind": "content_crc32c"}},
            {"name": "file", "value": {"kind": "content_data"}}
        ]
    }))
    .unwrap();

    assert!(matches!(
        &target.fields[0].value,
        UploadFieldValue::Static { value } if value == "object/key"
    ));
    assert!(matches!(
        target.fields[1].value,
        UploadFieldValue::ContentCrc32C
    ));
    assert!(matches!(
        target.fields[2].value,
        UploadFieldValue::ContentData
    ));
}
