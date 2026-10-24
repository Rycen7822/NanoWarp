//! CLI agent brand metadata.
//!
//! This module provides display metadata for CLI-based AI agents
//! like Claude Code, Gemini CLI, Codex, Amp, Droid, OpenCode, Grok Build, and Muse Code.

use enum_iterator::Sequence;
use pathfinder_color::ColorU;
use serde::{Deserialize, Serialize};
use warp_cli::agent::Harness;

use crate::ai::blocklist::CLAUDE_ORANGE;
use crate::ui_components::icons::Icon;

/// Gemini brand blue color
pub(crate) const GEMINI_BLUE: ColorU = ColorU {
    r: 66,
    g: 133,
    b: 244,
    a: 255,
};

/// OpenAI brand color (dark gray/black)
pub(crate) const OPENAI_COLOR: ColorU = ColorU {
    r: 0,
    g: 0,
    b: 0,
    a: 255,
};

/// Amp brand color (#F34E3F)
const AMP_COLOR: ColorU = ColorU {
    r: 243,
    g: 78,
    b: 63,
    a: 255,
};

/// Droid brand color (white)
const DROID_COLOR: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// OpenCode brand color (gray, used for contrast calculation only)
pub(crate) const OPENCODE_COLOR: ColorU = ColorU {
    r: 128,
    g: 128,
    b: 128,
    a: 255,
};

/// Copilot brand color (Copilot purple selected from https://brand.github.com/brand-identity/copilot)
const COPILOT_COLOR: ColorU = ColorU {
    r: 133,
    g: 52,
    b: 243,
    a: 255,
};

/// Pi brand color (white, monochrome logo)
const PI_COLOR: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// Antigravity brand color (white, monochrome logo)
const ANTIGRAVITY_COLOR: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// Auggie brand color (white, monochrome logo)
const AUGGIE_COLOR: ColorU = ColorU {
    r: 255,
    g: 255,
    b: 255,
    a: 255,
};

/// Cursor brand color (#26251E, from official brand assets)
const CURSOR_COLOR: ColorU = ColorU {
    r: 38,
    g: 37,
    b: 30,
    a: 255,
};

/// Goose brand color (#101010, from Block's official Goose logo)
const GOOSE_COLOR: ColorU = ColorU {
    r: 16,
    g: 16,
    b: 16,
    a: 255,
};

/// Hermes brand color (Nous Research purple #7C3AED)
const HERMES_PURPLE: ColorU = ColorU {
    r: 124,
    g: 58,
    b: 237,
    a: 255,
};

/// Mistral brand orange (#FA520F)
const MISTRAL_ORANGE: ColorU = ColorU {
    r: 250,
    g: 82,
    b: 15,
    a: 255,
};

/// Grok / SpaceXAI brand color (near-black, monochrome logomark on circular tile)
const GROK_COLOR: ColorU = ColorU {
    r: 16,
    g: 16,
    b: 16,
    a: 255,
};

const KIRO_PURPLE: ColorU = ColorU {
    r: 144,
    g: 70,
    b: 255,
    a: 255,
};

/// Muse Code / Meta brand blue (#0081FB)
const MUSE_COLOR: ColorU = ColorU {
    r: 0,
    g: 129,
    b: 251,
    a: 255,
};

/// Represents a CLI agent (e.g., Claude Code, Gemini CLI, Codex, Amp, Droid, OpenCode, Copilot, Pi, Auggie, Cursor, Goose, Hermes, Mistral Vibe, Grok Build, Kiro CLI, Muse Code)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Sequence, Serialize, Deserialize)]
pub enum CLIAgent {
    Claude,
    Gemini,
    Codex,
    Amp,
    Droid,
    OpenCode,
    Copilot,
    Pi,
    OhMyPi,
    Auggie,
    CursorCli,
    Goose,
    Hermes,
    Vibe,
    Antigravity,
    Grok,
    Kiro,
    Muse,
    /// Represents an unknown/custom CLI agent matched by user-configured regex patterns.
    Unknown,
}

impl CLIAgent {
    /// Command prefixes that identify this CLI agent.
    pub(crate) fn command_prefixes(&self) -> &'static [&'static str] {
        match self {
            CLIAgent::Claude => &["claude"],
            CLIAgent::Gemini => &["gemini"],
            CLIAgent::Codex => &["codex"],
            CLIAgent::Amp => &["amp"],
            CLIAgent::Droid => &["droid"],
            CLIAgent::OpenCode => &["opencode"],
            CLIAgent::Copilot => &["copilot"],
            CLIAgent::Pi => &["pi"],
            CLIAgent::OhMyPi => &["omp"],
            CLIAgent::Auggie => &["auggie"],
            CLIAgent::CursorCli => &["agent"],
            CLIAgent::Goose => &["goose"],
            CLIAgent::Hermes => &["hermes"],
            CLIAgent::Vibe => &["vibe", "vibe-acp"],
            CLIAgent::Antigravity => &["agy", "antigravity"],
            CLIAgent::Grok => &["grok"],
            CLIAgent::Kiro => &["kiro-cli", "kiro"],
            CLIAgent::Muse => &["muse"],
            CLIAgent::Unknown => &[],
        }
    }

    /// The canonical command prefix used to identify this CLI agent in places
    /// that require one stable value.
    pub fn command_prefix(&self) -> &'static str {
        self.command_prefixes().first().copied().unwrap_or_default()
    }

    /// Returns the [`CLIAgent`] corresponding to a [`Harness`] when it represents a
    /// third-party agent. Returns `None` for [`Harness::Oz`] (Warp's built-in harness has no
    /// distinct CLI agent identity).
    pub fn from_harness(harness: Harness) -> Option<Self> {
        match harness {
            Harness::Oz => None,
            Harness::Claude => Some(CLIAgent::Claude),
            Harness::Gemini => Some(CLIAgent::Gemini),
            Harness::OpenCode => Some(CLIAgent::OpenCode),
            Harness::Codex => Some(CLIAgent::Codex),
            Harness::Pi | Harness::Unknown => Some(CLIAgent::Unknown),
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            CLIAgent::Claude => "Claude Code",
            CLIAgent::Gemini => "Gemini",
            CLIAgent::Codex => "Codex",
            CLIAgent::Amp => "Amp",
            CLIAgent::Droid => "Droid",
            CLIAgent::OpenCode => "OpenCode",
            CLIAgent::Copilot => "Copilot",
            CLIAgent::Pi => "Pi",
            CLIAgent::OhMyPi => "oh-my-pi",
            CLIAgent::Auggie => "Auggie",
            CLIAgent::CursorCli => "Cursor",
            CLIAgent::Goose => "Goose",
            CLIAgent::Hermes => "Hermes",
            CLIAgent::Vibe => "Mistral Vibe",
            CLIAgent::Antigravity => "Antigravity",
            CLIAgent::Grok => "Grok Build",
            CLIAgent::Kiro => "Kiro CLI",
            CLIAgent::Muse => "Muse Code",
            CLIAgent::Unknown => "CLI Agent",
        }
    }

    /// Returns the Icon for this CLI agent, or `None` for unknown/custom agents.
    pub fn icon(&self) -> Option<Icon> {
        match self {
            CLIAgent::Claude => Some(Icon::ClaudeLogo),
            CLIAgent::Gemini => Some(Icon::GeminiLogo),
            CLIAgent::Codex => Some(Icon::OpenAILogo),
            CLIAgent::Amp => Some(Icon::AmpLogo),
            CLIAgent::Droid => Some(Icon::DroidLogo),
            CLIAgent::OpenCode => Some(Icon::OpenCodeLogo),
            CLIAgent::Copilot => Some(Icon::CopilotLogo),
            CLIAgent::Pi => Some(Icon::PiLogo),
            CLIAgent::OhMyPi => Some(Icon::OhMyPiLogo),
            CLIAgent::Auggie => Some(Icon::AuggieLogo),
            CLIAgent::CursorCli => Some(Icon::CursorLogo),
            CLIAgent::Goose => Some(Icon::GooseLogo),
            CLIAgent::Hermes => None,
            // Vibe is recognized but ships without a brand asset. The brand color
            // still drives the toolbar tile; an `Icon::MistralLogo` can be wired
            // up in a follow-up once an officially licensed SVG is available.
            CLIAgent::Vibe => None,
            CLIAgent::Antigravity => Some(Icon::AntigravityLogo),
            CLIAgent::Grok => Some(Icon::GrokLogo),
            CLIAgent::Kiro => Some(Icon::KiroLogo),
            CLIAgent::Muse => Some(Icon::MuseLogo),
            CLIAgent::Unknown => None,
        }
    }

    /// Returns the brand color for this CLI agent, or `None` for unknown/custom agents.
    pub fn brand_color(&self) -> Option<ColorU> {
        match self {
            CLIAgent::Claude => Some(CLAUDE_ORANGE),
            CLIAgent::Gemini => Some(GEMINI_BLUE),
            CLIAgent::Codex => Some(OPENAI_COLOR),
            CLIAgent::Amp => Some(AMP_COLOR),
            CLIAgent::Droid => Some(DROID_COLOR),
            CLIAgent::OpenCode => Some(OPENCODE_COLOR),
            CLIAgent::Copilot => Some(COPILOT_COLOR),
            CLIAgent::Pi => Some(PI_COLOR),
            CLIAgent::OhMyPi => Some(PI_COLOR),
            CLIAgent::Auggie => Some(AUGGIE_COLOR),
            CLIAgent::CursorCli => Some(CURSOR_COLOR),
            CLIAgent::Goose => Some(GOOSE_COLOR),
            CLIAgent::Hermes => Some(HERMES_PURPLE),
            CLIAgent::Vibe => Some(MISTRAL_ORANGE),
            CLIAgent::Antigravity => Some(ANTIGRAVITY_COLOR),
            CLIAgent::Grok => Some(GROK_COLOR),
            CLIAgent::Kiro => Some(KIRO_PURPLE),
            CLIAgent::Muse => Some(MUSE_COLOR),
            CLIAgent::Unknown => None,
        }
    }

    /// Returns the icon color to use when rendered on the brand-colored circle background.
    /// Agents with light brand colors use a dark icon for contrast.
    pub fn brand_icon_color(&self) -> ColorU {
        match self {
            CLIAgent::Pi
            | CLIAgent::OhMyPi
            | CLIAgent::Auggie
            | CLIAgent::Droid
            | CLIAgent::Antigravity => ColorU::new(0, 0, 0, 255),
            _ => ColorU::white(),
        }
    }
}

#[cfg(test)]
#[path = "cli_agent_tests.rs"]
mod tests;
