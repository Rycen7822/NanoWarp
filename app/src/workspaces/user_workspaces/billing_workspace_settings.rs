//! Workspace-level billing/plan accessors: reads of `workspace.billing_metadata` (tier and
//! policy entitlements) that hold regardless of which team a window has selected. See
//! [`crate::workspaces::user_workspaces::team_workspace_settings`] for the workspace-vs-team
//! two-layer model and the team-scoped policies that layer on top.

use super::UserWorkspaces;
use crate::workspaces::team::Team;
use crate::workspaces::workspace::BillingMetadata;

impl UserWorkspaces {
    pub fn current_workspace_billing_metadata(&self) -> Option<&BillingMetadata> {
        self.current_workspace()
            .map(|workspace| &workspace.billing_metadata)
    }

    /// The given team's billing metadata when the team is known, otherwise
    /// the current workspace's, including team/workspace-scoped delinquency state.
    pub fn team_billing_metadata<'a>(
        &'a self,
        team: Option<&'a Team>,
    ) -> Option<&'a BillingMetadata> {
        team.map(|team| &team.billing_metadata)
            .or_else(|| self.current_workspace_billing_metadata())
    }
}
