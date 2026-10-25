use crate::schema;
#[derive(cynic::Enum, Clone, Debug)]
#[cynic(non_exhaustive)]
pub enum Experiment {
    CloudPreferencesControl,
    CloudPreferencesGradualRolloutExperiment,
    EnvVarsEarlyAccessExperiment,
    LinuxLaunchExperiment,
    ReverseProTrialControl,
    ReverseProTrialExperiment,
    ReverseProTrialOneWeekControl,
    ReverseProTrialOneWeekExperiment,
    ReverseTrialControl,
    ReverseTrialExperiment,
    SessionSharingControl,
    SessionSharingExperiment,
    WindowsLaunchControl,
    WindowsLaunchExperiment,
    TmuxSshWarpificationControl,
    TmuxSshWarpificationExperiment,
    SshRemoteServerControl,
    SshRemoteServerExperiment,
    #[cynic(fallback)]
    Other(String),
}
