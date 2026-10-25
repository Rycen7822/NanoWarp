use anyhow::{Result, anyhow};
use async_trait::async_trait;
use cynic::QueryBuilder;
#[cfg(test)]
use mockall::{automock, predicate::*};
use warp_graphql::queries::get_workspaces_metadata_for_user::{
    GetWorkspacesMetadataForUser, GetWorkspacesMetadataForUserVariables,
};

use super::ServerApi;
use crate::server::graphql::get_request_context;
use crate::workspaces::gql_convert::workspaces_metadata_response_from_gql;
use crate::workspaces::user_workspaces::WorkspacesMetadataResponse;

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait TeamClient: 'static + Send + Sync {
    async fn workspaces_metadata(&self) -> Result<WorkspacesMetadataResponse>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl TeamClient for ServerApi {
    #[tracing::instrument(skip_all, err)]
    async fn workspaces_metadata(&self) -> Result<WorkspacesMetadataResponse> {
        let variables = GetWorkspacesMetadataForUserVariables {
            request_context: get_request_context(),
        };
        let operation = GetWorkspacesMetadataForUser::build(variables);
        let response = self.send_graphql_request(operation, None).await?;

        let metadata = match response.user {
            warp_graphql::queries::get_workspaces_metadata_for_user::UserResult::UserOutput(
                user_output,
            ) => workspaces_metadata_response_from_gql(user_output.user, self.is_service_account()),
            warp_graphql::queries::get_workspaces_metadata_for_user::UserResult::Unknown => {
                return Err(anyhow!("Unable to fetch workspaces metadata"));
            }
        };

        Ok(metadata)
    }
}
