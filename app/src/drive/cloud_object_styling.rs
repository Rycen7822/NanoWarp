use pathfinder_color::ColorU;
use warp_core::ui::appearance::Appearance;

use crate::ui_components::blended_colors;

pub fn warp_drive_icon_color(appearance: &Appearance) -> ColorU {
    // Match File Tree styling - use text_sub color
    blended_colors::text_sub(appearance.theme(), appearance.theme().background())
}
