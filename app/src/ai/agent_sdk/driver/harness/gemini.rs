use super::{ThirdPartyHarness, install_docs_url};
use crate::terminal::CLIAgent;
use warp_cli::agent::Harness;

pub(crate) struct GeminiHarness;

impl ThirdPartyHarness for GeminiHarness {
    fn harness(&self) -> Harness {
        Harness::Gemini
    }

    fn cli_agent(&self) -> CLIAgent {
        CLIAgent::Gemini
    }

    fn install_docs_url(&self) -> Option<&'static str> {
        install_docs_url(self.harness())
    }
}
