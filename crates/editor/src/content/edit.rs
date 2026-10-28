use std::collections::HashMap;
use std::mem;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use itertools::Itertools;
use num_traits::SaturatingSub;
use rangemap::RangeSet;
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use string_offset::{ByteOffset, CharOffset};
use urlocator::{UrlLocation, UrlLocator};
use vec1::Vec1;
use warp_core::ui::theme::Fill as ThemeFill;
use warp_errors::report_error;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::text::char_slice;
use warpui_core::text::point::Point;
use warpui_core::text_layout::StyleAndFont;
use warpui_core::units::IntoPixels;

use super::buffer::{StyledBufferBlock, StyledBufferRun, StyledTextBlock};
use super::text::{BufferBlockItem, BufferBlockStyle};
use crate::parallel_util::Last;
use crate::render::layout::{TextLayout, add_link_to_style_and_font};
use crate::render::model::{
    BlockItem, BlockLocation, Cursor, Decoration, FrameOffset, HiddenBlockConfig,
    HorizontalRuleConfig, ImageBlockConfig, LineCount, OffsetMap, Paragraph, ParagraphBlock,
    ParagraphStyles, SelectableTextRun, gutter_expansion_button_types,
};

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;

/// Resolve an image source path to an AssetSource.
///
/// Supports the following markdown image formats per the CommonMark spec:
/// https://spec.commonmark.org/0.31.2/#images
/// - Inline data: base64 `data:` URIs (e.g. notebook image outputs)
/// - URLs: `http://` or `https://` prefixed paths
/// - Absolute paths: paths starting with `/`
/// - Relative paths: all other paths, resolved relative to the document location
///
/// Note: Path canonicalization is not available on WASM targets.
pub fn resolve_asset_source_relative_to_directory(
    source: &str,
    base_directory: Option<&Path>,
) -> AssetSource {
    if let Some(data_uri_source) = asset_cache::data_uri_source(source) {
        data_uri_source
    } else if source.starts_with("http://") || source.starts_with("https://") {
        asset_cache::url_source(source)
    } else if source.starts_with("/") {
        AssetSource::LocalFile {
            path: source.to_string(),
            content_version: None,
        }
    } else {
        let resolved_path = if let Some(base_directory) = base_directory {
            base_directory.join(source)
        } else {
            Path::new(source).to_path_buf()
        };

        AssetSource::LocalFile {
            path: match resolved_path.canonicalize() {
                Ok(canon) => canon.to_string_lossy().to_string(),
                Err(_) => resolved_path.to_string_lossy().to_string(),
            },
            content_version: None,
        }
    }
}

/// Resolve an image source when its Markdown block is laid out.
///
/// Local-file metadata is read here so refreshes get a new cache key, while
/// ordinary frame rendering continues to reuse the resolved source without I/O.
fn resolve_asset_source(source: &str, base_path: Option<&Path>) -> AssetSource {
    let base_directory = base_path.map(|base| base.parent().unwrap_or(base));
    resolve_asset_source_relative_to_directory(source, base_directory)
        .with_local_file_content_version()
}

/// Default height multiplier for images when no dimensions are specified.
/// Images are rendered at 10x the base line height by default.
const DEFAULT_IMAGE_HEIGHT_LINE_MULTIPLIER: f32 = 10.0;

/// Metadata for rendering a temporary block in the render model. Temporary blocks are not selectable or editable.
#[derive(Debug, Clone, PartialEq)]
pub struct TemporaryBlock {
    pub content: String,
    pub insert_before: LineCount,
    pub line_decoration: Option<ThemeFill>,
    pub inline_text_decorations: Vec<Decoration>,
}

/// The exact replacement range of an edit.
/// Notice there are three possible prefix here, each with different semantics:
/// * `replaced`: The old content state before the edit
/// * `new`: The new content state right after the edit
/// * `resolved`: The final content state after all edits are applied (note this will be different from
///   `new` if there are other edits after this one)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreciseDelta {
    /// The old content range that was replaced (before this edit).
    pub replaced_range: Range<CharOffset>,
    /// The old point range that was replaced (before this edit).
    pub replaced_points: Range<Point>,
    /// 1-indexed byte range of the old content that was replaced (before this edit).
    pub replaced_byte_range: Range<ByteOffset>,
    /// Byte length of the newly inserted content (right after this edit).
    pub new_byte_length: usize,
    /// Point at the end of the newly inserted content (right after this edit, 1-indexed).
    pub new_end_point: Point,
    /// The range of the newly inserted content in the **final** buffer's coordinate system.
    /// For multi-delta edits, this is resolved via anchors after all edits are applied,
    /// ensuring it correctly reflects any shifts caused by subsequent edits.
    pub resolved_range: Range<CharOffset>,
}

impl PreciseDelta {
    /// The length of the resolved content range in the final buffer.
    pub fn resolved_length(&self) -> CharOffset {
        self.resolved_range
            .end
            .saturating_sub(&self.resolved_range.start)
    }
}

/// Delta after an edit operation recording the old range of rows that got replaced
/// and the content of new rows changed after the edit. This is necessary for the rendering
/// model to know what block objects need a re-layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditDelta {
    /// The exact replacement charoffset range where content was changed.
    pub precise_deltas: Vec<PreciseDelta>,
    /// Offset of the old blocks that are being replaced. The start is the first
    /// character of the first block, and the end is the end of the last block -
    /// the first character after it.
    pub old_offset: Range<CharOffset>,
    /// Content of the lines that have been changed.
    ///
    /// Wrapped in `Arc` so that `EditDelta::clone` is O(1). The render pipeline
    /// often clones a delta (e.g. when storing it in `DelayRendering::edits`)
    /// before consuming it in `layout_delta`. Without `Arc`, cloning an entire
    /// file's worth of styled blocks can add several gigabytes of transient
    /// allocation for large files.
    pub new_lines: Arc<Vec<StyledBufferBlock>>,
}

/// Render Delta that has its content laid out into TextFrames.
#[derive(Debug)]
pub struct LaidOutRenderDelta {
    /// Offset of the old blocks that are being replaced. The start is the first
    /// character of the first block, and the end is the end of the last block -
    /// the first character after it.
    pub old_offset: Range<CharOffset>,
    pub laid_out_line: Vec<BlockItem>,
    /// There is a trailing newline within this edit delta. This is used to
    /// render a placeholder for cursors that are at the end of the buffer and on a newline.
    pub trailing_newline: Option<Cursor>,
}

/// A detected URL from the updated text frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUrl {
    /// The range of the URL offsetted from the beginning of the block.
    /// Note that CharOffset is not used here as this is 1:1 mapped to the character length
    /// at the text layout stage and not the internal buffer (we could have placeholder text).
    url_range: Range<usize>,
    link: String,
}

impl ParsedUrl {
    pub fn url_range(&self) -> Range<usize> {
        self.url_range.clone()
    }

    pub fn link(&self) -> String {
        self.link.clone()
    }
}

// Utility struct for necessary arguments to lay out a TextFrame.
struct LayOutArgs {
    /// Visible characters in the current paragraph, including content text and placeholders.
    text: String,
    /// Style decorating each content fragment.
    style_runs: Vec<(Range<usize>, StyleAndFont)>,
    /// Total length of buffer-content in the run, not necessarily the same as the length of `text`.
    content_length: CharOffset,
    /// The "current" interactive run. We keep adding styled runs onto this until we find a
    /// placeholder. At that point, we end the current run and start a new one after the placeholder.
    current_interactive_run: SelectableTextRun,
    /// Accumulator for runs of interactive (non-placeholder) text within the paragraph.
    selectable_runs: Vec<SelectableTextRun>,
    /// List of detected urls.
    highlighted_urls: Vec<ParsedUrl>,
    /// Offset of the current text frame from the start of the block. This lets us handle URLs
    /// in multi-line blocks.
    frame_offset_from_block_start: usize,
    /// The next url that has not been fully laid out yet.
    next_url_index: usize,
    /// The active urls in the current text line.
    active_line_url: Vec<ParsedUrl>,
}

impl LayOutArgs {
    fn new() -> LayOutArgs {
        Self {
            text: String::new(),
            style_runs: vec![],
            content_length: CharOffset::zero(),
            current_interactive_run: SelectableTextRun {
                content_start: CharOffset::zero(),
                frame_start: FrameOffset::zero(),
                length: 0,
            },
            selectable_runs: vec![],
            highlighted_urls: vec![],
            frame_offset_from_block_start: 0,
            next_url_index: 0,
            active_line_url: vec![],
        }
    }

    /// Reset LayOutArgs for a new text line. Note that states
    /// tracked across lines are not reset.
    fn reset_for_newline(&mut self) {
        self.text = String::new();
        self.style_runs = vec![];
        self.content_length = CharOffset::zero();
        self.current_interactive_run = SelectableTextRun {
            content_start: CharOffset::zero(),
            frame_start: FrameOffset::zero(),
            length: 0,
        };
        self.selectable_runs = vec![];
        self.active_line_url = vec![];
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Builds the [`OffsetMap`] from the accumulated [`SelectableTextRun`]s.
    fn finish_offsets(&mut self) -> OffsetMap {
        // If the last styled buffer run was a placeholder, then the current interactive run will
        // be empty. Otherwise, we need to add the current run onto the accumulator before
        // creating the OffsetMap.
        if self.current_interactive_run.length > 0 {
            self.selectable_runs
                .push(self.current_interactive_run.clone());
        }
        OffsetMap::new(mem::take(&mut self.selectable_runs))
    }

    fn layout_run(
        &mut self,
        layout: &TextLayout,
        content: &StyledBufferRun,
        paragraph_styles: &ParagraphStyles,
    ) -> bool {
        let (run_content, has_new_line) = if let Some(content) = content.run.strip_suffix('\n') {
            (content, true)
        } else {
            (content.run.as_str(), false)
        };

        let offset_from_frame_start = self.text.chars().count();

        // Update the active line style run.
        let char_length = run_content.chars().count();

        // Base style passed from the buffer model.
        let base_style_and_font = layout.style_and_font(paragraph_styles, &content.text_styles);
        let mut styling_start = offset_from_frame_start;

        while let Some(url) = self.highlighted_urls.get(self.next_url_index) {
            let index_range = &url.url_range;
            let Some(url_start) = index_range
                .start
                .checked_sub(self.frame_offset_from_block_start)
            else {
                // Autodetected URLs cannot cross frame boundaries.
                report_error!(
                    "URL start offset precedes frame start offset",
                    extra: {
                        "url_start" => %index_range.start,
                        "frame_start" => %self.frame_offset_from_block_start
                    }
                );
                continue;
            };
            let url_end = index_range.end - self.frame_offset_from_block_start;

            // If url does not overlap with the current run, break and render the run in base style.
            if url_start >= offset_from_frame_start + char_length {
                break;
            }

            // Push fragments before the url.
            if url_start > styling_start {
                self.style_runs
                    .push((styling_start..url_start, base_style_and_font));
                styling_start = url_start;
            }

            let end = url_end
                .min(offset_from_frame_start + char_length)
                .max(styling_start);
            let style_and_font = add_link_to_style_and_font(base_style_and_font);
            self.style_runs.push((styling_start..end, style_and_font));

            styling_start = end;

            if end >= url_end {
                // If end is after the url_end, this means we have fully laid out the url and could
                // advance to the next one.
                self.active_line_url.push(ParsedUrl {
                    url_range: url_start..url_end,
                    link: url.link.clone(),
                });
                self.next_url_index += 1;
            } else {
                // Otherwise, the URL continues past this run, so we should lay out the rest of
                // the URL as part of the next run. There will be no more auto-detected URLs
                // that overlap with this run.
                break;
            }
        }

        if styling_start < offset_from_frame_start + char_length {
            self.style_runs.push((
                styling_start..offset_from_frame_start + char_length,
                base_style_and_font,
            ))
        }

        // Update the active line content.
        self.text.push_str(run_content);

        if content.text_styles.is_placeholder() {
            // Placeholders count as 1 content character.
            self.content_length += 1;

            // Push the previous interactive run, making the new current interactive run start
            // just _after_ the placeholder.
            let previous_run = mem::replace(
                &mut self.current_interactive_run,
                SelectableTextRun {
                    // We updated content_length above, so this is the offset just after the placeholder.
                    content_start: self.content_length,
                    // This will be the text frame character right after the last character in the
                    // placeholder.
                    frame_start: FrameOffset::from(offset_from_frame_start + char_length),
                    length: 0,
                },
            );
            self.selectable_runs.push(previous_run);
        } else {
            self.content_length += char_length;
            self.current_interactive_run.length += char_length;
        }

        if has_new_line {
            // frame_offset_from_block_start is in terms of displayed characters, not content
            // characters, so we use the length of self.text instead of self.content_length
            // (the calculation below is equal to the number of characters in self.text)
            self.frame_offset_from_block_start += offset_from_frame_start + char_length;
            self.content_length += 1;
            self.current_interactive_run.length += 1;
        }

        has_new_line
    }
}

/// Cap on the number of [`LayoutTask`]s run through a single `into_par_iter()` fan-out.
///
/// Rayon fans a chunk's tasks out across every CPU core, and each task can allocate a CoreText
/// frame plus derived `Line`/`Glyph` vectors that stay live until the whole chunk's parallel
/// collection completes. Bounding the chunk size caps how many of those allocations can be live
/// at once (APP-5392), rather than growing with the size of the input. Inputs at or under this
/// size (the overwhelming majority) still produce exactly one chunk, so they keep taking a
/// single full-width parallel pass.
const MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK: usize = 64;

/// Cap on the cumulative buffer content (in `char`s) laid out through a single `into_par_iter()`
/// fan-out.
///
/// A chunk of [`MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK`] short lines and a chunk of that many
/// multi-KB lines are not equally expensive to lay out in parallel, so bound content length too:
/// this stops a handful of unusually long lines from slipping past the task-count cap. A single
/// task longer than this cap still gets its own chunk rather than stalling chunking.
const MAX_LAYOUT_CONTENT_CHARS_PER_PARALLEL_CHUNK: usize = 64 * 1024;

/// Split `tasks` into ordered chunks bounded by both [`MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK`] and
/// [`MAX_LAYOUT_CONTENT_CHARS_PER_PARALLEL_CHUNK`], preserving task order within and across
/// chunks. `T` carries per-task metadata that chunking doesn't need to inspect (e.g. hidden-range
/// membership, or a temporary block's destination line).
fn chunk_layout_tasks<T>(tasks: Vec<(LayoutTask, T, usize)>) -> Vec<Vec<(LayoutTask, T)>> {
    let mut chunks = Vec::new();
    let mut current_chunk = Vec::new();
    let mut current_chars = 0usize;

    for (task, metadata, content_chars) in tasks {
        if !current_chunk.is_empty()
            && (current_chunk.len() >= MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK
                || current_chars + content_chars > MAX_LAYOUT_CONTENT_CHARS_PER_PARALLEL_CHUNK)
        {
            chunks.push(mem::take(&mut current_chunk));
            current_chars = 0;
        }
        current_chars += content_chars;
        current_chunk.push((task, metadata));
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

impl EditDelta {
    /// Lay out the given EditDelta into TextFrames.
    /// If hidden_lines is provided, lines within hidden ranges will be laid out as BlockItem::Hidden.
    ///
    /// Takes `&self` rather than consuming `self` so that laying out a delta never depends on
    /// `new_lines` having a single owner. `EditDelta` can be cloned by multiple editors sharing
    /// the same underlying buffer (each clone only bumps the `Arc` refcount), so unconditionally
    /// borrowing here guarantees the whole `new_lines` vector is never duplicated at layout time,
    /// regardless of how many clones of the delta are alive.
    pub fn layout_delta(
        &self,
        layout: &TextLayout,
        document_path: Option<&Path>,
        hidden_ranges: Option<RangeSet<CharOffset>>,
    ) -> LaidOutRenderDelta {
        let hidden_ranges = hidden_ranges.unwrap_or_default();

        // old_offset is in the same 1-indexed coordinate system as hidden ranges.
        let mut current_offset = (self.old_offset.start).max(CharOffset::from(1));

        // First, build a Vec of layout tasks with information about whether they're hidden, and
        // how much buffer content they carry (used below to bound parallel fan-out). Each task
        // borrows its source block from `self.new_lines` rather than taking ownership, so this
        // never clones the (potentially file-sized) block list.
        let layout_tasks: Vec<_> = self
            .new_lines
            .iter()
            .filter_map(|block| {
                let content_length = block.content_length();
                if content_length == CharOffset::zero() {
                    None
                } else {
                    let task = LayoutTask::from_styled_block(block, layout, document_path);
                    let is_hidden = hidden_ranges.contains(&current_offset);
                    current_offset += content_length;
                    Some((task, is_hidden, content_length.as_usize()))
                }
            })
            .collect();

        let last_task = layout_tasks.len().saturating_sub(1);

        // Then, run the tasks in bounded chunks (each chunk in parallel, chunks in sequence),
        // collecting (a) the laid out BlockItems in order and (b) whether or not the last item
        // ends with a newline.
        let mut block_items = Vec::with_capacity(layout_tasks.len());
        let mut has_trailing_newline = None;
        let mut chunk_start = 0;

        for chunk in chunk_layout_tasks(layout_tasks) {
            let chunk_len = chunk.len();
            let (chunk_items, chunk_trailing_newline): (Vec<_>, Last<_>) = chunk
                .into_par_iter()
                .enumerate()
                .filter_map(|(local_idx, (task, is_hidden))| {
                    let idx = chunk_start + local_idx;
                    let location = if idx == 0 {
                        BlockLocation::Start
                    } else if idx >= last_task {
                        BlockLocation::End
                    } else {
                        BlockLocation::Middle
                    };

                    match task.run(layout, location, is_hidden) {
                        Ok(result) => Some(result),
                        Err(e) => {
                            report_error!(
                                e.context("Failed to lay out BlockItem"),
                                extra: { "offset" => ?self.old_offset }
                            );
                            None
                        }
                    }
                })
                .unzip();

            block_items.extend(chunk_items);
            if let Some(trailing_newline) = chunk_trailing_newline.into_inner() {
                has_trailing_newline = Some(trailing_newline);
            }
            chunk_start += chunk_len;
        }

        // Iterate through block_items, and collapse adjacent Hidden items.
        let block_items = block_items.into_iter().fold(Vec::new(), |mut acc, item| {
            if let Some(last) = acc.last_mut() {
                // If the last item is Hidden and the current item is also Hidden,
                // we can skip adding the current item.
                if let (BlockItem::Hidden(running_config), BlockItem::Hidden(config)) =
                    (last, &item)
                {
                    *running_config += config.clone();
                    return acc;
                }
            }
            acc.push(item);
            acc
        });

        // Trailing newline is default to true. This default value is used when
        // edit delta has no new line, which means one or multiple entire lines have
        // been deleted. We should still leave a trailing newline in this case.
        let has_trailing_newline = has_trailing_newline.unwrap_or(true);
        let rich_text_styles = layout.rich_text_styles();

        LaidOutRenderDelta {
            old_offset: self.old_offset.clone(),
            laid_out_line: block_items,
            trailing_newline: has_trailing_newline.then(|| {
                Cursor::new(
                    rich_text_styles.base_line_height(),
                    rich_text_styles.cursor_width.into_pixels(),
                    rich_text_styles
                        .block_spacings
                        .from_block_style(&BufferBlockStyle::PlainText),
                    rich_text_styles.minimum_paragraph_height,
                )
            }),
        }
    }
}

/// Lay out a list of temporary blocks, in bounded chunks (see [`chunk_layout_tasks`]).
///
/// Diff navigation aggregates every hunk's removed lines into a single vector with no cap
/// (`CodeEditorModel::refresh_diff_state`), so a single large diff can drive as much unbounded
/// fan-out here as a large paste does in [`EditDelta::layout_delta`].
pub fn layout_temporary_blocks(
    blocks: Vec<TemporaryBlock>,
    layout: &TextLayout,
) -> HashMap<LineCount, Vec<BlockItem>> {
    let layout_tasks: Vec<_> = blocks
        .into_iter()
        .map(|block| {
            let content_chars = block.content.chars().count();
            (
                LayoutTask::temporary_block(
                    block.content,
                    block.line_decoration,
                    block.inline_text_decorations,
                ),
                block.insert_before,
                content_chars,
            )
        })
        .collect();

    let last_task = layout_tasks.len().saturating_sub(1);
    let mut results = Vec::with_capacity(layout_tasks.len());
    let mut chunk_start = 0;

    for chunk in chunk_layout_tasks(layout_tasks) {
        let chunk_len = chunk.len();
        let chunk_results: Vec<_> = chunk
            .into_par_iter()
            .enumerate()
            .filter_map(|(local_idx, (task, line_count))| {
                let idx = chunk_start + local_idx;
                let location = if idx == 0 {
                    BlockLocation::Start
                } else if idx >= last_task {
                    BlockLocation::End
                } else {
                    BlockLocation::Middle
                };

                match task.run(layout, location, false) {
                    Ok(result) => Some((line_count, result.0)),
                    Err(e) => {
                        report_error!(e.context("Failed to lay out temporary blocks"));
                        None
                    }
                }
            })
            .collect();

        results.extend(chunk_results);
        chunk_start += chunk_len;
    }

    results.into_iter().into_group_map()
}

/// A unit of work for parallel layout of an edit.
///
/// Borrows its source text blocks (`'a`) from the `EditDelta::new_lines` they were built from,
/// rather than owning them, so building the set of layout tasks for a delta never requires
/// cloning the (potentially file-sized) block list.
enum LayoutTask<'a> {
    /// A text block, which will be laid out in parallel.
    Text(&'a StyledTextBlock),
    /// A horizontal rule, which requires no layout.
    HorizontalRule(HorizontalRuleConfig),
    /// An image, which requires no layout.
    Image {
        alt_text: String,
        source: String,
        config: ImageBlockConfig,
        document_path: Option<PathBuf>,
    },
    /// A temporary text block.
    TemporaryBlock {
        content: String,
        line_decoration: Option<ThemeFill>,
        decoration: Vec<Decoration>,
    },
}

impl<'a> LayoutTask<'a> {
    /// Convert a block of styled content to the possibly-parallelizable layout work it requires.
    fn from_styled_block(
        content: &'a StyledBufferBlock,
        layout: &TextLayout,
        document_path: Option<&Path>,
    ) -> Self {
        match content {
            StyledBufferBlock::Item(item) => match item {
                // To lay out a horizontal rule, we only need the viewport dimensions.
                BufferBlockItem::HorizontalRule => {
                    let spacing = layout
                        .rich_text_styles()
                        .block_spacings
                        .from_block_style(&BufferBlockStyle::PlainText);
                    Self::HorizontalRule(HorizontalRuleConfig {
                        line_height: layout.rich_text_styles().base_line_height(),
                        width: layout.max_width() - spacing.x_axis_offset(),
                        spacing,
                    })
                }
                BufferBlockItem::Image {
                    alt_text,
                    source,
                    // Title is preserved on BufferBlockItem for round-trip and
                    // HTML export; the editor render currently doesn't surface it.
                    title: _,
                } => {
                    let spacing = layout
                        .rich_text_styles()
                        .block_spacings
                        .from_block_style(&BufferBlockStyle::PlainText);
                    // Default size for images - will scale based on actual image dimensions
                    let max_width = layout.max_width() - spacing.x_axis_offset();
                    let default_height = layout.rich_text_styles().base_line_height()
                        * DEFAULT_IMAGE_HEIGHT_LINE_MULTIPLIER.into_pixels();
                    Self::Image {
                        alt_text: alt_text.clone(),
                        source: source.clone(),
                        config: ImageBlockConfig {
                            width: max_width,
                            height: default_height,
                            spacing,
                        },
                        document_path: document_path.map(|p| p.to_path_buf()),
                    }
                }
            },
            StyledBufferBlock::Text(text_block) => Self::Text(text_block),
        }
    }

    fn temporary_block(
        content: String,
        line_decoration: Option<ThemeFill>,
        decoration: Vec<Decoration>,
    ) -> Self {
        Self::TemporaryBlock {
            content,
            line_decoration,
            decoration,
        }
    }

    /// Run this task, returning the laid-out block item and whether or not it has a trailing
    /// newline.
    fn run(
        self,
        layout: &TextLayout,
        location: BlockLocation,
        is_hidden: bool,
    ) -> Result<(BlockItem, bool)> {
        match self {
            Self::HorizontalRule(config) => Ok((BlockItem::HorizontalRule(config), true)),
            Self::Image {
                alt_text,
                source,
                config,
                document_path,
            } => {
                let asset_source = resolve_asset_source(&source, document_path.as_deref());
                Ok((
                    BlockItem::Image {
                        alt_text,
                        source,
                        asset_source,
                        config,
                    },
                    true, // Images are always followed by a trailing newline in the buffer
                ))
            }
            Self::Text(text_block) => layout_text_block(text_block, layout, location, is_hidden),
            Self::TemporaryBlock {
                content,
                line_decoration,
                decoration,
            } => Ok((
                BlockItem::TemporaryBlock {
                    paragraph_block: layout_temporary_block(content, layout),
                    text_decoration: decoration,
                    decoration: line_decoration,
                },
                true,
            )),
        }
    }
}

/// Estimate the number of paragraphs (logical lines) in a text block.
/// For code blocks, use the number of runs as an overestimate.
/// For non-code blocks, assume a single paragraph.
fn estimate_paragraph_count(text_block: &StyledTextBlock) -> usize {
    if matches!(text_block.style, BufferBlockStyle::CodeBlock { .. }) {
        // Code blocks don't have user-defined styling (although they do have syntax
        // highlighting). Use the number of individual runs as an overestimate for the number
        // of lines.
        text_block.block.len()
    } else {
        // Non-code blocks may only contain a single paragraph.
        1
    }
}

/// Calculate the line count to display for hidden blocks.
/// Hidden sections at the start and end of a file only ever take up one line.
/// Large hidden sections in the middle of a file may take up two lines, since
/// they'll have two separate buttons to expand the visible section up or down.
fn calculate_hidden_block_line_count(
    text_block: &StyledTextBlock,
    location: BlockLocation,
) -> usize {
    let line_count = estimate_paragraph_count(text_block);
    gutter_expansion_button_types(&location, line_count).len()
}

/// Lay out a single text block. This returns both the laid-out block item and a boolean for
/// whether or not it has a trailing newline.
///
/// In theory, this function shouldn't error, but we've had panics where there
/// were no paragraphs. So a `Result` is returned for now so that we can bubble
/// up the error and add appropriate logging. See CLD-2093.
fn layout_text_block(
    text_block: &StyledTextBlock,
    layout: &TextLayout,
    location: BlockLocation,
    is_hidden: bool,
) -> Result<(BlockItem, bool)> {
    if is_hidden {
        // If all text is hidden, return a BlockItem::Hidden without doing any layout
        let content_length = text_block.content_length;
        let line_count = calculate_hidden_block_line_count(text_block, location);
        return Ok((
            BlockItem::Hidden(HiddenBlockConfig::new(
                line_count.into(),
                content_length,
                location,
            )),
            false,
        ));
    }

    // Accumulator for the current line (paragraph) of text.
    let mut active_line = LayOutArgs::new();
    // Accumulator for fully laid-out paragraphs.
    let mut paragraphs = Vec::with_capacity(estimate_paragraph_count(text_block));

    let rich_text_styles = layout.rich_text_styles();
    let spacing = rich_text_styles
        .block_spacings
        .from_block_style(&text_block.style);
    let paragraph_styles = layout.paragraph_styles(&text_block.style);

    if rich_text_styles.highlight_urls {
        active_line.highlighted_urls = highlight_urls(&text_block.block);
    }

    active_line.next_url_index = 0;

    for run in &text_block.block {
        let new_line = active_line.layout_run(layout, run, &paragraph_styles);

        if new_line {
            let offsets = active_line.finish_offsets();
            paragraphs.push(Paragraph::new(
                layout.layout_text(
                    &active_line.text,
                    &paragraph_styles,
                    &spacing,
                    &active_line.style_runs,
                ),
                offsets,
                active_line.content_length,
                active_line.active_line_url.clone(),
                spacing,
                rich_text_styles.minimum_paragraph_height,
            ));
            active_line.reset_for_newline();
        }
    }

    let has_trailing_newline = if !active_line.is_empty() {
        let offsets = active_line.finish_offsets();
        paragraphs.push(Paragraph::new(
            layout.layout_text(
                &active_line.text,
                &paragraph_styles,
                &spacing,
                &active_line.style_runs,
            ),
            offsets,
            // This assumes that every line is ended by a newline, block marker, or
            // block item. Currently, that's true of every paragraph but the last,
            // if it's plain text without a trailing newline. Acting as if there's
            // a newline character there lets us render the cursor at the end of
            // the buffer and distinguish between the end of the last paragraph and
            // a potential TrailingNewline item.
            active_line.content_length + 1,
            active_line.active_line_url,
            spacing,
            rich_text_styles.minimum_paragraph_height,
        ));
        false
    } else {
        true
    };

    let block_item = match &text_block.style {
        BufferBlockStyle::CodeBlock { code_block_type } => {
            let code_block_type = code_block_type.clone();
            Vec1::try_from_vec(paragraphs)
                .ok()
                .map(|p| {
                    let paragraph_block = ParagraphBlock::new(p);
                    BlockItem::RunnableCodeBlock {
                        paragraph_block,
                        code_block_type,
                    }
                })
                .ok_or_else(|| anyhow!("Code block should have at least one paragraph"))
        }
        BufferBlockStyle::TaskList {
            indent_level,
            complete,
        } => {
            let indent_level = *indent_level;
            let complete = *complete;
            debug_assert_eq!(
                paragraphs.len(),
                1,
                "Task list paragraphs should only have one line."
            );
            paragraphs
                .pop()
                .map(|paragraph| BlockItem::TaskList {
                    indent_level,
                    complete,
                    paragraph,
                    mouse_state: Default::default(),
                })
                .ok_or_else(|| anyhow!("Task list item should have one paragraph"))
        }
        BufferBlockStyle::UnorderedList { indent_level } => {
            let indent_level = *indent_level;
            debug_assert_eq!(
                paragraphs.len(),
                1,
                "Unordered list paragraphs should only have one line."
            );
            paragraphs
                .pop()
                .map(|paragraph| BlockItem::UnorderedList {
                    indent_level,
                    paragraph,
                })
                .ok_or_else(|| anyhow!("Unordered list item should have one paragraph"))
        }
        BufferBlockStyle::OrderedList {
            indent_level,
            number,
        } => {
            let indent_level = *indent_level;
            let number = *number;
            debug_assert_eq!(
                paragraphs.len(),
                1,
                "Ordered list paragraphs should only have one line."
            );
            paragraphs
                .pop()
                .map(|paragraph| BlockItem::OrderedList {
                    indent_level,
                    number,
                    paragraph,
                })
                .ok_or_else(|| anyhow!("Ordered list item should have one paragraph"))
        }
        BufferBlockStyle::Header { header_size } => {
            let header_size = *header_size;
            debug_assert_eq!(
                paragraphs.len(),
                1,
                "Header paragraphs should only have one line."
            );
            paragraphs
                .pop()
                .map(|paragraph| BlockItem::Header {
                    header_size,
                    paragraph,
                })
                .ok_or_else(|| anyhow!("Header item should have one paragraph"))
        }
        BufferBlockStyle::PlainText => {
            debug_assert_eq!(
                paragraphs.len(),
                1,
                "Plain text paragraphs should only have one line."
            );

            paragraphs
                .pop()
                .map(BlockItem::Paragraph)
                .ok_or_else(|| anyhow!("Plain text item should have one paragraph"))
        }
    };

    block_item.map(|item| (item, has_trailing_newline))
}

/// Lay out a single temporary text block.
fn layout_temporary_block(content: String, layout: &TextLayout) -> ParagraphBlock {
    // Accumulator for the current paragraph.
    let mut active_line = LayOutArgs::new();
    let lines = content.lines();

    let mut paragraphs = Vec::new();
    // Default use the plain text block style and spacing.
    let styles = layout.paragraph_styles(&BufferBlockStyle::PlainText);
    let rich_text_styles = layout.rich_text_styles();
    let spacing = rich_text_styles
        .block_spacings
        .from_block_style(&BufferBlockStyle::PlainText);

    for run in lines {
        let mut content = run.to_string();
        content.push('\n');
        active_line.layout_run(
            layout,
            &StyledBufferRun {
                run: content,
                text_styles: Default::default(),
                block_style: BufferBlockStyle::PlainText,
            },
            &styles,
        );

        let offsets = active_line.finish_offsets();
        paragraphs.push(Paragraph::new(
            layout.layout_text(
                &active_line.text,
                &styles,
                &spacing,
                &active_line.style_runs,
            ),
            offsets,
            active_line.content_length,
            active_line.active_line_url.clone(),
            spacing,
            rich_text_styles.minimum_paragraph_height,
        ));
        active_line.reset_for_newline();
    }

    ParagraphBlock::new(
        Vec1::try_from_vec(paragraphs).expect("Temporary block should have at least one paragraph"),
    )
}

/// Parsed out all urls in the given list of StyledBufferRun.
///
/// Links cannot be auto-linked and break up potential URLs.
fn highlight_urls(runs: &[StyledBufferRun]) -> Vec<ParsedUrl> {
    let mut state = UrlDetectionState::default();

    for content in runs {
        let (run_content, has_newline) = if let Some(content) = content.run.strip_suffix('\n') {
            (content, true)
        } else {
            (content.run.as_str(), false)
        };

        state.total_content.push_str(run_content);

        if content.text_styles.is_link() || content.text_styles.is_placeholder() {
            state.reset();
            // If this run is already a link, it cannot contain additional URLs. Likewise,
            // placeholders act as URL boundaries.
            // However, we still need to track positions within the overall block's text.
            state.frame_offset += run_content.chars().count();
            continue;
        }

        let mut run_length = 0;
        for (i, c) in run_content.chars().enumerate() {
            // Reference to https://docs.rs/urlocator/latest/urlocator/#example-url-boundaries
            // We know we have fully parsed an url when the locator advances from the `UrlLocation::Url`
            // to the `UrlLocation::Reset` stage.
            match state.locator.advance(c) {
                UrlLocation::Url(length, end_offset) => {
                    // end_offset is the number of characters between the parser location and the
                    // end of the URL (because the parser might be at a `.` or other character
                    // that's valid within a URL but not at its end).
                    let parser_location = i + state.frame_offset;
                    let end = parser_location
                        .checked_sub(end_offset as usize)
                        .expect("Auto-detected URLs cannot span frames");
                    let start = (end + 1).saturating_sub(length as usize);

                    if let Some(url) = &mut state.last_url {
                        if url.start == start {
                            url.end = end + 1
                        } else {
                            url.start = start;
                            url.end = end + 1;
                        }
                    } else {
                        state.last_url = Some(start..end);
                    }
                }
                UrlLocation::Reset => {
                    state.finish_url();
                }
                _ => (),
            }
            run_length += 1;
        }

        state.frame_offset += run_length;

        if has_newline {
            state.reset();
        }
    }

    state.finish_url();

    state.urls
}

/// State helper for highlighting URLs.
#[derive(Default)]
struct UrlDetectionState {
    /// The starting character offset of the current run within the block's displayed text. This is
    /// relative to all text frames within the block, not just the one containing the run.
    frame_offset: usize,
    /// Accumulator for detected URLs.
    urls: Vec<ParsedUrl>,
    /// The URL that is currently being parsed.
    last_url: Option<Range<usize>>,
    /// Accumulator for the overall [`TextFrame`] content, so that we can extract URL values.
    total_content: String,
    locator: UrlLocator,
}

impl UrlDetectionState {
    /// Finish the current URL.
    fn finish_url(&mut self) {
        if let Some(url) = self.last_url.take()
            && let Some(link) = char_slice(&self.total_content, url.start, url.end)
        {
            self.urls.push(ParsedUrl {
                url_range: url.start..url.end,
                link: link.to_string(),
            })
        }
    }

    /// Reset the detection state after encountering something that forces a URL boundary (such as
    /// a newline or hyperlink).
    fn reset(&mut self) {
        self.finish_url();
        // Reset the UrlLocator so that text surrounding the break isn't detected as
        // a URL. We can do this because UrlLocator reports a length and offset from the
        // current end position, which we can use as long as block_offset is correct.
        self.locator = UrlLocator::new();
    }
}
