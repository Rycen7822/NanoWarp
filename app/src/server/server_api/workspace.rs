use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cynic::QueryBuilder;
#[cfg(test)]
use mockall::{automock, predicate::*};
use warp_graphql::queries::get_ai_overages_for_workspace::{
    GetAiOveragesForWorkspace, GetAiOveragesForWorkspaceVariables, UserResult,
};

use super::ServerApi;
use crate::server::graphql::get_request_context;
use crate::workspaces::workspace::AiOverages;

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait WorkspaceClient: 'static + Send + Sync {
    async fn refresh_ai_overages(&self) -> Result<AiOverages>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl WorkspaceClient for ServerApi {
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
