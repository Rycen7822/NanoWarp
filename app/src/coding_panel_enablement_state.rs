#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CodingPanelEnablementState {
    Enabled,
    /// The active session is on a remote host.
    RemoteSession,
    UnsupportedSession,
}
