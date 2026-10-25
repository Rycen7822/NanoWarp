//! Team-scoped reads of workspace settings, plus the [`TeamScope`] types that name which team a
//! read is for.
//!
use super::UserWorkspaces;
use crate::server::ids::ServerId;
use crate::workspaces::team::Team;
use crate::workspaces::workspace::Workspace;
use regex::Regex;
use std::rc::Rc;
use warpui::{AppContext, Entity, SingletonEntity, ViewContext, WeakViewHandle, WindowId};

mod sealed {
    pub trait Sealed {}
}

/// Reads a [`TeamContextForOperation`] or [`TeamContext`]'s team.
///
/// Application code obtains a [`TeamContext`] or [`TeamContextForOperation`] from a view-bound
/// context, handle, or window. Neither type can be copied or cloned. `TeamContext` is borrow-bound
/// to an immediate read, while `TeamContextForOperation` is owned so one operation can move it
/// across an asynchronous boundary without re-resolving against a different window team.
///
/// Sealed: only this module implements [`sealed::Sealed`], so a scope can never be minted
/// outside [`UserWorkspaces`].
#[allow(private_bounds)]
pub trait TeamScope: sealed::Sealed {
    fn team_uid(&self) -> Option<ServerId>;
}

/// The team selected when a view-scoped operation starts.
pub struct TeamContextForOperation {
    team_uid: Option<ServerId>,
}

impl sealed::Sealed for TeamContextForOperation {}

impl TeamScope for TeamContextForOperation {
    fn team_uid(&self) -> Option<ServerId> {
        self.team_uid
    }
}

#[cfg(test)]
impl TeamContextForOperation {
    pub(crate) fn new_for_test(team_uid: ServerId) -> Self {
        Self {
            team_uid: Some(team_uid),
        }
    }
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

pub struct ResolvedTeamScope(Option<ServerId>);

impl ResolvedTeamScope {
    pub fn from_scope(scope: &(impl TeamScope + ?Sized)) -> Self {
        Self(scope.team_uid())
    }
}

impl sealed::Sealed for ResolvedTeamScope {}

impl TeamScope for ResolvedTeamScope {
    fn team_uid(&self) -> Option<ServerId> {
        self.0
    }
}

/// A teamless [`TeamScope`] for tests that pass a scope without standing up a window.
#[cfg(test)]
pub(crate) struct TeamlessScopeForTest;

#[cfg(test)]
impl sealed::Sealed for TeamlessScopeForTest {}

#[cfg(test)]
impl TeamScope for TeamlessScopeForTest {
    fn team_uid(&self) -> Option<ServerId> {
        None
    }
}

/// Resolves a [`TeamContext`] on demand from a view captured up front. See
/// [`UserWorkspaces::team_context_resolver`].
pub type TeamContextResolver = Rc<dyn for<'a> Fn(&'a AppContext) -> TeamContext<'a>>;

impl UserWorkspaces {
    /// Captures the team selected in `ctx`'s window as an operation's
    /// [`TeamContextForOperation`]. Always succeeds -- a window with no team selected still yields
    /// a scope whose `team_uid()` is `None`.
    pub fn team_context_for_operation<T: Entity>(
        &self,
        ctx: &ViewContext<T>,
    ) -> TeamContextForOperation {
        self.team_context_for_window_operation(ctx.window_id())
    }
    /// Captures the team selected in a headless frontend's window.
    pub fn team_context_for_window_operation(
        &self,
        window_id: WindowId,
    ) -> TeamContextForOperation {
        TeamContextForOperation {
            team_uid: self.team_uid_for_window(window_id),
        }
    }

    pub(crate) fn team_context<'a, T: Entity>(
        &'a self,
        view: &WeakViewHandle<T>,
        app: &AppContext,
    ) -> TeamContext<'a> {
        let team_uid = self.team_for_view_handle(view, app).map(|team| &team.uid);
        TeamContext { team_uid }
    }

    /// Captures `view` as a reusable source of [`TeamContext`], for consumers that cannot name
    /// a view at the boundaries where they need one.
    pub fn team_context_resolver<T: Entity>(view: WeakViewHandle<T>) -> TeamContextResolver {
        Rc::new(move |app| Self::as_ref(app).team_context(&view, app))
    }

    fn team_context_for_window_id(&self, window_id: WindowId) -> TeamContext<'_> {
        TeamContext {
            team_uid: self
                .team_uid_for_window(window_id)
                .and_then(|team_uid| self.team_from_uid(team_uid))
                .map(|team| &team.uid),
        }
    }

    pub fn team_context_for_window(&self, window_id: WindowId) -> TeamContext<'_> {
        self.team_context_for_window_id(window_id)
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

    pub(crate) fn is_anyone_with_link_sharing_enabled<S: TeamScope + ?Sized>(
        &self,
        scope: &S,
    ) -> bool {
        self.scoped_or_workspace_setting(
            scope,
            |team| {
                team.settings
                    .link_sharing
                    .anyone_with_link_sharing_enabled
                    .value
            },
            |workspace| {
                workspace
                    .settings
                    .link_sharing_settings
                    .anyone_with_link_sharing_enabled
            },
            true,
        )
    }

    pub(crate) fn is_direct_link_sharing_enabled<S: TeamScope + ?Sized>(&self, scope: &S) -> bool {
        self.scoped_or_workspace_setting(
            scope,
            |team| team.settings.link_sharing.direct_link_sharing_enabled.value,
            |workspace| {
                workspace
                    .settings
                    .link_sharing_settings
                    .direct_link_sharing_enabled
            },
            true,
        )
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
