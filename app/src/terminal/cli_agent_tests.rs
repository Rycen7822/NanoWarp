use super::CLIAgent;
use crate::ui_components::icons::Icon;

#[test]
fn test_grok_public_configuration() {
    assert_eq!(CLIAgent::Grok.command_prefix(), "grok");
    assert_eq!(CLIAgent::Grok.display_name(), "Grok Build");
    assert_eq!(CLIAgent::Grok.icon(), Some(Icon::GrokLogo));
}

#[test]
fn test_muse_public_configuration() {
    assert_eq!(CLIAgent::Muse.command_prefix(), "muse");
    assert_eq!(CLIAgent::Muse.display_name(), "Muse Code");
    assert_eq!(CLIAgent::Muse.icon(), Some(Icon::MuseLogo));
}
