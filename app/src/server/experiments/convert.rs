//! Logic to convert from / to [`ServerExperiment`].

use std::fmt::{Display, Formatter};

use anyhow::{Ok, Result};
use warp_graphql::experiment::Experiment;

use super::ServerExperiment;

impl Display for ServerExperiment {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            Self::SessionSharingControl => "SESSION_SHARING_CONTROL",
            Self::SessionSharingExperiment => "SESSION_SHARING_EXPERIMENT",
            Self::EnvVarsEarlyAccessExperiment => "ENV_VARS_EARLY_ACCESS_EXPERIMENT",
            Self::WindowsLaunchExperiment => "WINDOWS_LAUNCH_EXPERIMENT",
            #[cfg(test)]
            Self::TestExperiment => "TEST_EXPERIMENT",
        };
        write!(f, "{str}")
    }
}

impl ServerExperiment {
    pub fn from_string(s: String) -> Result<Self> {
        match s.as_str() {
            "SESSION_SHARING_CONTROL" => Ok(Self::SessionSharingControl),
            "SESSION_SHARING_EXPERIMENT" => Ok(Self::SessionSharingExperiment),
            "ENV_VARS_EARLY_ACCESS_EXPERIMENT" => Ok(Self::EnvVarsEarlyAccessExperiment),
            "WINDOWS_LAUNCH_EXPERIMENT" => Ok(Self::WindowsLaunchExperiment),
            s => Err(anyhow::anyhow!(
                "String doesn't match any server experiment variant {s}"
            )),
        }
    }
}

impl TryFrom<Experiment> for ServerExperiment {
    type Error = anyhow::Error;

    fn try_from(value: Experiment) -> Result<Self, Self::Error> {
        match value {
            Experiment::SessionSharingExperiment => Ok(Self::SessionSharingExperiment),
            Experiment::SessionSharingControl => Ok(Self::SessionSharingControl),
            Experiment::EnvVarsEarlyAccessExperiment => Ok(Self::EnvVarsEarlyAccessExperiment),
            Experiment::WindowsLaunchExperiment => Ok(Self::WindowsLaunchExperiment),
            // Experiments that we no longer support on the client.
            e => Err(anyhow::anyhow!(
                "Server-side enabled experiment '{e:?}' is no longer supported by the client."
            )),
        }
    }
}

#[macro_export]
macro_rules! convert_to_server_experiment {
    ($gql_type:expr_2021) => {{
        let mut acc = Vec::new();
        for a in $gql_type {
            // Note for server experiments we don't currently track on the client.
            // This could be because the client is out of date and we should still
            // apply the experiments the client does track.
            if let Ok(b) = ServerExperiment::try_from(a) {
                acc.push(b);
            }
        }
        Some(acc)
    }};
}
