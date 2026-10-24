use anyhow::anyhow;
use async_trait::async_trait;
use base64::Engine;
use chrono::{DateTime, Utc};
use cynic::{MutationBuilder, QueryBuilder};
#[cfg(test)]
use mockall::automock;
use prost::Message;
use warp_core::features::FeatureFlag;
use warp_errors::report_error;
use warp_graphql::ai::{AgentTaskState, PlatformErrorCode};
use warp_graphql::client::Operation;
use warp_graphql::mutations::delete_ai_conversation::{
    DeleteAIConversation, DeleteAIConversationVariables, DeleteConversationInput,
    DeleteConversationResult,
};
use warp_graphql::mutations::request_bonus::{
    ProvideNegativeFeedbackResponseForAiConversation,
    ProvideNegativeFeedbackResponseForAiConversationInput,
    ProvideNegativeFeedbackResponseForAiConversationVariables, RequestsRefundedResult,
};
use warp_graphql::mutations::update_agent_task::{
    AgentTaskStatusMessageInput, UpdateAgentTask, UpdateAgentTaskInput, UpdateAgentTaskResult,
    UpdateAgentTaskVariables,
};
use warp_graphql::platform_error::PlatformErrorInfo;
use warp_graphql::queries::free_available_models::{
    FreeAvailableModels, FreeAvailableModelsInput, FreeAvailableModelsResult,
    FreeAvailableModelsVariables,
};
#[cfg(not(feature = "agent_mode_evals"))]
use warp_graphql::queries::get_ai_credit_availability::{
    GetAICreditAvailability, GetAICreditAvailabilityVariables,
};
use warp_graphql::queries::get_available_harnesses::{
    GetAvailableHarnesses, GetAvailableHarnessesVariables,
};
use warp_graphql::queries::get_feature_model_choices::{
    GetFeatureModelChoices, GetFeatureModelChoicesVariables,
};
#[cfg(not(feature = "agent_mode_evals"))]
use warp_graphql::queries::get_request_limit_info::{
    GetRequestLimitInfo, GetRequestLimitInfoVariables,
};
use warp_multi_agent_api::ConversationData;

use super::ServerApi;
#[cfg(not(feature = "agent_mode_evals"))]
use crate::ai::BonusGrant;
use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::{AIAgentHarness, ServerAIConversationMetadata};
use crate::ai::ambient_agents::AmbientAgentTaskId;
// Re-export ambient agent types for backwards compatibility
pub use crate::ai::ambient_agents::{
    AgentSource, AmbientAgentTask, AmbientAgentTaskState, ExecutionLocation,
};
use crate::ai::artifacts::Artifact;
use crate::ai::harness_availability::HarnessAvailability;
use crate::ai::llms::{
    AvailableLLMs, DisableReason, LLMContextWindow, LLMInfo, LLMModelHost, LLMSpec,
    LLMUsageMetadata, ModelsByFeature, RoutingHostConfig,
};
#[cfg(feature = "agent_mode_evals")]
use crate::ai::request_usage_model::RequestLimitInfo;
use crate::ai::{AICreditAvailability, RequestUsageInfo};
use crate::persistence::model::ConversationUsageMetadata;
use crate::server::graphql::{get_request_context, get_user_facing_error_message};
use crate::server::team_scope::RequestTeamScope;
#[cfg(not(feature = "agent_mode_evals"))]
use crate::{
    server::ids::ServerId,
    workspaces::{gql_convert::PLACEHOLDER_WORKSPACE_UID, workspace::WorkspaceUid},
};

/// A status update for a task, optionally including a platform error code.
pub struct TaskStatusUpdate {
    pub message: String,
    pub error_code: Option<PlatformErrorCode>,
    pub platform_error: Option<Box<PlatformErrorInfo>>,
}

fn agent_task_status_message_input(update: TaskStatusUpdate) -> AgentTaskStatusMessageInput {
    AgentTaskStatusMessageInput {
        message: update.message,
        error_code: update.error_code,
        error: update.platform_error.map(|info| (*info).into()),
    }
}

impl TaskStatusUpdate {
    /// Create a status update with just a message (no error code).
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            error_code: None,
            platform_error: None,
        }
    }

    /// Create a status update with a message and error code.
    pub fn with_error_code(message: impl Into<String>, error_code: PlatformErrorCode) -> Self {
        Self {
            message: message.into(),
            error_code: Some(error_code),
            platform_error: Some(Box::new(PlatformErrorInfo::new(error_code, false))),
        }
    }
}

/// Request body for `POST /agent/conversations/{conversation_id}/rename`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenameConversationRequest {
    pub title: String,
}

/// Response body for `POST /agent/conversations/{conversation_id}/rename`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RenameConversationResponse {
    pub title: String,
}

/// Response from the artifact endpoint.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(tag = "artifact_type")]
pub enum ArtifactDownloadResponse {
    #[serde(rename = "FILE")]
    File {
        #[serde(flatten)]
        common: ArtifactDownloadCommonFields,
        data: FileArtifactResponseData,
    },
}

impl ArtifactDownloadResponse {
    fn common(&self) -> &ArtifactDownloadCommonFields {
        match self {
            ArtifactDownloadResponse::File { common, .. } => common,
        }
    }

    pub fn artifact_uid(&self) -> &str {
        &self.common().artifact_uid
    }

    pub fn download_url(&self) -> &str {
        match self {
            ArtifactDownloadResponse::File { data, .. } => &data.download_url,
        }
    }

    pub fn content_type(&self) -> &str {
        match self {
            ArtifactDownloadResponse::File { data, .. } => &data.content_type,
        }
    }

    pub fn filename(&self) -> Option<&str> {
        match self {
            ArtifactDownloadResponse::File { data, .. } => Some(&data.filename),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ArtifactDownloadCommonFields {
    pub artifact_uid: String,
    pub created_at: DateTime<Utc>,
}

/// File-specific data from the artifact endpoint.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct FileArtifactResponseData {
    pub download_url: String,
    pub expires_at: DateTime<Utc>,
    pub content_type: String,
    pub filepath: String,
    pub filename: String,
    pub description: Option<String>,
    pub size_bytes: Option<i64>,
}

/// Filter parameters for listing ambient agent tasks.
#[derive(Clone, Debug, Default)]
pub struct TaskListFilter {
    pub creator_uid: Option<String>,
    pub updated_after: Option<DateTime<Utc>>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub states: Option<Vec<AmbientAgentTaskState>>,
    pub source: Option<AgentSource>,
    pub execution_location: Option<ExecutionLocation>,
    pub skill_spec: Option<String>,
    pub config_name: Option<String>,
    pub model_id: Option<String>,
    pub search_query: Option<String>,
    pub cursor: Option<String>,
}

/// Build the path + query string for `GET /api/v1/agent/runs` from a filter.
pub(crate) fn build_list_agent_runs_url(limit: i32, filter: &TaskListFilter) -> String {
    let mut url = format!("agent/runs?limit={limit}");

    let mut push = |key: &str, value: &str| {
        url.push('&');
        url.push_str(key);
        url.push('=');
        url.push_str(urlencoding::encode(value).as_ref());
    };

    if let Some(creator_uid) = filter.creator_uid.as_deref() {
        push("creator", creator_uid);
    }
    if let Some(updated_after) = filter.updated_after {
        push("updated_after", &updated_after.to_rfc3339());
    }
    if let Some(created_after) = filter.created_after {
        push("created_after", &created_after.to_rfc3339());
    }
    if let Some(created_before) = filter.created_before {
        push("created_before", &created_before.to_rfc3339());
    }
    if let Some(states) = filter.states.as_ref() {
        for state in states {
            if let Some(value) = state.as_query_param() {
                push("state", value);
            }
        }
    }
    if let Some(source) = filter.source.as_ref() {
        push("source", source.as_str());
    }
    if let Some(execution_location) = filter.execution_location {
        push("execution_location", execution_location.as_query_param());
    }

    if let Some(skill_spec) = filter.skill_spec.as_deref() {
        push("skill_spec", skill_spec);
    }

    if let Some(config_name) = filter.config_name.as_deref() {
        push("name", config_name);
    }
    if let Some(model_id) = filter.model_id.as_deref() {
        push("model_id", model_id);
    }
    if let Some(search_query) = filter.search_query.as_deref() {
        push("q", search_query);
    }
    if let Some(cursor) = filter.cursor.as_deref() {
        push("cursor", cursor);
    }

    url
}

pub(crate) fn build_rename_conversation_url(conversation_id: &str) -> String {
    format!(
        "agent/conversations/{}/rename",
        urlencoding::encode(conversation_id)
    )
}

struct ListRunsResponse {
    runs: Vec<AmbientAgentTask>,
}

impl<'de> serde::Deserialize<'de> for ListRunsResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[serde_with::serde_as]
        #[derive(serde::Deserialize)]
        struct RawResponse {
            #[serde_as(deserialize_as = "serde_with::DefaultOnNull")]
            runs: Vec<serde_json::Value>,
        }

        let raw = RawResponse::deserialize(deserializer)?;
        let mut runs = Vec::with_capacity(raw.runs.len());

        for task_value in raw.runs.into_iter() {
            match serde_json::from_value::<AmbientAgentTask>(task_value) {
                Ok(task) => runs.push(task),
                Err(e) => {
                    // Log the error and skip this task instead of failing the entire request
                    report_error!(anyhow!("Failed to deserialize ambient agent task: {}", e));
                }
            }
        }

        Ok(ListRunsResponse { runs })
    }
}

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AIClient: 'static + Send + Sync {
    async fn get_request_limit_info(&self) -> Result<RequestUsageInfo, anyhow::Error>;

    /// Fetches the server-authoritative decision on whether the authenticated
    /// user can start an interactive AI request.
    async fn get_ai_credit_availability(&self) -> Result<AICreditAvailability, anyhow::Error>;

    async fn get_feature_model_choices(&self) -> Result<ModelsByFeature, anyhow::Error>;

    async fn get_available_harnesses(&self) -> Result<Vec<HarnessAvailability>, anyhow::Error>;
    /// Fetches the free-tier available models without requiring authentication.
    /// Used during pre-login onboarding so logged-out users see an accurate model list
    /// instead of the hard-coded `ModelsByFeature::default()` fallback.
    async fn get_free_available_models(
        &self,
        referrer: Option<String>,
    ) -> Result<ModelsByFeature, anyhow::Error>;

    async fn provide_negative_feedback_response_for_ai_conversation(
        &self,
        conversation_id: String,
        request_ids: Vec<String>,
    ) -> anyhow::Result<i32, anyhow::Error>;

    /// Updates a run's server-side record. Every argument is independently optional; omitted
    /// fields are left untouched rather than cleared.
    ///
    async fn update_agent_task(
        &self,
        task_id: AmbientAgentTaskId,
        task_state: Option<AgentTaskState>,
        session_id: Option<session_sharing_protocol::common::SessionId>,
        conversation_id: Option<String>,
        status_message: Option<TaskStatusUpdate>,
    ) -> anyhow::Result<(), anyhow::Error>;

    /// Rename a server-side conversation and return the normalized title.
    async fn rename_conversation(
        &self,
        conversation_id: String,
        title: String,
    ) -> anyhow::Result<RenameConversationResponse, anyhow::Error>;

    async fn list_ambient_agent_tasks(
        &self,
        limit: i32,
        filter: TaskListFilter,
        request_team_scope: Option<RequestTeamScope>,
    ) -> anyhow::Result<Vec<AmbientAgentTask>, anyhow::Error>;

    async fn get_ambient_agent_task(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> anyhow::Result<AmbientAgentTask, anyhow::Error>;

    async fn get_ai_conversation(
        &self,
        server_conversation_token: ServerConversationToken,
    ) -> anyhow::Result<(ConversationData, ServerAIConversationMetadata), anyhow::Error>;

    async fn list_ai_conversation_metadata(
        &self,
        conversation_ids: Option<Vec<String>>,
    ) -> anyhow::Result<Vec<ServerAIConversationMetadata>>;

    async fn delete_ai_conversation(
        &self,
        server_conversation_token: String,
    ) -> anyhow::Result<(), anyhow::Error>;

    async fn cancel_ambient_agent_task(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> anyhow::Result<(), anyhow::Error>;

    async fn get_artifact_download(
        &self,
        artifact_uid: &str,
    ) -> anyhow::Result<ArtifactDownloadResponse, anyhow::Error>;
}

impl ServerApi {
    async fn get_public_api_with_team_scope<R>(
        &self,
        path: &str,
        request_team_scope: Option<RequestTeamScope>,
    ) -> anyhow::Result<R>
    where
        R: serde::de::DeserializeOwned,
    {
        self.base_client
            .get_public_api_for_team(
                path,
                request_team_scope.and_then(RequestTeamScope::team_uid),
            )
            .await
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl AIClient for ServerApi {
    #[cfg(feature = "agent_mode_evals")]
    async fn get_request_limit_info(&self) -> Result<RequestUsageInfo, anyhow::Error> {
        Ok(RequestUsageInfo {
            request_limit_info: RequestLimitInfo::new_for_evals(),
            bonus_grants: vec![],
        })
    }

    #[cfg(not(feature = "agent_mode_evals"))]
    async fn get_request_limit_info(&self) -> Result<RequestUsageInfo, anyhow::Error> {
        let variables = GetRequestLimitInfoVariables {
            request_context: get_request_context(),
        };
        let operation = GetRequestLimitInfo::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.user {
            warp_graphql::queries::get_request_limit_info::UserResult::UserOutput(user_output) => {
                let request_limit_info = user_output.user.request_limit_info.into();

                let workspace_and_team_bonus_grants = user_output
                    .user
                    .workspaces
                    .into_iter()
                    .filter(|workspace| workspace.uid != PLACEHOLDER_WORKSPACE_UID.into())
                    .flat_map(|workspace| {
                        let workspace_uid =
                            WorkspaceUid::from(ServerId::from_string_lossy(workspace.uid.inner()));
                        workspace
                            .bonus_grants_info
                            .grants
                            .into_iter()
                            .map(move |grant| {
                                BonusGrant::from_gql_workspace_or_team_bonus_grant(
                                    grant,
                                    workspace_uid,
                                )
                            })
                    });

                let bonus_grants: Vec<BonusGrant> = user_output
                    .user
                    .bonus_grants
                    .into_iter()
                    .map(BonusGrant::from_gql_user_bonus_grant)
                    .chain(workspace_and_team_bonus_grants)
                    .collect();

                Ok(RequestUsageInfo {
                    request_limit_info,
                    bonus_grants,
                })
            }
            warp_graphql::queries::get_request_limit_info::UserResult::UserFacingError(e) => {
                Err(anyhow!(get_user_facing_error_message(e)))
            }
            warp_graphql::queries::get_request_limit_info::UserResult::Unknown => {
                Err(anyhow!("failed to get request limit info"))
            }
        }
    }

    #[cfg(feature = "agent_mode_evals")]
    async fn get_ai_credit_availability(&self) -> Result<AICreditAvailability, anyhow::Error> {
        Ok(AICreditAvailability::available_with_source(Some(
            crate::ai::AICreditSource::BaseLimit,
        )))
    }

    #[cfg(not(feature = "agent_mode_evals"))]
    async fn get_ai_credit_availability(&self) -> Result<AICreditAvailability, anyhow::Error> {
        let variables = GetAICreditAvailabilityVariables {
            request_context: get_request_context(),
        };
        let operation = GetAICreditAvailability::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.user {
            warp_graphql::queries::get_ai_credit_availability::UserResult::UserOutput(output) => {
                Ok(output.user.ai_credit_availability.into())
            }
            warp_graphql::queries::get_ai_credit_availability::UserResult::UserFacingError(e) => {
                Err(anyhow!(get_user_facing_error_message(e)))
            }
            warp_graphql::queries::get_ai_credit_availability::UserResult::Unknown => {
                Err(anyhow!("failed to get AI credit availability"))
            }
        }
    }

    async fn get_feature_model_choices(&self) -> Result<ModelsByFeature, anyhow::Error> {
        let variables = GetFeatureModelChoicesVariables {
            request_context: get_request_context(),
        };
        let operation = GetFeatureModelChoices::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.user {
            warp_graphql::queries::get_feature_model_choices::UserResult::UserOutput(
                warp_graphql::queries::get_feature_model_choices::UserOutput {
                    user: warp_graphql::queries::get_feature_model_choices::User { mut workspaces },
                },
            ) if !workspaces.is_empty() => {
                // This is safe (`remove()` can panic) because we ensure workspaces is non-empty
                // above.
                workspaces.remove(0).feature_model_choice.try_into()
            }
            _ => Err(anyhow!("Failed to get available feature model choices")),
        }
    }

    async fn get_available_harnesses(&self) -> Result<Vec<HarnessAvailability>, anyhow::Error> {
        let variables = GetAvailableHarnessesVariables {
            request_context: get_request_context(),
        };
        let operation = GetAvailableHarnesses::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.user {
            warp_graphql::queries::get_available_harnesses::UserResult::UserOutput(output) => {
                Ok(output
                    .user
                    .available_harnesses
                    .harnesses
                    .into_iter()
                    .map(|h| HarnessAvailability {
                        harness: convert_harness(h.harness).into(),
                        display_name: h.display_name,
                        enabled: h.enabled,
                        available_models: h
                            .available_models
                            .into_iter()
                            .map(|m| crate::ai::harness_availability::HarnessModelInfo {
                                id: m.id.into_inner(),
                                display_name: m.display_name,
                                reasoning_level: m.reasoning_level,
                            })
                            .collect(),
                    })
                    .collect())
            }
            warp_graphql::queries::get_available_harnesses::UserResult::Unknown => {
                Err(anyhow!("Failed to get available harnesses"))
            }
        }
    }

    async fn get_free_available_models(
        &self,
        referrer: Option<String>,
    ) -> Result<ModelsByFeature, anyhow::Error> {
        // This resolver is public; it does not require an auth token. We must NOT go through
        // `send_graphql_request`, which awaits `get_or_refresh_access_token()`
        let variables = FreeAvailableModelsVariables {
            input: FreeAvailableModelsInput { referrer },
            request_context: get_request_context(),
        };
        let operation = FreeAvailableModels::build(variables);

        // Best-effort: if the user has a valid token (e.g. anonymous Firebase), include it;
        // otherwise send unauthenticated. Either is acceptable for this resolver.
        let auth_token = self
            .get_or_refresh_access_token()
            .await
            .ok()
            .and_then(|token| token.bearer_token());

        let response = operation
            .send_request(
                self.base_client.owned_http_client(),
                self.base_client
                    .graphql_request_options_with_token(auth_token),
            )
            .await?
            .data
            .ok_or_else(|| anyhow!("Missing data in freeAvailableModels response"))?;

        match response.free_available_models {
            FreeAvailableModelsResult::FreeAvailableModelsOutput(output) => {
                output.feature_model_choice.try_into()
            }
            FreeAvailableModelsResult::Unknown => {
                Err(anyhow!("Unexpected freeAvailableModels response variant"))
            }
        }
    }

    async fn provide_negative_feedback_response_for_ai_conversation(
        &self,
        conversation_id: String,
        request_ids: Vec<String>,
    ) -> anyhow::Result<i32, anyhow::Error> {
        let variables = ProvideNegativeFeedbackResponseForAiConversationVariables {
            input: ProvideNegativeFeedbackResponseForAiConversationInput {
                conversation_id: conversation_id.into(),
                request_ids: request_ids.into_iter().map(Into::into).collect(),
            },
            request_context: get_request_context(),
        };

        let operation = ProvideNegativeFeedbackResponseForAiConversation::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.provide_negative_feedback_response_for_ai_conversation {
            RequestsRefundedResult::RequestsRefundedOutput(output) => Ok(output.requests_refunded),
            RequestsRefundedResult::UserFacingError(e) => {
                Err(anyhow!(get_user_facing_error_message(e)))
            }
            RequestsRefundedResult::Unknown => Err(anyhow!(
                "failed to provide negative feedback response for ai conversation"
            )),
        }
    }

    #[tracing::instrument(skip_all, err, fields(
        ?task_state,
        ?session_id,
        ?conversation_id,
        error_code = ?status_message.as_ref().map(|m| m.error_code)
    ))]
    async fn update_agent_task(
        &self,
        task_id: AmbientAgentTaskId,
        task_state: Option<AgentTaskState>,
        session_id: Option<session_sharing_protocol::common::SessionId>,
        conversation_id: Option<String>,
        status_message: Option<TaskStatusUpdate>,
    ) -> anyhow::Result<(), anyhow::Error> {
        let variables = UpdateAgentTaskVariables {
            input: UpdateAgentTaskInput {
                task_id: task_id.to_string().into(),
                task_state,
                session_id: session_id.map(|id| id.to_string().into()),
                conversation_id: conversation_id.map(|id| id.into()),
                status_message: status_message.map(agent_task_status_message_input),
            },
            request_context: get_request_context(),
        };

        let operation = UpdateAgentTask::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.update_agent_task {
            UpdateAgentTaskResult::UpdateAgentTaskOutput(_) => Ok(()),
            UpdateAgentTaskResult::UserFacingError(e) => {
                Err(anyhow!(get_user_facing_error_message(e)))
            }
            UpdateAgentTaskResult::Unknown => Err(anyhow!("failed to update agent task")),
        }
    }

    async fn rename_conversation(
        &self,
        conversation_id: String,
        title: String,
    ) -> anyhow::Result<RenameConversationResponse, anyhow::Error> {
        let request = RenameConversationRequest { title };
        let response: RenameConversationResponse = self
            .post_public_api(&build_rename_conversation_url(&conversation_id), &request)
            .await?;
        Ok(response)
    }

    async fn list_ambient_agent_tasks(
        &self,
        limit: i32,
        filter: TaskListFilter,
        request_team_scope: Option<RequestTeamScope>,
    ) -> anyhow::Result<Vec<AmbientAgentTask>, anyhow::Error> {
        let url = build_list_agent_runs_url(limit, &filter);
        let response: ListRunsResponse = self
            .get_public_api_with_team_scope(&url, request_team_scope)
            .await?;
        Ok(response.runs)
    }

    #[tracing::instrument(skip_all, err, fields(tags.cloud_agent = true))]
    async fn get_ambient_agent_task(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> anyhow::Result<AmbientAgentTask, anyhow::Error> {
        let response: AmbientAgentTask = self
            .get_public_api(&format!("agent/runs/{task_id}"))
            .await?;
        Ok(response)
    }

    #[tracing::instrument(skip_all, err, fields(tags.cloud_agent = true))]
    async fn get_ai_conversation(
        &self,
        server_conversation_token: ServerConversationToken,
    ) -> anyhow::Result<(ConversationData, ServerAIConversationMetadata), anyhow::Error> {
        use warp_graphql::queries::list_ai_conversations::{
            ListAIConversations, ListAIConversationsInput, ListAIConversationsResult,
            ListAIConversationsVariables,
        };

        let conversation_id = server_conversation_token.as_str().to_string();
        let operation = ListAIConversations::build(ListAIConversationsVariables {
            input: ListAIConversationsInput {
                conversation_ids: Some(vec![cynic::Id::new(conversation_id)]),
            },
            request_context: get_request_context(),
        });
        let response = self.send_graphql_request(operation, None).await?;

        let gql_conversation = match response.list_ai_conversations {
            ListAIConversationsResult::ListAIConversationsOutput(output) => output
                .conversations
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("Conversation not found"))?,
            ListAIConversationsResult::UserFacingError(e) => {
                return Err(anyhow!(get_user_facing_error_message(e)));
            }
            ListAIConversationsResult::Unknown => {
                return Err(anyhow!("Failed to get AI conversation"));
            }
        };

        let conversation_data_bytes = base64::engine::general_purpose::STANDARD
            .decode(&gql_conversation.final_task_list)
            .map_err(|e| anyhow!("Failed to decode base64 conversation data: {e}"))?;

        let conversation_data = ConversationData::decode(conversation_data_bytes.as_slice())
            .map_err(|e| anyhow!("Failed to decode proto ConversationData: {e}"))?;

        // Build AIConversationMetadata from GraphQL response
        let metadata = gql_conversation.try_into()?;

        Ok((conversation_data, metadata))
    }

    async fn list_ai_conversation_metadata(
        &self,
        conversation_ids: Option<Vec<String>>,
    ) -> anyhow::Result<Vec<ServerAIConversationMetadata>> {
        if !FeatureFlag::CloudConversations.is_enabled() {
            return Ok(vec![]);
        }
        use warp_graphql::queries::list_ai_conversations::{
            ListAIConversationMetadata, ListAIConversationMetadataResult,
            ListAIConversationMetadataVariables, ListAIConversationsInput,
        };

        let input = ListAIConversationsInput {
            conversation_ids: conversation_ids
                .map(|ids| ids.into_iter().map(cynic::Id::new).collect()),
        };

        let variables = ListAIConversationMetadataVariables {
            input,
            request_context: get_request_context(),
        };

        let operation = ListAIConversationMetadata::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.list_ai_conversations {
            ListAIConversationMetadataResult::ListAIConversationsOutput(output) => {
                let metadata_vec: Result<Vec<_>, _> = output
                    .conversations
                    .into_iter()
                    .map(|conv| conv.try_into())
                    .collect();
                metadata_vec
            }
            ListAIConversationMetadataResult::UserFacingError(e) => {
                Err(anyhow!(get_user_facing_error_message(e)))
            }
            ListAIConversationMetadataResult::Unknown => {
                Err(anyhow!("Failed to list AI conversations metadata"))
            }
        }
    }

    async fn delete_ai_conversation(
        &self,
        server_conversation_token: String,
    ) -> anyhow::Result<(), anyhow::Error> {
        let variables = DeleteAIConversationVariables {
            input: DeleteConversationInput {
                conversation_id: server_conversation_token.into(),
            },
            request_context: get_request_context(),
        };

        let operation = DeleteAIConversation::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.delete_conversation {
            DeleteConversationResult::DeleteConversationOutput(_) => Ok(()),
            DeleteConversationResult::UserFacingError(e) => {
                Err(anyhow!(get_user_facing_error_message(e)))
            }
            DeleteConversationResult::Unknown => Err(anyhow!("Failed to delete AI conversation")),
        }
    }

    async fn cancel_ambient_agent_task(
        &self,
        task_id: &AmbientAgentTaskId,
    ) -> anyhow::Result<(), anyhow::Error> {
        let _: String = self
            .post_public_api(&format!("agent/tasks/{task_id}/cancel"), &())
            .await?;
        Ok(())
    }

    async fn get_artifact_download(
        &self,
        artifact_uid: &str,
    ) -> anyhow::Result<ArtifactDownloadResponse, anyhow::Error> {
        let response: ArtifactDownloadResponse = self
            .get_public_api(&format!("agent/artifacts/{artifact_uid}"))
            .await?;
        Ok(response)
    }
}

impl TryFrom<warp_graphql::queries::get_feature_model_choices::FeatureModelChoice>
    for ModelsByFeature
{
    type Error = anyhow::Error;

    fn try_from(
        value: warp_graphql::queries::get_feature_model_choices::FeatureModelChoice,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_mode: value.agent_mode.try_into()?,
            coding: value.coding.try_into()?,
            cli_agent: Some(value.cli_agent.try_into()?),
        })
    }
}

impl TryFrom<warp_graphql::workspace::FeatureModelChoice> for ModelsByFeature {
    type Error = anyhow::Error;

    fn try_from(value: warp_graphql::workspace::FeatureModelChoice) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_mode: value.agent_mode.try_into()?,
            coding: value.coding.try_into()?,
            cli_agent: Some(value.cli_agent.try_into()?),
        })
    }
}

impl TryFrom<warp_graphql::queries::get_feature_model_choices::AvailableLlms> for AvailableLLMs {
    type Error = anyhow::Error;

    fn try_from(
        value: warp_graphql::queries::get_feature_model_choices::AvailableLlms,
    ) -> Result<Self, Self::Error> {
        Self::new(
            value.default_id.into(),
            value.choices.into_iter().map(LLMInfo::from),
            value.preferred_codex_model_id.map(Into::into),
        )
    }
}

impl TryFrom<warp_graphql::workspace::AvailableLlms> for AvailableLLMs {
    type Error = anyhow::Error;

    fn try_from(value: warp_graphql::workspace::AvailableLlms) -> Result<Self, Self::Error> {
        Self::new(
            value.default_id.into(),
            value.choices.into_iter().map(LLMInfo::from),
            value.preferred_codex_model_id.map(Into::into),
        )
    }
}

impl From<warp_graphql::queries::get_feature_model_choices::LlmInfo> for LLMInfo {
    fn from(value: warp_graphql::queries::get_feature_model_choices::LlmInfo) -> Self {
        let host_configs = {
            let mut map = std::collections::HashMap::new();
            for config in value.host_configs {
                let config: RoutingHostConfig = config.into();
                let host = config.model_routing_host.clone();
                if map.insert(host.clone(), config).is_some() {
                    log::warn!(
                        "Duplicate LlmModelHost entry for {:?}, using latest value",
                        host
                    );
                }
            }
            map
        };
        Self {
            id: value.id.into(),
            display_name: value.display_name,
            base_model_name: value.base_model_name,
            reasoning_level: value.reasoning_level,
            usage_metadata: value.usage_metadata.into(),
            description: value.description,
            disable_reason: value.disable_reason.map(DisableReason::from),
            vision_supported: value.vision_supported,
            spec: value.spec.map(Into::into),
            provider: value.provider.into(),
            host_configs,
            discount_percentage: value.pricing.discount_percentage.map(|v| v as f32),
            context_window: LLMContextWindow {
                is_configurable: value.context_window.is_configurable,
                min: value.context_window.min.into(),
                max: value.context_window.max.into(),
                default_max: value.context_window.default.into(),
            },
        }
    }
}

impl From<warp_graphql::workspace::LlmInfo> for LLMInfo {
    fn from(value: warp_graphql::workspace::LlmInfo) -> Self {
        let host_configs = {
            let mut map = std::collections::HashMap::new();
            for config in value.host_configs {
                let config: RoutingHostConfig = config.into();
                let host = config.model_routing_host.clone();
                if map.insert(host.clone(), config).is_some() {
                    log::warn!(
                        "Duplicate LlmModelHost entry for {:?}, using latest value",
                        host
                    );
                }
            }
            map
        };
        Self {
            id: value.id.into(),
            display_name: value.display_name,
            base_model_name: value.base_model_name,
            reasoning_level: value.reasoning_level,
            usage_metadata: value.usage_metadata.into(),
            description: value.description,
            disable_reason: value.disable_reason.map(DisableReason::from),
            vision_supported: value.vision_supported,
            spec: value.spec.map(Into::into),
            provider: value.provider.into(),
            host_configs,
            discount_percentage: value.pricing.discount_percentage.map(|v| v as f32),
            context_window: LLMContextWindow {
                is_configurable: value.context_window.is_configurable,
                min: value.context_window.min.into(),
                max: value.context_window.max.into(),
                default_max: value.context_window.default.into(),
            },
        }
    }
}

impl From<warp_graphql::queries::get_feature_model_choices::RoutingHostConfig>
    for RoutingHostConfig
{
    fn from(value: warp_graphql::queries::get_feature_model_choices::RoutingHostConfig) -> Self {
        Self {
            enabled: value.enabled,
            model_routing_host: value.model_routing_host.into(),
        }
    }
}

impl From<warp_graphql::workspace::RoutingHostConfig> for RoutingHostConfig {
    fn from(value: warp_graphql::workspace::RoutingHostConfig) -> Self {
        Self {
            enabled: value.enabled,
            model_routing_host: value.model_routing_host.into(),
        }
    }
}

impl From<warp_graphql::queries::get_feature_model_choices::LlmModelHost> for LLMModelHost {
    fn from(value: warp_graphql::queries::get_feature_model_choices::LlmModelHost) -> Self {
        match value {
            warp_graphql::queries::get_feature_model_choices::LlmModelHost::DirectApi => {
                LLMModelHost::DirectApi
            }
            warp_graphql::queries::get_feature_model_choices::LlmModelHost::AwsBedrock => {
                LLMModelHost::AwsBedrock
            }
            warp_graphql::queries::get_feature_model_choices::LlmModelHost::CustomEndpoint => {
                LLMModelHost::CustomEndpoint
            }
            warp_graphql::queries::get_feature_model_choices::LlmModelHost::GeminiEnterprise => {
                LLMModelHost::GeminiEnterprise
            }
            warp_graphql::queries::get_feature_model_choices::LlmModelHost::Other(value) => {
                log::warn!(
                    "Unknown LlmModelHost '{value}'. Make sure to update client GraphQL types!"
                );
                LLMModelHost::Unknown
            }
        }
    }
}

impl From<warp_graphql::queries::get_feature_model_choices::LlmSpec> for LLMSpec {
    fn from(value: warp_graphql::queries::get_feature_model_choices::LlmSpec) -> Self {
        Self {
            cost: value.cost as f32,
            quality: value.quality as f32,
            speed: value.speed as f32,
        }
    }
}

impl From<warp_graphql::workspace::LlmSpec> for LLMSpec {
    fn from(value: warp_graphql::workspace::LlmSpec) -> Self {
        Self {
            cost: value.cost as f32,
            quality: value.quality as f32,
            speed: value.speed as f32,
        }
    }
}

impl From<warp_graphql::queries::get_feature_model_choices::LlmUsageMetadata> for LLMUsageMetadata {
    fn from(value: warp_graphql::queries::get_feature_model_choices::LlmUsageMetadata) -> Self {
        Self {
            request_multiplier: value.request_multiplier.max(1) as usize,
            credit_multiplier: value.credit_multiplier.map(|v| v as f32),
        }
    }
}

impl From<warp_graphql::workspace::LlmUsageMetadata> for LLMUsageMetadata {
    fn from(value: warp_graphql::workspace::LlmUsageMetadata) -> Self {
        Self {
            request_multiplier: value.request_multiplier.max(1) as usize,
            credit_multiplier: value.credit_multiplier.map(|v| v as f32),
        }
    }
}

impl From<warp_graphql::queries::get_feature_model_choices::DisableReason> for DisableReason {
    fn from(value: warp_graphql::queries::get_feature_model_choices::DisableReason) -> Self {
        match value {
            warp_graphql::queries::get_feature_model_choices::DisableReason::AdminDisabled => {
                DisableReason::AdminDisabled
            }
            warp_graphql::queries::get_feature_model_choices::DisableReason::OutOfRequests => {
                DisableReason::OutOfRequests
            }
            warp_graphql::queries::get_feature_model_choices::DisableReason::ProviderOutage => {
                DisableReason::ProviderOutage
            }
            warp_graphql::queries::get_feature_model_choices::DisableReason::RequiresUpgrade => {
                DisableReason::RequiresUpgrade
            }
            warp_graphql::queries::get_feature_model_choices::DisableReason::Other(_) => {
                DisableReason::Unavailable
            }
        }
    }
}

impl From<warp_graphql::workspace::DisableReason> for DisableReason {
    fn from(value: warp_graphql::workspace::DisableReason) -> Self {
        match value {
            warp_graphql::workspace::DisableReason::AdminDisabled => DisableReason::AdminDisabled,
            warp_graphql::workspace::DisableReason::OutOfRequests => DisableReason::OutOfRequests,
            warp_graphql::workspace::DisableReason::ProviderOutage => DisableReason::ProviderOutage,
            warp_graphql::workspace::DisableReason::RequiresUpgrade => {
                DisableReason::RequiresUpgrade
            }
            warp_graphql::workspace::DisableReason::Other(_) => DisableReason::Unavailable,
        }
    }
}

// Conversions for AIConversationMetadata from GraphQL types

fn convert_harness(harness: warp_graphql::ai::AgentHarness) -> AIAgentHarness {
    match harness {
        warp_graphql::ai::AgentHarness::Oz => AIAgentHarness::Oz,
        warp_graphql::ai::AgentHarness::ClaudeCode => AIAgentHarness::ClaudeCode,
        warp_graphql::ai::AgentHarness::Gemini => AIAgentHarness::Gemini,
        warp_graphql::ai::AgentHarness::Pi => AIAgentHarness::Pi,
        warp_graphql::ai::AgentHarness::Codex => AIAgentHarness::Codex,
        warp_graphql::ai::AgentHarness::Other(value) => {
            report_error!(
                "Invalid AgentHarness; update client GraphQL types",
                extra: { "harness" => %value },
                warp_errors::ReportErrorLogMode::OncePerRun
            );
            AIAgentHarness::Unknown
        }
    }
}

impl TryFrom<warp_graphql::ai::AIConversation> for ServerAIConversationMetadata {
    type Error = anyhow::Error;

    fn try_from(value: warp_graphql::ai::AIConversation) -> Result<Self, Self::Error> {
        // Full conversion including per-model token usage and tool usage
        // stats, so restored conversations render the same usage details
        // (e.g. the credits-expansion "Models" rows) as live ones.
        let usage: ConversationUsageMetadata = (&value.usage.usage_metadata).into();
        let metadata = value.metadata.try_into()?;
        let permissions = value.permissions.try_into()?;
        let ambient_agent_task_id = value
            .ambient_agent_task_id
            .map(|id| id.into_inner().parse())
            .transpose()?;
        let server_conversation_token =
            ServerConversationToken::new(value.conversation_id.into_inner());

        // If we fail to parse any artifacts, don't fail the entire conversion -- just don't include them in the list
        let artifacts = value
            .artifacts
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| Artifact::try_from(a).ok())
            .collect();

        Ok(Self {
            title: value.title,
            working_directory: value.working_directory,
            harness: convert_harness(value.harness),
            usage,
            metadata,
            creator: value.creator.map(Into::into),
            permissions,
            ambient_agent_task_id,
            server_conversation_token,
            artifacts,
        })
    }
}

impl TryFrom<warp_graphql::queries::list_ai_conversations::AIConversationMetadata>
    for ServerAIConversationMetadata
{
    type Error = anyhow::Error;

    fn try_from(
        value: warp_graphql::queries::list_ai_conversations::AIConversationMetadata,
    ) -> Result<Self, Self::Error> {
        let usage: ConversationUsageMetadata = (&value.usage.usage_metadata).into();
        let metadata = value.metadata.try_into()?;
        let permissions = value.permissions.try_into()?;
        let ambient_agent_task_id = value
            .ambient_agent_task_id
            .map(|id| id.into_inner().parse())
            .transpose()?;
        let server_conversation_token =
            ServerConversationToken::new(value.conversation_id.into_inner());

        let artifacts = value
            .artifacts
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| Artifact::try_from(a).ok())
            .collect();

        Ok(Self {
            title: value.title,
            working_directory: value.working_directory,
            harness: convert_harness(value.harness),
            usage,
            metadata,
            creator: value.creator.map(Into::into),
            permissions,
            ambient_agent_task_id,
            server_conversation_token,
            artifacts,
        })
    }
}

#[cfg(test)]
#[path = "ai_tests.rs"]
mod tests;
