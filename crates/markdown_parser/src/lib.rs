use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::fmt::Debug;
use std::ops::Range;
use std::sync::Arc;

pub mod markdown_parser;
pub mod weight;
use itertools::Itertools;
pub use markdown_parser::{parse_image_prefix, parse_image_run_line, parse_markdown};
use serde_yaml::Mapping;
use weight::CustomWeight;

/// Trait for an "action" that can be dispatched via a hyperlink click handler.
/// This purposefully shadows the `Action` trait from `warpui`.
///
/// Since `warpui` depends on this crate, we can't depend on the `warpui_core::Action` trait directly.
/// Instead, we create a new trait with a blanket implementation that implicitly results
/// in any `warpui_core::Action` implementing this `Action`.
pub trait Action: Any + Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T> Action for T
where
    T: Any + Debug + Send + Sync,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub trait LineCount {
    fn num_lines(&self) -> usize;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedText {
    pub lines: VecDeque<FormattedTextLine>,
}

impl FormattedText {
    pub fn new(lines: impl Into<VecDeque<FormattedTextLine>>) -> Self {
        Self {
            lines: lines.into(),
        }
    }

    /// Creates a new FormattedText where the first and last line breaks are removed, if any.
    pub fn new_trimmed(lines: impl Into<VecDeque<FormattedTextLine>>) -> Self {
        let mut new = Self::new(lines);
        new.trim();
        new
    }

    fn trim(&mut self) {
        // Since we exhaust contiguous new lines into a single line break,
        // there won't be multiple contiguous line breaks; there's at most one to remove.
        if let Some(FormattedTextLine::LineBreak) = self.lines.front() {
            self.lines.pop_front();
        }

        // Similarly for the end.
        if let Some(FormattedTextLine::LineBreak) = self.lines.back() {
            self.lines.pop_back();
        }
    }

    /// Returns the raw text of the markdown, without any of the markdown
    /// markers.
    pub fn raw_text(&self) -> String {
        self.lines.iter().map(|line| line.raw_text()).join("")
    }

    pub fn append_line(mut self, line: FormattedTextLine) -> Self {
        self.lines.push_back(line);
        self
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum FormattedTextLine {
    Heading(FormattedTextHeader),
    Line(FormattedTextInline),
    OrderedList(OrderedFormattedIndentTextInline),
    UnorderedList(FormattedIndentTextInline),
    CodeBlock(CodeBlockText),
    TaskList(FormattedTaskList),
    LineBreak,
    HorizontalRule,
    Embedded(Mapping),
    Image(FormattedImage),
}

impl FormattedTextLine {
    pub fn raw_text(&self) -> String {
        let mut text = match self {
            Self::CodeBlock(text) => text.code.clone(),
            Self::Heading(header) => header
                .text
                .iter()
                .map(|fragment| fragment.raw_text())
                .join(""),
            Self::Line(line) => line.iter().map(|fragment| fragment.raw_text()).join(""),
            Self::TaskList(line) => line
                .text
                .iter()
                .map(|fragment| fragment.raw_text())
                .join(""),
            Self::OrderedList(list) => list
                .indented_text
                .text
                .iter()
                .map(|fragment| fragment.raw_text())
                .join(""),
            Self::UnorderedList(list) => list
                .text
                .iter()
                .map(|fragment| fragment.raw_text())
                .join(""),
            Self::LineBreak | Self::HorizontalRule | Self::Embedded(_) => "\n".to_string(),
            Self::Image(image) => format!("{}\n", image.alt_text),
        };
        // Each `FormattedTextLine` unit represents a complete line. If it doesn't already end in
        // a newline, add one.
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text
    }

    pub fn set_weight(&mut self, weight: Option<CustomWeight>) -> &Self {
        match self {
            Self::Heading(header) => {
                for fragment in &mut header.text {
                    fragment.styles.weight = weight;
                }
            }
            Self::Line(line) => {
                for fragment in line {
                    fragment.styles.weight = weight;
                }
            }
            Self::OrderedList(list) => {
                for fragment in &mut list.indented_text.text {
                    fragment.styles.weight = weight;
                }
            }
            Self::UnorderedList(list) => {
                for fragment in &mut list.text {
                    fragment.styles.weight = weight;
                }
            }
            Self::TaskList(list) => {
                for fragment in &mut list.text {
                    fragment.styles.weight = weight;
                }
            }
            Self::CodeBlock(_)
            | Self::LineBreak
            | Self::HorizontalRule
            | Self::Embedded(_)
            | Self::Image(_) => {}
        }
        self
    }

    fn inline_fragments(&self) -> Option<&FormattedTextInline> {
        match &self {
            FormattedTextLine::Heading(header) => Some(&header.text),
            FormattedTextLine::Line(texts) => Some(texts),
            FormattedTextLine::OrderedList(texts) => Some(&texts.indented_text.text),
            FormattedTextLine::UnorderedList(texts) => Some(&texts.text),
            FormattedTextLine::TaskList(list) => Some(&list.text),
            FormattedTextLine::CodeBlock(_)
            | FormattedTextLine::LineBreak
            | FormattedTextLine::HorizontalRule
            | FormattedTextLine::Embedded(_)
            | FormattedTextLine::Image(_) => None,
        }
    }

    pub fn hyperlinks(&self, skip_raw_links: bool) -> Vec<(Range<usize>, Hyperlink)> {
        let mut hyperlinks: Vec<(Range<usize>, Hyperlink)> = Vec::new();
        if let Some(inline_fragments) = self.inline_fragments() {
            let mut char_count = 0;
            for fragment in inline_fragments {
                let range_start = char_count;
                char_count += fragment.text.chars().count();
                if let Some(link) = &fragment.styles.hyperlink
                    && (!skip_raw_links
                        || !matches!(&link, Hyperlink::Url(url) if url == &fragment.text))
                {
                    hyperlinks.push((range_start..char_count, link.clone()));
                }
            }
        }
        hyperlinks
    }

    pub fn is_empty_line(&self) -> bool {
        matches!(self, Self::Line(line) if line.iter().all(|fragment| fragment.text.is_empty()))
    }
}

impl LineCount for FormattedTextLine {
    fn num_lines(&self) -> usize {
        match self {
            Self::CodeBlock(text) => text.code.matches('\n').count(),
            Self::Heading(_) => 1,
            Self::Line(_) => 1,
            Self::OrderedList(_) => 1,
            Self::UnorderedList(_) => 1,
            Self::TaskList(_) => 1,
            Self::LineBreak => 0,
            Self::HorizontalRule => 0,
            Self::Embedded(_) => 1,
            Self::Image(_) => 1,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedTextHeader {
    pub heading_size: usize,
    pub text: FormattedTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedTaskList {
    pub complete: bool,
    pub indent_level: usize,
    pub text: FormattedTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedIndentTextInline {
    pub indent_level: usize,
    pub text: FormattedTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CodeBlockText {
    pub lang: String,
    pub code: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct OrderedFormattedIndentTextInline {
    /// The number of this item, which may be `None` if it was unspecified or invalid in the source
    /// document.
    pub number: Option<usize>,
    pub indented_text: FormattedIndentTextInline,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FormattedImage {
    pub alt_text: String,
    pub source: String,
    /// Optional CommonMark image title, e.g. the `title` in `![alt](src "title")`.
    /// Empty titles are normalized to `None` by the parser.
    pub title: Option<String>,
}

pub type FormattedTextInline = Vec<FormattedTextFragment>;

/// A fragment of formatted text, containing the text itself and formatting flags/metadata.
#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub struct FormattedTextFragment {
    pub text: String,
    pub styles: FormattedTextStyles,
}

#[derive(Debug, Clone)]
pub enum Hyperlink {
    Url(String),
    Action(Arc<dyn Action>),
}

impl Hyperlink {
    /// Returns the URL if this is a URL, or `None` otherwise.
    pub fn url(self) -> Option<String> {
        match self {
            Hyperlink::Url(url) => Some(url),
            Hyperlink::Action(_) => None,
        }
    }
}

impl PartialEq for Hyperlink {
    // Stub implementation for [`Hyperlink`] that only compares URLs and not Actions.
    // This is an unfortunate byproduct of the fact that an [`Action`] does not implement [`PartialEq`]
    // but we require [`PartialEq`] to consolidate [`FormattedTextStyles`].
    // To get around this, we only compare URLs, which works for style consolidation since this is only
    // needed when generating formatted text from markdown, which provably does not support URLs that dispatch
    // actions
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Url(left), Self::Url(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for Hyperlink {}

/// Formatted text styling, with no attached content.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct FormattedTextStyles {
    pub weight: Option<CustomWeight>,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub inline_code: bool,
    pub hyperlink: Option<Hyperlink>,
}

impl FormattedTextFragment {
    pub fn plain_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: Default::default(),
        }
    }

    pub fn weighted(text: impl Into<String>, weight: Option<CustomWeight>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                weight,
                ..Default::default()
            },
        }
    }

    pub fn with_weight(&mut self, weight: Option<CustomWeight>) -> &Self {
        self.styles.weight = weight;
        self
    }

    pub fn bold(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                weight: Some(CustomWeight::Bold),
                ..Default::default()
            },
        }
    }

    pub fn italic(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                italic: true,
                ..Default::default()
            },
        }
    }

    pub fn bold_italic(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                weight: Some(CustomWeight::Bold),
                italic: true,
                ..Default::default()
            },
        }
    }

    pub fn hyperlink(tag: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            text: tag.into(),
            styles: FormattedTextStyles {
                hyperlink: Some(Hyperlink::Url(url.into())),
                ..Default::default()
            },
        }
    }

    /// Constructs a new hyperlink that dispatches an action when clicked.
    pub fn hyperlink_action<A: Action>(tag: impl Into<String>, action: A) -> Self {
        Self {
            text: tag.into(),
            styles: FormattedTextStyles {
                hyperlink: Some(Hyperlink::Action(Arc::new(action))),
                ..Default::default()
            },
        }
    }

    pub fn inline_code(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                inline_code: true,
                ..Default::default()
            },
        }
    }

    pub fn strikethrough(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                strikethrough: true,
                ..Default::default()
            },
        }
    }

    pub fn underline(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            styles: FormattedTextStyles {
                underline: true,
                ..Default::default()
            },
        }
    }

    pub fn raw_text(&self) -> &String {
        &self.text
    }
}

impl fmt::Debug for FormattedTextStyles {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // For readability, only show active styles.
        let mut first = true;

        if let Some(weight) = self.weight {
            if !first {
                f.write_str(" | ")?;
            }
            write!(f, "{weight:?}")?;
            first = false;
        }

        if self.italic {
            if !first {
                f.write_str(" | ")?;
            }
            f.write_str("Italic")?;
            first = false;
        }

        if self.strikethrough {
            if !first {
                f.write_str(" | ")?;
            }
            f.write_str("Strikethrough")?;
            first = false;
        }

        if self.inline_code {
            if !first {
                f.write_str(" | ")?;
            }
            f.write_str("InlineCode")?;
            first = false;
        }

        if let Some(link) = &self.hyperlink {
            if !first {
                f.write_str(" | ")?;
            }

            write!(f, "Hyperlink({link:?})")?;
            first = false;
        }

        if first {
            // No styles are active, so this is plain text.
            f.write_str("PlainText")?;
        }

        Ok(())
    }
}
