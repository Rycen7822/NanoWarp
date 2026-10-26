use pathfinder_color::ColorU;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::ColorScheme;
use warpui::Element;
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{CacheOption, ConstrainedBox, Container, CornerRadius, Image, Radius};
use warpui::ui_components::button::ButtonVariant;

use crate::themes::theme::ThemeKind;

pub fn action_button_color_and_variant(appearance: &Appearance) -> (ColorU, ButtonVariant) {
    let (button_color, button_variant) = match appearance.theme().name() {
        Some(name) if ThemeKind::Dark.matches(&name) => {
            (ColorU::new(0, 109, 168, 255), ButtonVariant::Basic)
        }
        Some(_) => (appearance.theme().accent().into(), ButtonVariant::Accent),
        None => (appearance.theme().accent().into(), ButtonVariant::Accent),
    };
    (button_color, button_variant)
}

pub fn render_square_logo(appearance: &Appearance) -> Box<dyn Element> {
    let image_path = if appearance.theme().inferred_color_scheme() == ColorScheme::LightOnDark {
        "bundled/svg/warp-logo-light.svg"
    } else {
        "bundled/svg/warp-logo-dark.svg"
    };

    ConstrainedBox::new(
        Container::new(
            Image::new(
                AssetSource::Bundled { path: image_path },
                CacheOption::BySize,
            )
            .finish(),
        )
        .with_background(appearance.theme().surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
        .with_horizontal_padding(11.)
        .finish(),
    )
    .with_width(64.)
    .with_height(64.)
    .finish()
}
