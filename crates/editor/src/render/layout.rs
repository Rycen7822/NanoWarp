//! Shared text-layout utilities needed throughout the editor implementation.

use std::ops::Range;
use std::sync::Arc;

use warpui_core::AppContext;
use warpui_core::color::ColorU;
use warpui_core::fonts::TextLayoutSystem;
use warpui_core::text_layout::{
    ClipConfig, LayoutCache, Line, StyleAndFont, TextAlignment, TextBorder, TextFrame, TextStyle,
};
use warpui_core::units::{IntoPixels, Pixels};

use super::model::{BlockSpacing, ParagraphStyles, RenderState, RichTextStyles};
use crate::content::text::{BufferBlockStyle, TextStylesWithMetadata};

const HYPERLINK_UNDERLINE_COLOR: u32 = 0x7aa6daff;

/// Cap on the number of `char`s shaped into a single text frame.
///
/// The editor lays out at an effectively infinite width, so one logical line becomes one unwrapped
/// shaping call. Without a cap, a single multi-megabyte line allocates a text frame plus per-`char`
/// glyph and caret-position vectors bounded only by that line's length (APP-5392). Text past the
/// cap is dropped before shaping, which is safe because callers track buffer offsets from their own
/// content length rather than from the shaped frame, and consumers that map an offset or coordinate
/// into a frame already clamp to its end. The cap sits far past what can be read on screen, so a
/// cursor pinning at the truncation point is only reachable in lines nobody edits visually.
const MAX_LAYOUT_LINE_CHARS: usize = 2 * 1024 * 1024;

/// Shortens `text` to at most [`MAX_LAYOUT_LINE_CHARS`], returning it unchanged when it already
/// fits.
fn truncate_text_for_layout(text: &str) -> &str {
    // Every `char` occupies at least one byte, so a byte length within the cap puts the char count
    // within it too. This keeps the common case off the linear `char_indices` walk below.
    if text.len() <= MAX_LAYOUT_LINE_CHARS {
        return text;
    }

    // `nth` yields the byte offset at which the first `MAX_LAYOUT_LINE_CHARS` chars end, and `None`
    // when the text holds no more chars than that. Slicing there is always on a char boundary.
    match text.char_indices().nth(MAX_LAYOUT_LINE_CHARS) {
        Some((truncate_at, _)) => &text[..truncate_at],
        None => text,
    }
}

/// Drops style runs starting past [`MAX_LAYOUT_LINE_CHARS`] and clamps the tail of a run straddling
/// it, keeping char-indexed runs consistent with text shortened by [`truncate_text_for_layout`].
fn clamp_style_runs_for_layout(
    style_runs: &[(Range<usize>, StyleAndFont)],
) -> Vec<(Range<usize>, StyleAndFont)> {
    style_runs
        .iter()
        .filter(|(range, _)| range.start < MAX_LAYOUT_LINE_CHARS)
        .map(|(range, style)| (range.start..range.end.min(MAX_LAYOUT_LINE_CHARS), *style))
        .collect()
}

/// Utility for laying out rich text.
pub struct TextLayout<'a> {
    font_cache: TextLayoutSystem<'a>,
    rich_text_styles: &'a RichTextStyles,
    max_width: f32,
}

impl<'a> TextLayout<'a> {
    pub fn new(
        font_cache: TextLayoutSystem<'a>,
        rich_text_styles: &'a RichTextStyles,
        max_width: f32,
    ) -> Self {
        Self {
            font_cache,
            rich_text_styles,
            max_width,
        }
    }

    /// Builds a [`TextLayout`] for an editor render state.
    pub fn for_render_state(app: &'a AppContext, model: &'a RenderState) -> Self {
        Self::new(
            app.font_cache().text_layout_system(),
            model.styles(),
            model.viewport().width().as_f32(),
        )
    }

    /// Lay out a single frame of text. The caller is responsible for mapping rich text into
    /// the paragraph's styling and spacing, as well as the per-character styles.
    ///
    /// See [`Self::style_and_font`] for help constructing the `style_runs`.
    pub fn layout_text(
        &self,
        text: &str,
        paragraph_style: &ParagraphStyles,
        spacing: &BlockSpacing,
        style_runs: &[(Range<usize>, StyleAndFont)],
    ) -> Arc<TextFrame> {
        self.layout_text_with_options(
            text,
            paragraph_style,
            style_runs,
            self.content_width(spacing),
            Default::default(),
        )
    }

    pub fn layout_text_with_options(
        &self,
        text: &str,
        paragraph_style: &ParagraphStyles,
        style_runs: &[(Range<usize>, StyleAndFont)],
        max_width: f32,
        alignment: TextAlignment,
    ) -> Arc<TextFrame> {
        if text.is_empty() {
            return Arc::new(TextFrame::empty(
                paragraph_style.font_size,
                paragraph_style.line_height_ratio,
            ));
        }

        let shaped_text = truncate_text_for_layout(text);
        let clamped_style_runs = (shaped_text.len() < text.len()).then(|| {
            // Only pay for a new run list when the text actually lost characters.
            clamp_style_runs_for_layout(style_runs)
        });

        Arc::new(self.font_cache.layout_text_uncached(
            shaped_text,
            paragraph_style.line_style(),
            clamped_style_runs.as_deref().unwrap_or(style_runs),
            max_width,
            f32::MAX,
            alignment,
            None,
        ))
    }

    /// Lays out placeholder text for empty blocks.
    pub fn layout_placeholder(
        &self,
        layout_cache: &LayoutCache,
        text: &str,
        block_type: &BufferBlockStyle,
        spacing: &BlockSpacing,
    ) -> Arc<Line> {
        let paragraph_styles = self.paragraph_styles(block_type);
        let style_and_font = self.style_and_font(
            &paragraph_styles,
            &TextStylesWithMetadata::default().for_placeholder(),
        );
        let text = truncate_text_for_layout(text);
        let style_runs = &[(0..text.chars().count(), style_and_font)];
        layout_cache.layout_line(
            text,
            paragraph_styles.line_style(),
            style_runs,
            self.content_width(spacing),
            ClipConfig::end(),
            &self.font_cache,
        )
    }

    /// Returns the maximum width for text content laid out with the given spacing.
    fn content_width(&self, spacing: &BlockSpacing) -> f32 {
        self.max_width - spacing.x_axis_offset().as_f32()
    }

    /// The paragraph-level styling to use for blocks of the given kind.
    pub fn paragraph_styles(&self, block_type: &BufferBlockStyle) -> ParagraphStyles {
        self.rich_text_styles.paragraph_styles(block_type)
    }

    /// Given the [paragraph-level](ParagraphStyles) and [text-level](TextStyles) styles applicable
    /// to a range of text, build the [`StyleAndFont`] configuration for laying out that text.
    pub fn style_and_font(
        &self,
        paragraph_styles: &ParagraphStyles,
        text_styles: &TextStylesWithMetadata,
    ) -> StyleAndFont {
        let font_properties = text_styles.apply_properties(paragraph_styles.properties());
        let font_family = if text_styles.is_inline_code() {
            self.rich_text_styles.inline_code_style.font_family
        } else {
            paragraph_styles.font_family
        };

        let mut styling = TextStyle::default();
        if text_styles.is_placeholder() {
            styling = styling.with_foreground_color(self.rich_text_styles.placeholder_color);
        }

        if text_styles.is_strikethrough() {
            styling = styling.with_show_strikethrough(true);
        }

        if text_styles.is_underlined() {
            styling = styling.with_underline_color(self.rich_text_styles.base_text.text_color);
        }

        if text_styles.is_inline_code() {
            styling = styling
                .with_foreground_color(self.rich_text_styles.inline_code_style.font_color)
                .with_background_color(self.rich_text_styles.inline_code_style.background)
                .with_border(TextBorder {
                    color: self.rich_text_styles.inline_code_style.background,
                    radius: 4,
                    width: 1,
                    // Use the set 1.2 line height ratio for inline code backgrounds.
                    line_height_ratio_override: Some(120),
                });
        }

        if let Some(color) = text_styles.color() {
            styling = styling.with_syntax_color(color);
        }

        let style_and_font = StyleAndFont::new(font_family, font_properties, styling);

        if text_styles.is_link() {
            add_link_to_style_and_font(style_and_font)
        } else {
            style_and_font
        }
    }

    pub fn rich_text_styles(&self) -> &'a RichTextStyles {
        self.rich_text_styles
    }

    pub fn max_width(&self) -> Pixels {
        self.max_width.into_pixels()
    }
}

/// The line height for a line of text. In CSS terminology, this is the height of the
/// [line box](https://www.w3.org/TR/css-inline-3/#line-box). In typographic terms,
/// it should correspond to the distance between the [top](https://stackoverflow.com/questions/27631736/meaning-of-top-ascent-baseline-descent-bottom-and-leading-in-androids-font)
/// of one line and the top of the next.
///
/// Unlike [`Line::height`], this height does not depend on the specific text in the line. The
/// `height` field measures the line's actual bounds, so it depends on whether glyphs go below
/// the baseline or above the [cap line](https://www.canva.com/learn/typography-terms/).
pub(crate) fn line_height(line: &Line) -> f32 {
    line.font_size * line.line_height_ratio
}

pub(crate) fn add_link_to_style_and_font(mut style: StyleAndFont) -> StyleAndFont {
    let hyperlink_color = ColorU::from_u32(HYPERLINK_UNDERLINE_COLOR);
    style.style = style
        .style
        .with_underline_color(hyperlink_color)
        .with_foreground_color(hyperlink_color);
    style
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
