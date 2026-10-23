use std::collections::HashMap;

use anyhow::{Context, Result, anyhow};
#[cfg(not(target_family = "wasm"))]
use base64::{Engine as _, engine::general_purpose::STANDARD};
#[cfg(not(target_family = "wasm"))]
use crc::{CRC_32_ISCSI, Crc};
pub use warp_server_client::HttpStatusError;

/// A presigned upload target returned by the server.
#[serde_with::serde_as]
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UploadTarget {
    pub url: String,
    pub method: String,
    #[serde(default)]
    #[serde_as(deserialize_as = "serde_with::DefaultOnNull")]
    pub headers: HashMap<String, String>,
    /// Ordered multipart form fields for POST uploads.
    #[serde(default)]
    #[serde_as(deserialize_as = "serde_with::DefaultOnNull")]
    pub fields: Vec<UploadField>,
}

/// A single multipart form field on a POST upload target.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct UploadField {
    pub name: String,
    pub value: UploadFieldValue,
}

/// Descriptor for a field value when uploading to an [`UploadTarget`].
/// This is currently only used for `POST` requests, but may be supported
/// for HTTP headers in the future.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UploadFieldValue {
    /// Literal string value known at URL-generation time.
    Static { value: String },
    /// Client should compute CRC32C of the upload, base64-encode the 4-byte
    /// big-endian result, and send it as this field's value.
    // `snake_case` would derive `content_crc32_c`, which does not match the
    // `ContentCRC32CFieldValue` discriminator in warp-server's OpenAPI schema.
    #[serde(rename = "content_crc32c")]
    ContentCrc32C,
    /// Client should use the raw upload bytes as this field's value.
    ContentData,
}

#[cfg(not(target_family = "wasm"))]
pub(crate) static CRC32C: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);
const CONTENT_LENGTH_HEADER_NAME: &str = "content-length";

struct NormalizedUploadTarget<'a> {
    url: &'a str,
    method: &'a str,
    headers: Vec<(&'a str, &'a str)>,
    fields: Vec<NormalizedField<'a>>,
}

/// Borrowed view of a single multipart form field.
#[derive(Debug, PartialEq, Eq)]
struct NormalizedField<'a> {
    name: &'a str,
    value: NormalizedFieldValue<'a>,
}

/// Borrowed view of a single multipart form field value.
#[derive(Debug, PartialEq, Eq)]
enum NormalizedFieldValue<'a> {
    Static(&'a str),
    ContentCrc32C,
    ContentData,
}

impl<'a> From<&'a UploadTarget> for NormalizedUploadTarget<'a> {
    fn from(target: &'a UploadTarget) -> Self {
        Self {
            url: &target.url,
            method: &target.method,
            headers: target
                .headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect(),
            fields: target
                .fields
                .iter()
                .map(|field| NormalizedField {
                    name: field.name.as_str(),
                    value: match &field.value {
                        UploadFieldValue::Static { value } => {
                            NormalizedFieldValue::Static(value.as_str())
                        }
                        UploadFieldValue::ContentCrc32C => NormalizedFieldValue::ContentCrc32C,
                        UploadFieldValue::ContentData => NormalizedFieldValue::ContentData,
                    },
                })
                .collect(),
        }
    }
}

fn build_upload_request<'a>(
    http_client: &'a http_client::Client,
    target: &NormalizedUploadTarget<'_>,
    content_length: Option<u64>,
) -> Result<http_client::RequestBuilder<'a>> {
    let method = target.method.to_ascii_uppercase();
    let has_content_length = target
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case(CONTENT_LENGTH_HEADER_NAME));

    let mut request = match method.as_str() {
        "GET" => http_client.get(target.url),
        "POST" => http_client.post(target.url),
        "PUT" => http_client.put(target.url),
        "DELETE" => http_client.delete(target.url),
        other => return Err(anyhow!("Unsupported HTTP method: {other}")),
    };

    for (name, value) in &target.headers {
        request = request.header(*name, *value);
    }

    if let Some(content_length) = content_length.filter(|_| !has_content_length) {
        request = request.header(CONTENT_LENGTH_HEADER_NAME, content_length.to_string());
    }

    Ok(request)
}

async fn ensure_upload_succeeded(response: http_client::Response) -> Result<()> {
    if response.status().is_success() {
        return Ok(());
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let status_err = HttpStatusError::new(status.as_u16(), body.clone());
    Err(anyhow::Error::new(status_err)
        .context(format!("Upload failed with status {status}: {body}")))
}

async fn send_upload_request(
    http_client: &http_client::Client,
    target: &NormalizedUploadTarget<'_>,
    body: impl Into<reqwest::Body>,
    content_length: u64,
) -> Result<()> {
    let response = build_upload_request(http_client, target, Some(content_length))?
        .body(body)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("Failed to upload to presigned URL")?;

    ensure_upload_succeeded(response).await
}

pub(crate) async fn upload_to_target(
    http_client: &http_client::Client,
    target: &UploadTarget,
    body: Vec<u8>,
) -> Result<()> {
    let normalized = NormalizedUploadTarget::from(target);
    send_upload(http_client, &normalized, body).await
}

/// Routes the request to either a plain body upload or a multipart form upload
/// based on whether the target has form fields.
async fn send_upload(
    http_client: &http_client::Client,
    target: &NormalizedUploadTarget<'_>,
    body: Vec<u8>,
) -> Result<()> {
    if target.fields.is_empty() {
        let length = body.len() as u64;
        let reqwest_body = reqwest::Body::from(body);
        send_upload_request(http_client, target, reqwest_body, length).await
    } else {
        #[cfg(not(target_family = "wasm"))]
        {
            send_multipart_upload(http_client, target, body).await
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = (http_client, body);
            Err(anyhow!(
                "Multipart upload targets are not supported on wasm"
            ))
        }
    }
}

/// Send a `multipart/form-data` request.
///
/// Computes a checksum only when requested by the upload target.
#[cfg(not(target_family = "wasm"))]
async fn send_multipart_upload(
    http_client: &http_client::Client,
    target: &NormalizedUploadTarget<'_>,
    body: Vec<u8>,
) -> Result<()> {
    let form = build_multipart_form(target, body).await?;
    let request = build_upload_request(http_client, target, None)?;
    let response = request
        .multipart(form)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .context("Failed to upload to presigned URL")?;
    ensure_upload_succeeded(response).await
}

#[cfg(not(target_family = "wasm"))]
async fn build_multipart_form(
    target: &NormalizedUploadTarget<'_>,
    body: Vec<u8>,
) -> Result<reqwest::multipart::Form> {
    use reqwest::multipart::{Form, Part};

    let needs_crc32c = target
        .fields
        .iter()
        .any(|field| matches!(field.value, NormalizedFieldValue::ContentCrc32C));
    let crc32c_base64 = if needs_crc32c {
        Some(encode_crc32c_base64(CRC32C.checksum(&body)))
    } else {
        None
    };
    let content_length = body.len() as u64;
    let content_body = reqwest::Body::from(body);

    let mut form = Form::new();
    let mut content_body_cell = Some((content_body, content_length));
    for field in &target.fields {
        let name = field.name.to_string();
        match &field.value {
            NormalizedFieldValue::Static(value) => {
                form = form.text(name, value.to_string());
            }
            NormalizedFieldValue::ContentCrc32C => {
                let crc = crc32c_base64.as_deref().ok_or_else(|| {
                    anyhow!(
                        "Internal error: ContentCrc32C field '{}' requires a computed checksum",
                        field.name
                    )
                })?;
                form = form.text(name, crc.to_string());
            }
            NormalizedFieldValue::ContentData => {
                let (body, length) = content_body_cell.take().ok_or_else(|| {
                    anyhow!("Multipart upload must contain exactly one ContentData field")
                })?;
                form = form.part(name, Part::stream_with_length(body, length));
            }
        }
    }
    if content_body_cell.is_some() {
        return Err(anyhow!(
            "Multipart upload must contain exactly one ContentData field"
        ));
    }
    Ok(form)
}

#[cfg(not(target_family = "wasm"))]
fn encode_crc32c_base64(crc32c: u32) -> String {
    // Storage providers expect the checksum as base64 of the raw big-endian CRC32C bytes,
    // not the more human-readable hex string we typically log.
    STANDARD.encode(crc32c.to_be_bytes())
}

#[cfg(test)]
#[path = "presigned_upload_tests.rs"]
mod tests;
