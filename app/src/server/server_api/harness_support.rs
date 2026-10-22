// Task-scoped harness context is only used by native agent harnesses.
#![cfg_attr(target_family = "wasm", expect(dead_code))]

use std::collections::HashMap;

use anyhow::{Context, Result};
#[cfg(not(target_family = "wasm"))]
use warp_server_client::PresignedDownloadResponse;

use super::ServerApi;
#[cfg(feature = "local_fs")]
pub use super::presigned_upload::FileUploadBody;
pub use super::presigned_upload::UploadBody;
#[cfg(not(target_family = "wasm"))]
use crate::ai::agent_sdk::retry::with_bounded_retry;
use crate::ai::ambient_agents::AmbientAgentTaskId;

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

/// Describes a single file in a snapshot upload request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SnapshotFileInfo {
    pub filename: String,
    pub mime_type: String,
}

/// Task context required when waking a dormant third-party harness session.
#[derive(serde::Deserialize)]
pub struct ResolvedHarnessPrompt {
    #[serde(default)]
    pub resumption_prompt: Option<String>,
}

impl ServerApi {
    pub(crate) async fn get_public_api_response_for_task(
        &self,
        task_id: &AmbientAgentTaskId,
        path: &str,
    ) -> Result<http_client::Response> {
        let auth_token = self
            .get_or_refresh_access_token()
            .await
            .context("Failed to get access token for API request")?;

        let url = format!("{}/api/v1/{}", crate::ChannelState::server_root_url(), path);

        let mut request = self.base_client.http_client().get(&url);
        if let Some(token) = auth_token.as_bearer_token() {
            request = request.bearer_auth(token);
        }

        for (name, value) in self.ambient_agent_headers_for_task(task_id).await? {
            request = request.header(name, value);
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("Failed to send API request to {url}"))?;

        if response.status().is_success() {
            Ok(response)
        } else {
            Err(Self::error_from_response(response).await)
        }
    }

    pub(crate) async fn post_public_api_response_for_task<B>(
        &self,
        task_id: &AmbientAgentTaskId,
        path: &str,
        body: &B,
    ) -> Result<http_client::Response>
    where
        B: serde::Serialize,
    {
        let auth_token = self
            .get_or_refresh_access_token()
            .await
            .context("Failed to get access token for API request")?;

        let url = format!("{}/api/v1/{}", crate::ChannelState::server_root_url(), path);

        let mut request = self.base_client.http_client().post(&url).json(body);
        if let Some(token) = auth_token.as_bearer_token() {
            request = request.bearer_auth(token);
        }

        for (name, value) in self.ambient_agent_headers_for_task(task_id).await? {
            request = request.header(name, value);
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("Failed to send API request to {url}"))?;

        if response.status().is_success() {
            Ok(response)
        } else {
            Err(Self::error_from_response(response).await)
        }
    }

    pub(crate) async fn resolve_prompt_for_task(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> Result<ResolvedHarnessPrompt> {
        let response = self
            .post_public_api_response_for_task(
                task_id,
                "harness-support/resolve-prompt",
                &serde_json::json!({}),
            )
            .await?;
        let url = response.url().clone();
        response
            .json::<ResolvedHarnessPrompt>()
            .await
            .with_context(|| format!("Failed to deserialize response from {url}"))
    }

    pub(crate) async fn fetch_transcript_for_task(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> Result<bytes::Bytes> {
        #[cfg(not(target_family = "wasm"))]
        {
            with_bounded_retry("fetch task-scoped harness-support transcript", || async {
                let response = self
                    .get_public_api_response_for_task(
                        task_id,
                        "harness-support/transcript?redirect=false",
                    )
                    .await?;
                let download = response
                    .json::<PresignedDownloadResponse>()
                    .await
                    .map_err(reqwest::Error::without_url)
                    .context("Failed to decode task-scoped transcript download URL")?;
                self.base_client.download_presigned(&download.url).await
            })
            .await
        }
        #[cfg(target_family = "wasm")]
        {
            let _ = task_id;
            unreachable!(
                "fetch_transcript_for_task is not supported on wasm; agent_sdk is not built on this target"
            );
        }
    }
}

/// Upload a blob to a presigned upload target.
pub async fn upload_to_target(
    http_client: &http_client::Client,
    target: &UploadTarget,
    body: impl UploadBody,
) -> Result<()> {
    super::presigned_upload::upload_to_target(http_client, target, body).await
}

#[cfg(test)]
#[path = "harness_support_tests.rs"]
mod tests;
