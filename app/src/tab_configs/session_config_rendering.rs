use std::path::Path;

use warpui::elements::{
    Border, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Flex, Hoverable,
    MouseStateHandle, ParentElement, Radius, Text,
};
use warpui::fonts::{Properties, Weight};
use warpui::geometry::vector::Vector2F;
use warpui::platform::Cursor;
use warpui::{Element, EventContext};

use crate::appearance::Appearance;
use crate::ui_components::blended_colors;

/// Renders the directory picker button.
pub fn render_directory_picker<F>(
    selected_directory: &Path,
    mouse_state: MouseStateHandle,
    on_click: F,
    appearance: &Appearance,
) -> Box<dyn Element>
where
    F: Fn(&mut EventContext, Vector2F) + 'static,
{
    let theme = appearance.theme();
    let bg_fill = theme.background();

    let label = Text::new_inline(
        "Select directory".to_string(),
        appearance.ui_font_family(),
        12.,
    )
    .with_color(blended_colors::text_disabled(theme, bg_fill))
    .finish();

    let home_dir = dirs::home_dir();
    let raw_path = selected_directory.to_string_lossy();
    let dir_display =
        warp_util::path::user_friendly_path(&raw_path, home_dir.as_ref().and_then(|h| h.to_str()))
            .into_owned();

    let dir_text = Text::new_inline(dir_display, appearance.ui_font_family(), 14.)
        .with_color(blended_colors::text_main(theme, bg_fill))
        .with_style(Properties::default().weight(Weight::Semibold))
        .finish();

    let border_color = blended_colors::neutral_4(theme);

    let button = Hoverable::new(mouse_state, move |_| {
        let content_row = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(dir_text)
            .finish();

        Container::new(ConstrainedBox::new(content_row).with_height(30.).finish())
            .with_horizontal_padding(12.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .with_border(Border::all(1.).with_border_color(border_color))
            .finish()
    })
    .with_cursor(Cursor::PointingHand)
    .on_click(move |ctx, _, position| {
        on_click(ctx, position);
    })
    .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(label)
        .with_child(Container::new(button).with_margin_top(4.).finish())
        .finish()
}
