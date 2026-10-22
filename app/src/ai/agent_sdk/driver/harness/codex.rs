use super::{ThirdPartyHarness, install_docs_url};
use crate::terminal::CLIAgent;
use warp_cli::agent::Harness;

pub(crate) struct CodexHarness;

impl ThirdPartyHarness for CodexHarness {
    fn harness(&self) -> Harness {
        Harness::Codex
    }

    fn cli_agent(&self) -> CLIAgent {
        CLIAgent::Codex
    }

    fn install_docs_url(&self) -> Option<&'static str> {
        install_docs_url(self.harness())
    }

    fn auth_check_command(&self) -> Option<String> {
        let cli = self.cli_agent().command_prefix();
        Some(format!("{cli} login status"))
    }
}
