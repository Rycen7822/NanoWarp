use std::ops::Range;

use crate::platform;
use crate::platform::LineStyle;
use crate::text_layout::{
    ClipConfig, Line, StyleAndFont, TextAlignment, TextFrame, strip_leading_unicode_bom,
};

/// Struct to layout text, updating cached font fallback state as needed.
/// See [fonts::Cache::text_layout_system].
pub struct TextLayoutSystem<'a> {
    pub(super) platform: &'a dyn platform::TextLayoutSystem,
}

impl TextLayoutSystem<'_> {
    pub fn layout_line(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        clip_config: ClipConfig,
    ) -> Line {
        self.platform
            .layout_line(text, line_style, style_runs, max_width, clip_config)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn layout_text(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        max_height: f32,
        alignment: TextAlignment,
        first_line_head_indent: Option<f32>,
    ) -> TextFrame {
        self.platform.layout_text(
            text,
            line_style,
            style_runs,
            max_width,
            max_height,
            alignment,
            first_line_head_indent,
        )
    }

    /// Lays out a text frame without retaining it in a [`crate::text_layout::LayoutCache`].
    #[allow(clippy::too_many_arguments)]
    pub fn layout_text_uncached(
        &self,
        text: &str,
        line_style: LineStyle,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        max_height: f32,
        alignment: TextAlignment,
        first_line_head_indent: Option<f32>,
    ) -> TextFrame {
        let (text, adjusted_style_runs) = strip_leading_unicode_bom(text, style_runs);
        let style_runs = adjusted_style_runs
            .as_ref()
            .map_or(style_runs, Vec::as_slice);
        self.layout_text(
            text,
            line_style,
            style_runs,
            max_width,
            max_height,
            alignment,
            first_line_head_indent,
        )
    }
}
