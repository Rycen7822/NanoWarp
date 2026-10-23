use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cynic::{MutationBuilder, QueryBuilder};
#[cfg(test)]
use mockall::{automock, predicate::*};
use warp_graphql::mutations::remove_user_from_workspace::{
    RemoveUserFromWorkspace, RemoveUserFromWorkspaceInput, RemoveUserFromWorkspaceResult,
    RemoveUserFromWorkspaceVariables,
};
use warp_graphql::queries::get_ai_overages_for_workspace::{
    GetAiOveragesForWorkspace, GetAiOveragesForWorkspaceVariables, UserResult,
};

use super::ServerApi;
use super::team::TeamClient;
use crate::auth::UserUid;
use crate::cloud_object::CloudObjectEventEntrypoint;
use crate::server::graphql::{get_request_context, get_user_facing_error_message};
use crate::workspaces::user_workspaces::WorkspacesMetadataWithPricing;
use crate::workspaces::workspace::{AiOverages, WorkspaceUid};

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait WorkspaceClient: 'static + Send + Sync {
    async fn remove_user_from_workspace(
        &self,
        user_uid: UserUid,
        workspace_uid: WorkspaceUid,
        entrypoint: CloudObjectEventEntrypoint,
    ) -> Result<WorkspacesMetadataWithPricing>;

    async fn refresh_ai_overages(&self) -> Result<AiOverages>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl WorkspaceClient for ServerApi {
    async fn remove_user_from_workspace(
        &self,
        user_uid: UserUid,
        workspace_uid: WorkspaceUid,
        entrypoint: CloudObjectEventEntrypoint,
    ) -> Result<WorkspacesMetadataWithPricing> {
        let variables = RemoveUserFromWorkspaceVariables {
            input: RemoveUserFromWorkspaceInput {
                user_uid: user_uid.as_str().into(),
                workspace_uid: String::from(workspace_uid).into(),
                entrypoint: entrypoint.into(),
            },
            request_context: get_request_context(),
        };
        let operation = RemoveUserFromWorkspace::build(variables);
        let result = self
            .send_graphql_request(operation, None)
            .await?
            .remove_user_from_workspace;

        match result {
            RemoveUserFromWorkspaceResult::RemoveUserFromWorkspaceOutput(output) => {
                if output.success {
                    self.workspaces_metadata().await
                } else {
                    Err(anyhow!("failed to remove user from workspace"))
                }
            }
            RemoveUserFromWorkspaceResult::UserFacingError(error) => {
                Err(anyhow!(get_user_facing_error_message(error)))
            }
            RemoveUserFromWorkspaceResult::Unknown => {
                Err(anyhow!("unknown error while removing user from workspace"))
            }
        }
    }

    async fn refresh_ai_overages(&self) -> Result<AiOverages> {
        let variables = GetAiOveragesForWorkspaceVariables {
            request_context: get_request_context(),
        };
        let operation = GetAiOveragesForWorkspace::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        match response.user {
            UserResult::UserOutput(user_output) => user_output
                .user
                .workspaces
                .first()
                .as_ref()
                .ok_or_else(|| anyhow!("No workspace found"))?
                .billing_metadata
                .ai_overages
                .as_ref()
                .ok_or_else(|| anyhow!("No AI overages found"))
                .map(|overages| AiOverages {
                    current_monthly_request_cost_cents: overages.current_monthly_request_cost_cents,
                    current_monthly_requests_used: overages.current_monthly_requests_used,
                    current_period_end: overages.current_period_end.utc(),
                }),
            UserResult::Unknown => Err(anyhow!("Unknown error")),
        }
    }
}
