use warp_core::ui::icons::Icon as WarpIcon;
use warp_core::ui::theme::color::internal_colors;
use warp_core::ui::theme::{Fill as WarpThemeFill, WarpTheme};
use warpui::elements::{ConstrainedBox, Container, CornerRadius, Element, Radius};

// Neutral variants have no overlay, so they fill the full `total_size` bounding box. The
// inner glyph occupies `NEUTRAL_GLYPH_RATIO * total_size`, matching the old sizing where
// a 24px container held a 16px glyph (16/24 ≈ 0.667).
const NEUTRAL_GLYPH_RATIO: f32 = 16.0 / 24.0;

/// What to render inside the circle.
pub(crate) enum PaneIcon {
    /// A generic icon with a given color on an overlay background.
    Neutral {
        icon: WarpIcon,
        icon_color: WarpThemeFill,
    },
    /// A pre-built icon element on an overlay background.
    NeutralElement { icon_element: Box<dyn Element> },
}

/// Renders a pane icon inside a circular background.
pub(crate) fn render_pane_icon(
    variant: PaneIcon,
    total_size: f32,
    theme: &WarpTheme,
) -> Box<dyn Element> {
    match variant {
        PaneIcon::Neutral { icon, icon_color } => render_neutral_circle(
            icon.to_warpui_icon(icon_color).finish(),
            internal_colors::fg_overlay_2(theme),
            total_size,
        ),
        PaneIcon::NeutralElement { icon_element } => render_neutral_circle(
            icon_element,
            internal_colors::fg_overlay_2(theme),
            total_size,
        ),
    }
}

/// Builds the neutral circle: a full-`total_size` container with the glyph at
/// `NEUTRAL_GLYPH_RATIO * total_size`. Used for non-agent surfaces (plain terminal,
/// code, file tabs, etc.) which have no status overlay and therefore should fill the
/// requested bounding box.
fn render_neutral_circle(
    icon_element: Box<dyn Element>,
    background: WarpThemeFill,
    total_size: f32,
) -> Box<dyn Element> {
    let glyph = total_size * NEUTRAL_GLYPH_RATIO;
    let padding = (total_size - glyph) / 2.;
    let inner = ConstrainedBox::new(icon_element)
        .with_width(glyph)
        .with_height(glyph)
        .finish();
    Container::new(inner)
        .with_uniform_padding(padding)
        .with_background(background)
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(total_size / 2.)))
        .finish()
}
