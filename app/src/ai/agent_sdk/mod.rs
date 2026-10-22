//! Shared agent execution support used by the GUI and its workers.

pub(crate) use driver::harness::{ClaudeHarness, task_env_vars, validate_cli_installed};

pub(crate) mod artifact_upload;
pub(crate) mod driver;
pub(crate) mod retry;
pub(crate) mod setup_observability;

pub(crate) fn run_environment_checkout(
    args: &warp_cli::environment_checkout::EnvironmentCheckoutArgs,
) -> anyhow::Result<()> {
    #[cfg(feature = "local_fs")]
    {
        driver::environment_checkout::run(args)
    }
    #[cfg(not(feature = "local_fs"))]
    {
        let _ = args;
        Err(anyhow::anyhow!(
            "environment checkout requires filesystem support"
        ))
    }
}
