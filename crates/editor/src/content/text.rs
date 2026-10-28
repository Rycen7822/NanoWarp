use std::fmt::Display;
use std::ops::{Add, AddAssign, BitXor, BitXorAssign, Range, Sub, SubAssign};

use arrayvec::ArrayString;
use enum_iterator::Sequence;
use markdown_parser::weight::CustomWeight;
use markdown_parser::{FormattedTextStyles, Hyperlink};
use pathfinder_color::ColorU;
use rand::Rng;
use serde::{Deserialize, Serialize};
use string_offset::{ByteOffset, CharOffset, impl_offset};
use sum_tree::{Cursor, SeekBias, SumTree};
use warpui_core::fonts::{Properties, Style, Weight};
use warpui_core::text::point::Point;

use super::buffer::Buffer;

#[cfg(test)]
pub const TEXT_FRAGMENT_SIZE: usize = 64;
#[cfg(not(test))]
pub const TEXT_FRAGMENT_SIZE: usize = 128;

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;

/// A summary of text locations.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextSummary {
    pub chars: CharOffset,
    pub bytes: ByteOffset,
    pub lines: Point,
    pub first_line_len: u32,
    pub rightmost_point: Point,
}

impl<'a> std::ops::AddAssign<&'a Self> for TextSummary {
    fn add_assign(&mut self, other: &'a Self) {
        let joined_line_len = self.lines.column + other.first_line_len;
        if joined_line_len > self.rightmost_point.column {
            self.rightmost_point = Point::new(self.lines.row, joined_line_len);
        }
        if other.rightmost_point.column > self.rightmost_point.column {
            self.rightmost_point = self.lines + other.rightmost_point;
        }

        if self.lines.row == 0 {
            self.first_line_len += other.first_line_len;
        }

        self.chars += other.chars;
        self.bytes += other.bytes;
        self.lines += other.lines;
    }
}

impl std::ops::AddAssign<Self> for TextSummary {
    fn add_assign(&mut self, other: Self) {
        *self += &other;
    }
}

impl sum_tree::Dimension<'_, TextSummary> for TextSummary {
    fn add_summary(&mut self, summary: &TextSummary) {
        *self += summary;
    }
}

impl sum_tree::Dimension<'_, TextSummary> for Point {
    fn add_summary(&mut self, summary: &TextSummary) {
        *self += summary.lines;
    }
}

impl sum_tree::Dimension<'_, TextSummary> for ByteOffset {
    fn add_summary(&mut self, summary: &TextSummary) {
        *self += summary.bytes
    }
}

impl sum_tree::Dimension<'_, TextSummary> for CharOffset {
    fn add_summary(&mut self, summary: &TextSummary) {
        *self += summary.chars;
    }
}

pub struct Bytes<'a> {
    cursor: Cursor<'a, BufferText, ByteOffset, ByteOffset>,
    start: ByteOffset,
    end: ByteOffset,
}

impl<'a> Bytes<'a> {
    pub fn new(buffer: &'a Buffer, start: ByteOffset, end: ByteOffset) -> Self {
        Self::from_sum_tree(&buffer.content, start, end)
    }

    pub fn from_sum_tree(
        content: &'a SumTree<BufferText>,
        start: ByteOffset,
        end: ByteOffset,
    ) -> Self {
        let mut cursor = content.cursor();
        cursor.seek(&start, SeekBias::Right);
        Self { cursor, start, end }
    }

    /// Re-seek the iterator to a new starting position, keeping the same end bound.
    pub fn seek(&mut self, offset: ByteOffset) {
        self.start = offset;
        self.cursor.seek(&offset, SeekBias::Right);
    }
}

impl<'a> Iterator for Bytes<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(item) = self.cursor.item() {
            let start = *self.cursor.start();
            let end = self.cursor.end();

            if start >= self.end {
                break;
            }

            self.cursor.next();
            match item {
                BufferText::BlockMarker { .. } | BufferText::Newline => return Some(b"\n"),
                BufferText::Text { fragment, .. } => {
                    let mut slice_start = 0;
                    let mut slice_end = fragment.len();
                    if self.start > start {
                        slice_start = (self.start - start).as_usize();
                    }

                    if self.end < end {
                        slice_end = slice_end.saturating_sub((end - self.end).as_usize());
                    }
                    return fragment.as_bytes().get(slice_start..slice_end);
                }
                _ => (),
            };
        }

        None
    }
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub enum LinkMarker {
    Start(String),
    End,
}

impl LinkMarker {
    fn to_counter_delta(&self) -> i32 {
        match &self {
            LinkMarker::Start(_) => 1,
            LinkMarker::End => -1,
        }
    }
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub enum BufferText {
    /// A character.
    Text {
        fragment: ArrayString<TEXT_FRAGMENT_SIZE>,
        char_count: u8,
    },
    /// A style marker. The markers come in pairs. If there is a starting marker,
    /// it has to be closed by an ending marker.
    Marker {
        /// The style type the marker represents.
        marker_type: BufferTextStyle,
        /// Whether this is a start or an end marker.
        dir: MarkerDir,
    },
    Link(LinkMarker),
    /// A newline.
    Newline,
    /// Styling marker that decorates an entire paragraph.
    BlockMarker {
        marker_type: BufferBlockStyle,
    },
    /// Ghosted text, such as an autosuggestion or zero-state placeholder for a newly-inserted block.
    Placeholder {
        content: String,
    },
}

impl Display for BufferText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text { fragment, .. } => write!(f, "{fragment}"),
            Self::Newline => f.write_str("\\n"),
            Self::Marker { marker_type, dir } => {
                let start = match marker_type {
                    BufferTextStyle::Weight(_) => "b",
                    BufferTextStyle::Italic => "i",
                    BufferTextStyle::Underline => "u",
                    BufferTextStyle::InlineCode => "c",
                    BufferTextStyle::StrikeThrough => "s",
                };

                let end = match dir {
                    MarkerDir::Start => "s",
                    MarkerDir::End => "e",
                };

                write!(f, "<{start}_{end}>")
            }
            Self::Link(marker) => {
                let name = match marker {
                    LinkMarker::Start(url) => format!("a_{url}"),
                    LinkMarker::End => "a".to_string(),
                };

                write!(f, "<{name}>")
            }
            Self::BlockMarker { marker_type } => {
                f.write_str("<")?;
                let _ = marker_type;
                f.write_str("text")?;
                f.write_str(">")
            }
            Self::Placeholder { content } => write!(f, "<placeholder_s>{content}<placeholder_e>"),
        }
    }
}

/// Identify whether it is a starting or ending marker.
#[derive(Eq, PartialEq, Clone, Debug, Copy)]
pub enum MarkerDir {
    Start,
    End,
}

impl MarkerDir {
    fn to_counter_delta(self) -> i32 {
        match self {
            MarkerDir::Start => 1,
            MarkerDir::End => -1,
        }
    }
}

#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub enum BufferBlockStyle {
    PlainText,
}

/// The unit that should be applied on each indent.
#[derive(Eq, PartialEq, Clone, Copy, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum IndentUnit {
    Tab,
    Space(usize),
}

impl Default for IndentUnit {
    fn default() -> Self {
        Self::Space(4)
    }
}

impl IndentUnit {
    pub fn width(&self) -> usize {
        match self {
            Self::Tab => 1,
            Self::Space(num) => *num,
        }
    }

    pub fn char_unit(&self) -> String {
        match self {
            Self::Tab => "\t".to_string(),
            Self::Space(_) => " ".to_string(),
        }
    }

    pub fn text_with_num_tab_stops(&self, count: usize) -> String {
        match self {
            Self::Tab => "\t".repeat(count),
            Self::Space(num) => " ".repeat(*num * count),
        }
    }
}

/// This describes behavior when indenting or unindenting a line.
#[derive(Eq, PartialEq)]
pub enum IndentBehavior {
    /// Insert or remove one tab's worth of spaces at the start of the line.
    TabIndent(IndentUnit),
    /// Do nothing - this block cannot be (un)indented.
    Ignore,
}

/// Supported text styles in the buffer.
#[derive(Eq, PartialEq, Clone, Copy, Debug, Hash, Sequence)]
pub enum BufferTextStyle {
    Weight(CustomWeight),
    Italic,
    Underline,
    InlineCode,
    StrikeThrough,
}

impl BufferTextStyle {
    pub fn bold() -> Self {
        Self::Weight(CustomWeight::Bold)
    }

    pub fn custom_weight(&self) -> Option<Weight> {
        match self {
            Self::Weight(weight) => Some(Weight::from_custom_weight(Some(*weight))),
            _ => None,
        }
    }

    /// Returns true if the style is [`BufferTextStyle::Weight`] and the weight is non-Normal.
    pub fn has_custom_weight(&self) -> bool {
        matches!(self, Self::Weight(_))
    }

    pub fn random<R: Rng>(rng: &mut R) -> Self {
        let r = rng.gen_range(0..5);
        match r {
            0 => Self::Weight(CustomWeight::Bold),
            1 => Self::Italic,
            2 => Self::Underline,
            3 => Self::InlineCode,
            4 => Self::StrikeThrough,
            _ => unreachable!(),
        }
    }
}

#[derive(Eq, PartialEq, Debug, Default, Clone)]
pub struct TextStylesWithMetadata {
    weight: Option<CustomWeight>,
    italic: bool,
    underline: bool,
    inline_code: bool,
    placeholder: bool,
    strikethrough: bool,
    link: Option<String>,
    color: Option<ColorU>,
}

impl TextStylesWithMetadata {
    pub fn bold(mut self) -> Self {
        self.weight = Some(CustomWeight::Bold);
        self
    }

    pub fn set_weight(&mut self, weight: Option<Weight>) {
        self.weight = weight.and_then(|w| w.to_custom_weight());
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn inline_code(mut self) -> Self {
        self.inline_code = true;
        self
    }

    pub fn link(mut self, link: String) -> Self {
        self.link = Some(link);
        self
    }

    pub fn with_color(mut self, color: ColorU) -> Self {
        self.color = Some(color);
        self
    }

    pub fn color_mut(&mut self) -> &mut Option<ColorU> {
        &mut self.color
    }

    pub fn inline_code_mut(&mut self) -> &mut bool {
        &mut self.inline_code
    }

    pub fn style_mut(&mut self, style: &BufferTextStyle) -> Option<&mut bool> {
        match style {
            BufferTextStyle::Italic => Some(&mut self.italic),
            BufferTextStyle::Underline => Some(&mut self.underline),
            BufferTextStyle::InlineCode => Some(&mut self.inline_code),
            BufferTextStyle::StrikeThrough => Some(&mut self.strikethrough),
            BufferTextStyle::Weight(_) => None,
        }
    }

    pub fn colliding_style(&self, style: &BufferTextStyle) -> bool {
        match style {
            BufferTextStyle::Italic => self.italic,
            BufferTextStyle::Underline => self.underline,
            BufferTextStyle::InlineCode => self.inline_code,
            BufferTextStyle::StrikeThrough => self.strikethrough,
            BufferTextStyle::Weight(_) => self.weight.is_some(),
        }
    }

    pub fn exact_match_style(&self, style: &BufferTextStyle) -> bool {
        match style {
            BufferTextStyle::Italic => self.italic,
            BufferTextStyle::Underline => self.underline,
            BufferTextStyle::InlineCode => self.inline_code,
            BufferTextStyle::StrikeThrough => self.strikethrough,
            BufferTextStyle::Weight(weight) => self.weight == Some(*weight),
        }
    }

    /// Applies the [`Properties`] encompassed by this style.
    pub fn apply_properties(&self, mut properties: Properties) -> Properties {
        if self.italic {
            properties = properties.style(Style::Italic);
        }
        // To ensure we respect the weight set by the block type,
        // we only set the weight if its explicitly set, rather than
        // allowing `None` to override the weight set by the block type.
        if let Some(custom_weight) = self.weight {
            properties = properties.weight(Weight::from_custom_weight(Some(custom_weight)));
        }

        properties
    }

    pub fn link_mut(&mut self) -> &mut Option<String> {
        &mut self.link
    }

    pub fn link_content(&self) -> Option<String> {
        self.link.clone()
    }

    /// Returns true if Weight is [`Weight::Normal`].
    pub fn is_normal_weight(&self) -> bool {
        self.weight.is_none()
    }

    /// Whether or not the weight is at least [`Weight::Bold`].
    pub fn is_at_least_bold(&self) -> bool {
        self.weight.is_some_and(|w| w.is_at_least_bold())
    }

    pub fn is_inline_code(&self) -> bool {
        self.inline_code
    }

    pub fn is_italic(&self) -> bool {
        self.italic
    }

    pub fn is_underlined(&self) -> bool {
        self.underline
    }

    pub fn is_link(&self) -> bool {
        self.link.is_some()
    }

    pub fn is_strikethrough(&self) -> bool {
        self.strikethrough
    }

    pub fn is_placeholder(&self) -> bool {
        self.placeholder
    }

    pub fn color(&self) -> Option<ColorU> {
        self.color
    }

    pub fn from_text_styles(
        text_styles: TextStyles,
        link: Option<String>,
        color: Option<ColorU>,
    ) -> Self {
        Self {
            weight: text_styles.weight,
            italic: text_styles.italic,
            underline: text_styles.underline,
            placeholder: text_styles.placeholder,
            inline_code: text_styles.inline_code,
            strikethrough: text_styles.strikethrough,
            link,
            color,
        }
    }

    /// Inherited text style behavior after backspacing.
    pub fn text_style_override_after_deletion(self, active_style: Self) -> Self {
        Self {
            weight: self.weight,
            italic: self.italic,
            inline_code: active_style.inline_code,
            ..Default::default()
        }
    }

    /// Mark these as styles for placeholder text. Currently, in [`super::buffer::StyledBufferRun`],
    /// we report placeholders as inline styles.
    pub fn for_placeholder(mut self) -> Self {
        self.placeholder = true;
        self
    }

    /// Given two text styles, return the styles that are active for both.
    pub fn mutual_styles(self, other: Self) -> Self {
        let link = match (self.link, other.link) {
            (Some(link1), Some(link2)) if link1 == link2 => Some(link1),
            _ => None,
        };

        let color = match (self.color, other.color) {
            (Some(color1), Some(color2)) if color1 == color2 => Some(color1),
            _ => None,
        };

        // If the weights are different, use the default Normal weight.
        let weight = if self.weight.eq(&other.weight) {
            self.weight
        } else {
            Default::default()
        };

        Self {
            weight,
            italic: self.italic && other.italic,
            underline: self.underline && other.underline,
            inline_code: self.inline_code && other.inline_code,
            strikethrough: self.strikethrough && other.strikethrough,
            link,
            color,
            placeholder: self.placeholder && other.placeholder,
        }
    }
}

impl From<FormattedTextStyles> for TextStylesWithMetadata {
    fn from(styles: FormattedTextStyles) -> Self {
        TextStylesWithMetadata {
            weight: styles.weight,
            italic: styles.italic,
            underline: styles.underline,
            inline_code: styles.inline_code,
            strikethrough: styles.strikethrough,
            placeholder: false,
            link: styles.hyperlink.and_then(Hyperlink::url),
            color: None, // TODO: Update this when adding strikethrough support.
        }
    }
}

impl From<TextStylesWithMetadata> for FormattedTextStyles {
    fn from(styles: TextStylesWithMetadata) -> Self {
        FormattedTextStyles {
            weight: styles.weight,
            italic: styles.italic,
            underline: styles.underline,
            strikethrough: styles.strikethrough,
            hyperlink: styles.link.map(Hyperlink::Url),
            inline_code: styles.inline_code,
        }
    }
}

#[derive(Eq, PartialEq, Debug, Default, Clone, Copy)]
pub struct TextStyles {
    weight: Option<CustomWeight>,
    italic: bool,
    underline: bool,
    inline_code: bool,
    strikethrough: bool,
    /// Whether or not this is placeholder text. Placeholders are sort of a pseudo-style - they're
    /// represented as their own kind of buffer content, but we render them as inline styled text.
    placeholder: bool,
    link: bool,
    colored: bool,
}

impl TextStyles {
    pub fn all() -> Self {
        Self {
            weight: Some(CustomWeight::Bold),
            italic: true,
            underline: true,
            inline_code: true,
            strikethrough: true,
            ..Default::default()
        }
    }

    pub fn bold(mut self) -> Self {
        self.weight = Some(CustomWeight::Bold);
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    pub fn inline_code(mut self) -> Self {
        self.inline_code = true;
        self
    }

    pub fn strikethrough(mut self) -> Self {
        self.strikethrough = true;
        self
    }

    pub fn set_weight(&mut self, weight: Weight) {
        self.weight = weight.to_custom_weight();
    }

    pub fn style_mut(&mut self, style: &BufferTextStyle) -> Option<&mut bool> {
        match style {
            BufferTextStyle::Italic => Some(&mut self.italic),
            BufferTextStyle::Underline => Some(&mut self.underline),
            BufferTextStyle::InlineCode => Some(&mut self.inline_code),
            BufferTextStyle::StrikeThrough => Some(&mut self.strikethrough),
            BufferTextStyle::Weight(_) => None,
        }
    }

    pub fn colliding_style(&self, style: &BufferTextStyle) -> bool {
        match style {
            BufferTextStyle::Italic => self.italic,
            BufferTextStyle::Underline => self.underline,
            BufferTextStyle::InlineCode => self.inline_code,
            BufferTextStyle::StrikeThrough => self.strikethrough,
            BufferTextStyle::Weight(_) => self.weight.is_some(),
        }
    }

    pub fn exact_match_style(&self, style: &BufferTextStyle) -> bool {
        match style {
            BufferTextStyle::Italic => self.italic,
            BufferTextStyle::Underline => self.underline,
            BufferTextStyle::InlineCode => self.inline_code,
            BufferTextStyle::StrikeThrough => self.strikethrough,
            BufferTextStyle::Weight(weight) => Some(*weight) == self.weight,
        }
    }

    pub fn is_inline_code(&self) -> bool {
        self.inline_code
    }

    pub fn is_strikethrough(&self) -> bool {
        self.strikethrough
    }

    pub fn get_weight(&self) -> Weight {
        Weight::from_custom_weight(self.weight)
    }

    pub fn get_custom_weight(&self) -> Option<CustomWeight> {
        self.weight
    }

    pub fn is_italic(&self) -> bool {
        self.italic
    }

    pub fn is_underlined(&self) -> bool {
        self.underline
    }

    pub fn is_link(&self) -> bool {
        self.link
    }

    pub fn is_placeholder(&self) -> bool {
        self.placeholder
    }

    pub fn is_colored(&self) -> bool {
        self.colored
    }

    /// Filters to only inheritable text styles. Keep in sync with [`BufferTextStyle::is_inheritable`].
    pub fn inheritable(self) -> Self {
        Self {
            weight: self.weight,
            italic: self.italic,
            underline: self.underline,
            inline_code: self.inline_code,
            strikethrough: self.strikethrough,
            placeholder: false,
            link: false,
            colored: false,
        }
    }
}

impl From<TextStylesWithMetadata> for TextStyles {
    fn from(styles: TextStylesWithMetadata) -> TextStyles {
        Self {
            weight: styles.weight,
            italic: styles.italic,
            underline: styles.underline,
            placeholder: styles.placeholder,
            inline_code: styles.inline_code,
            strikethrough: styles.strikethrough,
            link: styles.link.is_some(),
            colored: styles.color.is_some(),
        }
    }
}

impl BitXor for TextStyles {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        let weight = if self.weight.is_some() == rhs.weight.is_some() {
            None
        } else if self.weight.is_some() {
            self.weight
        } else {
            rhs.weight
        };
        Self {
            weight,
            italic: self.italic ^ rhs.italic,
            underline: self.underline ^ rhs.underline,
            placeholder: self.placeholder ^ rhs.placeholder,
            inline_code: self.inline_code ^ rhs.inline_code,
            strikethrough: self.strikethrough ^ rhs.strikethrough,
            link: self.link ^ rhs.link,
            colored: self.colored ^ rhs.colored,
        }
    }
}

impl BitXorAssign for TextStyles {
    fn bitxor_assign(&mut self, rhs: Self) {
        let weight = if self.weight.is_some() == rhs.weight.is_some() {
            None
        } else if self.weight.is_some() {
            self.weight
        } else {
            rhs.weight
        };
        self.weight = weight;
        self.italic ^= rhs.italic;
        self.underline ^= rhs.underline;
        self.placeholder ^= rhs.placeholder;
        self.inline_code ^= rhs.inline_code;
        self.strikethrough ^= rhs.strikethrough;
        self.link ^= rhs.link;
        self.colored ^= rhs.colored;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LineCount(usize);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockCount(usize);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LinkCount(pub usize);

impl_offset!(LineCount);
impl_offset!(BlockCount);
impl_offset!(LinkCount);

#[derive(Eq, PartialEq, Clone, Copy, Debug, Default)]
pub struct BlockSummary {
    pub block: BlockCount,
}

impl AddAssign<&BlockSummary> for BlockSummary {
    fn add_assign(&mut self, other: &Self) {
        self.block += other.block;
    }
}

/// Summary of the style decorating the current cursor position. This should match the style enums in BufferTextStyle.
///
/// How it works: A range of text is decorated by a style if it is enclosed by a pair of style markers.
/// The starting marker increments the counter by 1 whereas a closing marker decrements it by 1.
/// If the current cursor position has a non-zero counter value, it means this position is decorated by the style.
#[derive(Eq, PartialEq, Clone, Copy, Debug, Default)]
pub struct StyleSummary {
    weight_counter: i32,
    // Today, we do not support nested weights. If we do, this should be a stack.
    weight: Option<CustomWeight>,
    italic_counter: i32,
    underline_counter: i32,
    inline_code_counter: i32,
    strikethrough_counter: i32,
    link_counter: i32,
    /// We need to keep track the total link marker count so we could index into a specific link marker
    /// to retrieve the url metadata.
    total_link_marker: i32,
}

impl AddAssign<&StyleSummary> for StyleSummary {
    fn add_assign(&mut self, other: &Self) {
        self.weight = CustomWeight::merge_weights(self.weight, other.weight);
        self.weight_counter += other.weight_counter;
        self.italic_counter += other.italic_counter;
        self.link_counter += other.link_counter;
        self.inline_code_counter += other.inline_code_counter;
        self.total_link_marker += other.total_link_marker;
        self.strikethrough_counter += other.strikethrough_counter;
        self.underline_counter += other.underline_counter;
    }
}

impl StyleSummary {
    pub(super) fn style_counter(&self, style: &BufferTextStyle) -> i32 {
        match style {
            BufferTextStyle::Weight(_) => self.weight_counter,
            BufferTextStyle::Italic => self.italic_counter,
            BufferTextStyle::Underline => self.underline_counter,
            BufferTextStyle::InlineCode => self.inline_code_counter,
            BufferTextStyle::StrikeThrough => self.strikethrough_counter,
        }
    }

    pub(super) fn total_link_counter(&self) -> i32 {
        self.total_link_marker
    }

    fn set_weight(&mut self, weight: Option<CustomWeight>) {
        self.weight = weight;
    }

    fn style_counter_mut(&mut self, style: &BufferTextStyle) -> &mut i32 {
        match style {
            BufferTextStyle::Weight(_) => &mut self.weight_counter,
            BufferTextStyle::Italic => &mut self.italic_counter,
            BufferTextStyle::Underline => &mut self.underline_counter,
            BufferTextStyle::InlineCode => &mut self.inline_code_counter,
            BufferTextStyle::StrikeThrough => &mut self.strikethrough_counter,
        }
    }

    /// The text-level styling in this summary.
    pub fn text_styles(&self) -> TextStyles {
        let weight = if self.weight_counter > 0 {
            self.weight
        } else {
            None
        };
        TextStyles {
            weight,
            italic: self.italic_counter > 0,
            underline: self.underline_counter > 0,
            link: self.link_counter > 0,
            inline_code: self.inline_code_counter > 0,
            strikethrough: self.strikethrough_counter > 0,
            placeholder: false,
            colored: false,
        }
    }
}

impl From<TextStyles> for StyleSummary {
    fn from(styles: TextStyles) -> StyleSummary {
        Self {
            weight: styles.weight,
            weight_counter: styles.weight.is_some().into(),
            italic_counter: styles.italic.into(),
            underline_counter: styles.underline.into(),
            link_counter: styles.link.into(),
            inline_code_counter: styles.inline_code.into(),
            strikethrough_counter: styles.strikethrough.into(),
            total_link_marker: 0,
        }
    }
}

impl From<StyleSummary> for TextStyles {
    fn from(summary: StyleSummary) -> TextStyles {
        summary.text_styles()
    }
}

/// Summary of a fragment of buffer. It contains both the text and style metadata.
///
/// Style data is stored behind an `Option<Box<>>` so that plain-text buffers
/// (code editors) pay zero cost for style tracking — the box is only allocated
/// when the item actually carries style information (Marker, Link, Color).
#[derive(Eq, PartialEq, Clone, Debug, Default)]
pub struct BufferSummary {
    pub style: Option<Box<StyleSummary>>,
    pub text: TextSummary,
    pub block: BlockSummary,
}

impl BufferSummary {
    /// Returns the style summary, or a default (all-zero) summary if no style
    /// data is present. This is the common accessor for code that needs to
    /// read style counters.
    pub fn style_summary(&self) -> StyleSummary {
        self.style.as_deref().copied().unwrap_or_default()
    }
}

impl AddAssign<&BufferSummary> for BufferSummary {
    fn add_assign(&mut self, other: &Self) {
        match (&mut self.style, &other.style) {
            (_, None) => {} // nothing to add
            (self_style @ None, Some(other_style)) => {
                *self_style = Some(other_style.clone());
            }
            (Some(self_style), Some(other_style)) => {
                *self_style.as_mut() += other_style.as_ref();
            }
        }
        self.text += &other.text;
        self.block += &other.block;
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for BufferSummary {
    fn add_summary(&mut self, summary: &BufferSummary) {
        *self += summary;
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for TextSummary {
    fn add_summary(&mut self, summary: &BufferSummary) {
        *self += &summary.text;
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for StyleSummary {
    fn add_summary(&mut self, summary: &BufferSummary) {
        if let Some(style) = &summary.style {
            *self += style.as_ref();
        }
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for CharOffset {
    fn add_summary(&mut self, summary: &BufferSummary) {
        *self += summary.text.chars;
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for LineCount {
    fn add_summary(&mut self, summary: &BufferSummary) {
        *self += summary.text.lines.row as usize;
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for BlockCount {
    fn add_summary(&mut self, summary: &BufferSummary) {
        *self += summary.block.block;
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for LinkCount {
    fn add_summary(&mut self, summary: &BufferSummary) {
        if let Some(style) = &summary.style {
            *self += style.total_link_marker as usize;
        }
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for ByteOffset {
    fn add_summary(&mut self, summary: &BufferSummary) {
        *self += summary.text.bytes;
    }
}

impl sum_tree::Dimension<'_, BufferSummary> for Point {
    fn add_summary(&mut self, summary: &BufferSummary) {
        *self += summary.text.lines;
    }
}

impl sum_tree::Item for BufferText {
    type Summary = BufferSummary;

    fn summary(&self) -> Self::Summary {
        let text_summary = match &self {
            BufferText::Newline => TextSummary {
                chars: 1.into(),
                bytes: 1.into(),
                lines: Point::new(1, 0),
                first_line_len: 0,
                rightmost_point: Point::new(0, 0),
            },
            BufferText::Text {
                fragment,
                char_count,
            } => TextSummary {
                chars: (*char_count as usize).into(),
                bytes: fragment.len().into(),
                lines: Point::new(0, (*char_count).into()),
                first_line_len: (*char_count).into(),
                rightmost_point: Point::new(0, (*char_count).into()),
            },
            BufferText::Marker { .. } | BufferText::Link(_) => TextSummary {
                chars: 0.into(),
                bytes: 0.into(),
                lines: Point::new(0, 0),
                first_line_len: 0,
                rightmost_point: Point::new(0, 0),
            },
            BufferText::BlockMarker { .. } => TextSummary {
                chars: 1.into(),
                bytes: 1.into(),
                lines: Point::new(1, 0),
                first_line_len: 0,
                rightmost_point: Point::new(0, 0),
            },
            // Placeholders count as single special characters. This lets us distinguish between
            // the cursor being at a placeholder and at the character just after it.
            BufferText::Placeholder { .. } => TextSummary {
                chars: 1.into(),
                bytes: 1.into(),
                lines: Point::new(0, 1),
                first_line_len: 1,
                rightmost_point: Point::new(0, 1),
            },
        };

        // Only allocate a StyleSummary when the item actually carries style
        // data. For plain-text buffers (code editors) this is never the case,
        // so style is always None — zero heap allocations.
        let style_summary = match self {
            BufferText::Marker { marker_type, dir } => {
                let mut s = StyleSummary::default();
                let delta = dir.to_counter_delta();
                if let BufferTextStyle::Weight(weight) = marker_type {
                    s.set_weight(Some(*weight));
                }
                *s.style_counter_mut(marker_type) += delta;
                Some(Box::new(s))
            }
            BufferText::Link(marker) => {
                let mut s = StyleSummary::default();
                let delta = marker.to_counter_delta();
                s.link_counter += delta;
                s.total_link_marker += 1;
                Some(Box::new(s))
            }
            _ => None,
        };

        let block_summary = match &self {
            BufferText::BlockMarker { .. } => BlockSummary { block: 1.into() },
            _ => Default::default(),
        };

        BufferSummary {
            style: style_summary,
            text: text_summary,
            block: block_summary,
        }
    }
}
