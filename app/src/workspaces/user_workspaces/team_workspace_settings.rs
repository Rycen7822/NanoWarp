//! Team-scoped reads of workspace settings, plus the [`TeamScope`] types that name which team a
//! read is for.
//!
use super::UserWorkspaces;
use crate::server::ids::ServerId;
use crate::workspaces::team::Team;
use crate::workspaces::workspace::Workspace;
use regex::Regex;
use warpui::{AppContext, Entity, WeakViewHandle};

mod sealed {
    pub trait Sealed {}
}

/// Reads the team of a view-bound [`TeamContext`] during an immediate read.
///
/// Sealed: only this module implements [`sealed::Sealed`], so a scope can never be minted
/// outside [`UserWorkspaces`].
#[allow(private_bounds)]
pub trait TeamScope: sealed::Sealed {
    fn team_uid(&self) -> Option<ServerId>;
}

/// The team a view renders as, borrowed for the duration of a single read.
///
/// It is resolved at the point of use so policy reads follow the view between windows.
pub struct TeamContext<'a> {
    team_uid: Option<&'a ServerId>,
}

impl sealed::Sealed for TeamContext<'_> {}

impl TeamScope for TeamContext<'_> {
    fn team_uid(&self) -> Option<ServerId> {
        self.team_uid.copied()
    }
}

impl UserWorkspaces {
    pub(crate) fn team_context<'a, T: Entity>(
        &'a self,
        view: &WeakViewHandle<T>,
        app: &AppContext,
    ) -> TeamContext<'a> {
        let team_uid = self.team_for_view_handle(view, app).map(|team| &team.uid);
        TeamContext { team_uid }
    }

    /// The team a scope names, when it names one that is still in the current workspace.
    ///
    /// Deliberately private. Callers get a resolved *setting* from a getter that takes their
    /// scope, never a `&Team` they could carry somewhere the scope never reached. Wanting a
    /// `&Team` at a call site means the read belongs behind a new getter here instead.
    fn team_from_scope<S: TeamScope + ?Sized>(&self, scope: &S) -> Option<&Team> {
        scope
            .team_uid()
            .and_then(|team_uid| self.team_from_uid(team_uid))
    }

    /// Resolves a per-team setting for `scope`: the scope's own team when it names one, otherwise
    /// `current_workspace().settings`.
    ///
    /// A scope naming an unresolvable team yields `absent`, never another team's value. The
    /// no-team branch reads `current_workspace().settings` unconditionally; for a member on teams
    /// that is the server's arbitrarily-elected stand-in (see [`TeamScope`]), a deliberate
    /// simplification because a windowed terminal is never expected to present a teamless scope,
    /// so in practice only a genuinely teamless user reaches it, whose workspace settings the
    /// server computes from tier defaults.
    fn scoped_or_workspace_setting<'a, S: TeamScope + ?Sized, T>(
        &'a self,
        scope: &S,
        from_team: impl FnOnce(&'a Team) -> T,
        from_workspace: impl FnOnce(&'a Workspace) -> T,
        absent: T,
    ) -> T {
        match scope.team_uid() {
            Some(_) => self.team_from_scope(scope).map_or(absent, from_team),
            None => self.current_workspace().map_or(absent, from_workspace),
        }
    }

    /// The remote-session command patterns configured by `scope`'s team. See
    /// [`Self::scoped_or_workspace_setting`] for the no-team fallback.
    pub(crate) fn get_remote_session_regex_list<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> &[Regex] {
        self.scoped_or_workspace_setting(
            scope,
            |team| team.settings.remote_session_regex_list.as_slice(),
            |workspace| workspace.settings.remote_session_regex_list.as_slice(),
            &[],
        )
    }
}
