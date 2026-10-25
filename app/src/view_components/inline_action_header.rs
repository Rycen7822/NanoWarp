use std::borrow::Cow;

use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    Container, CornerRadius, CrossAxisAlignment, Expanded, Flex, MainAxisAlignment, MainAxisSize,
    ParentElement, Radius, Shrinkable, Text,
};
use warpui::fonts::FamilyId;
use warpui::{AppContext, Element, SingletonEntity};

use crate::ui_components::blended_colors;

pub const INLINE_ACTION_HORIZONTAL_PADDING: f32 = 16.;
pub const INLINE_ACTION_HEADER_VERTICAL_PADDING: f32 = 10.;

pub struct HeaderConfig {
    pub title: Cow<'static, str>,
    pub font_family: FamilyId,
    pub corner_radius_override: Option<CornerRadius>,
}

impl HeaderConfig {
    pub fn new(title: impl Into<Cow<'static, str>>, app: &AppContext) -> Self {
        Self {
            title: title.into(),
            font_family: Appearance::as_ref(app).ui_font_family(),
            corner_radius_override: None,
        }
    }

    pub fn with_corner_radius_override(mut self, corner_radius: CornerRadius) -> Self {
        self.corner_radius_override = Some(corner_radius);
        self
    }

    pub fn render_header(self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let header_background = appearance.theme().surface_2();

        let mut header_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        let mut left_content_container = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::Start)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        let text_color = blended_colors::text_main(appearance.theme(), header_background);
        let title_element = Text::new_inline(
            self.title,
            self.font_family,
            appearance.monospace_font_size(),
        )
        .soft_wrap(false)
        .with_selectable(false)
        .with_color(text_color)
        .finish();

        left_content_container.add_child(
            Expanded::new(
                1.,
                Container::new(title_element).with_margin_right(8.).finish(),
            )
            .finish(),
        );
        header_row.add_child(Shrinkable::new(1., left_content_container.finish()).finish());

        Container::new(header_row.finish())
            .with_padding_left(INLINE_ACTION_HORIZONTAL_PADDING)
            .with_padding_right(INLINE_ACTION_HORIZONTAL_PADDING)
            .with_vertical_padding(INLINE_ACTION_HEADER_VERTICAL_PADDING)
            .with_background(header_background)
            .with_corner_radius(
                self.corner_radius_override
                    .unwrap_or_else(|| CornerRadius::with_all(Radius::Pixels(8.))),
            )
            .finish()
    }
}
