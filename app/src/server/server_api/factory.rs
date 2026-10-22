use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cynic::QueryBuilder;
#[cfg(test)]
use mockall::automock;
use warp_graphql::queries::get_runners::{
    GetRunners, GetRunnersResult, GetRunnersVariables, Runner, RunnerSortBy,
};

use super::ServerApi;
use crate::server::graphql::{get_request_context, get_user_facing_error_message};
use crate::server::team_scope::RequestTeamScope;

/// Client for listing Factory runners.
#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait FactoryClient: 'static + Send + Sync {
    /// Fetch all runners visible to the caller, optionally sorted.
    async fn get_runners(
        &self,
        sort_by: Option<RunnerSortBy>,
        team_scope: Option<RequestTeamScope>,
    ) -> Result<Vec<Runner>>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl FactoryClient for ServerApi {
    async fn get_runners(
        &self,
        sort_by: Option<RunnerSortBy>,
        team_scope: Option<RequestTeamScope>,
    ) -> Result<Vec<Runner>> {
        let operation = GetRunners::build(GetRunnersVariables {
            request_context: get_request_context(),
            sort_by,
        });
        let response = match team_scope {
            Some(team_scope) => {
                self.send_graphql_request_for_team(operation, team_scope)
                    .await?
            }
            None => self.send_graphql_request(operation, None).await?,
        };
        match response.get_runners {
            GetRunnersResult::GetRunnersOutput(output) => Ok(output.runners),
            GetRunnersResult::UserFacingError(e) => Err(anyhow!(get_user_facing_error_message(e))),
            GetRunnersResult::Unknown => Err(anyhow!("failed to list runners")),
        }
    }
}
