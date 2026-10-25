use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::AnsiColorIdentifier;
use warpui::{AppContext, SingletonEntity};

use crate::ui_components::blended_colors;
use crate::ui_components::icons::Icon;

/// Returns the size for icons in the AI block, scaled to the user's current font size.
pub fn icon_size(app: &AppContext) -> f32 {
    let appearance = Appearance::as_ref(app);
    app.font_cache().line_height(
        appearance.monospace_font_size(),
        appearance.line_height_ratio(),
    )
}

pub fn green_check_icon(appearance: &Appearance) -> warpui::elements::Icon {
    warpui::elements::Icon::new(
        Icon::Check.into(),
        AnsiColorIdentifier::Green.to_ansi_color(&appearance.theme().terminal_colors().normal),
    )
}

pub fn red_x_icon(appearance: &Appearance) -> warpui::elements::Icon {
    warpui::elements::Icon::new(
        Icon::X.into(),
        AnsiColorIdentifier::Red.to_ansi_color(&appearance.theme().terminal_colors().normal),
    )
}

pub fn cancelled_icon(appearance: &Appearance) -> warpui::elements::Icon {
    warpui::elements::Icon::new(
        Icon::Cancelled.into(),
        blended_colors::neutral_6(appearance.theme()),
    )
}

/// Not running, requires user's attention
pub fn yellow_stop_icon(appearance: &Appearance) -> warpui::elements::Icon {
    warpui::elements::Icon::new(
        Icon::StopFilled.into(),
        AnsiColorIdentifier::Yellow.to_ansi_color(&appearance.theme().terminal_colors().normal),
    )
}

/// To be used for actions (like running commands/reading files) that are long-running and executing.
pub fn yellow_running_icon(appearance: &Appearance) -> warpui::elements::Icon {
    warpui::elements::Icon::new(
        Icon::Circle.into(),
        AnsiColorIdentifier::Yellow.to_ansi_color(&appearance.theme().terminal_colors().normal),
    )
}
