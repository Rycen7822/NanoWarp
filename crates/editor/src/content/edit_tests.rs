use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rangemap::RangeSet;
use string_offset::CharOffset;
use warpui_core::App;
use warpui_core::assets::asset_cache::AssetSource;
use warpui_core::fonts::{Properties, Weight};
use warpui_core::text_layout::{StyleAndFont, TextStyle};

use super::{
    LayOutArgs, LayoutTask, MAX_LAYOUT_CONTENT_CHARS_PER_PARALLEL_CHUNK,
    MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK, chunk_layout_tasks, layout_temporary_blocks,
};
use crate::content::buffer::{StyledBufferBlock, StyledBufferRun, StyledTextBlock};
use crate::content::edit::{
    EditDelta, ParsedUrl, TemporaryBlock, highlight_urls, resolve_asset_source,
    resolve_asset_source_relative_to_directory,
};
use crate::content::text::{BufferBlockStyle, CodeBlockType, TextStylesWithMetadata};
use crate::render::layout::{TextLayout, add_link_to_style_and_font};
use crate::render::model::test_utils::TEST_STYLES;
use crate::render::model::{BlockItem, CODE_EDITOR_HIDDEN_SECTION_EXPANSION_LINES, LineCount};

#[test]
fn test_highlight_urls() {
    let mut test_styled_buffer_runs = vec![
        StyledBufferRun {
            run: "https:".to_string(),
            text_styles: TextStylesWithMetadata::default().bold(),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "//".to_string(),
            text_styles: TextStylesWithMetadata::default().italic(),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "google.com".to_string(),
            text_styles: TextStylesWithMetadata::default(),
            block_style: BufferBlockStyle::PlainText,
        },
    ];

    assert_eq!(
        highlight_urls(&test_styled_buffer_runs),
        [ParsedUrl {
            url_range: 0..18,
            link: "https://google.com".to_string()
        },]
    );

    test_styled_buffer_runs.extend(vec![
        StyledBufferRun {
            run: " abc ".to_string(),
            text_styles: TextStylesWithMetadata::default().bold(),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "https://warp.dev".to_string(),
            text_styles: TextStylesWithMetadata::default(),
            block_style: BufferBlockStyle::PlainText,
        },
    ]);

    assert_eq!(
        highlight_urls(&test_styled_buffer_runs),
        [
            ParsedUrl {
                url_range: 0..18,
                link: "https://google.com".to_string()
            },
            ParsedUrl {
                url_range: 23..39,
                link: "https://warp.dev".to_string()
            }
        ]
    );
}

#[test]
fn test_highlight_urls_unicode() {
    let test_runs = vec![StyledBufferRun {
        run: "This (not https://example.com) is a 🔥 link about a 🇨🇦 🏡:\u{a0}https://warp.dev"
            .to_string(),
        text_styles: Default::default(),
        block_style: BufferBlockStyle::PlainText,
    }];
    assert_eq!(
        highlight_urls(&test_runs),
        [
            ParsedUrl {
                url_range: 10..29,
                link: "https://example.com".to_string()
            },
            ParsedUrl {
                url_range: 57..73,
                link: "https://warp.dev".to_string()
            }
        ]
    )
}

#[test]
fn test_highlight_incomplete_url() {
    // Tests that we can highlight the valid range of a URL that's still being typed.
    // URLs can't end in a `.`, so the detector stops at `www`.
    let test_runs = vec![StyledBufferRun {
        run: "Word https://www. later".to_string(),
        text_styles: Default::default(),
        block_style: BufferBlockStyle::PlainText,
    }];
    assert_eq!(
        highlight_urls(&test_runs),
        [ParsedUrl {
            url_range: 5..16,
            link: "https://www".to_string()
        },]
    )
}

#[test]
fn test_links_not_auto_highlighted() {
    // Test that links whose tags look like URLs aren't auto-linked, but also that they don't
    // prevent auto-linking other URLs.
    let runs = &[
        StyledBufferRun {
            run: "first link is https://warp.dev ".to_string(),
            text_styles: Default::default(),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "http://example.com".to_string(),
            text_styles: TextStylesWithMetadata::default().link("https://warp.dev".to_string()),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: " second is https://google.com".to_string(),
            text_styles: Default::default(),
            block_style: BufferBlockStyle::PlainText,
        },
    ];

    assert_eq!(
        highlight_urls(runs),
        &[
            ParsedUrl {
                url_range: 14..30,
                link: "https://warp.dev".to_string()
            },
            ParsedUrl {
                url_range: 60..78,
                link: "https://google.com".to_string()
            }
        ]
    )
}

#[test]
fn test_highlight_url_before_link() {
    // Test that a URL right before an actual hyperlink is still highlighted.
    let runs = &[
        StyledBufferRun {
            run: "https://example.com".to_string(),
            text_styles: Default::default(),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "hyperlink".to_string(),
            text_styles: TextStylesWithMetadata::default().link("https://example.com".to_string()),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "https://warp.dev".to_string(),
            text_styles: Default::default(),
            block_style: BufferBlockStyle::PlainText,
        },
    ];

    assert_eq!(
        highlight_urls(runs),
        vec![
            ParsedUrl {
                url_range: 0..19,
                link: "https://example.com".to_string()
            },
            ParsedUrl {
                url_range: 28..44,
                link: "https://warp.dev".to_string()
            }
        ]
    )
}

#[test]
fn test_text_around_link_not_auto_highlighted() {
    // Test that text which, without the link in the middle, would be a URL is not auto-linked.
    let runs = &[
        StyledBufferRun {
            run: "ht".to_string(),
            text_styles: Default::default(),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "alink".to_string(),
            text_styles: TextStylesWithMetadata::default().link("https://warp.dev".to_string()),
            block_style: BufferBlockStyle::PlainText,
        },
        StyledBufferRun {
            run: "tps://example.com".to_string(),
            text_styles: Default::default(),
            block_style: BufferBlockStyle::PlainText,
        },
    ];

    assert!(highlight_urls(runs).is_empty());
}

#[test]
fn test_layout_delta_never_takes_ownership_of_new_lines_with_multiple_owners() {
    // Regression test for APP-4844: `EditDelta::new_lines` is wrapped in an `Arc` so that
    // cloning a delta (e.g. to stash it in `DelayRendering::edits`, or because multiple editors
    // share the same underlying buffer) is O(1) instead of O(file size). `layout_delta` must not
    // depend on `new_lines` having a single owner to stay cheap: it takes `&self` and only ever
    // borrows through the `Arc`, so laying out a delta can never fall back to cloning the whole
    // (potentially file-sized) block list, no matter how many clones of the delta are alive.
    //
    // This exercises `layout_delta` itself (not just raw `Arc` semantics) with two live clones of
    // the same delta -- the exact shape of two `CodeEditorModel`s sharing one buffer, each
    // holding their own clone of the `ContentChanged` event's delta -- and confirms neither
    // layout call touches the `Arc`'s strong count or invalidates the other clone.
    App::test((), |app| async move {
        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );

            let block = StyledBufferBlock::Text(StyledTextBlock {
                block: vec![StyledBufferRun {
                    run: "hello\n".to_string(),
                    text_styles: Default::default(),
                    block_style: BufferBlockStyle::PlainText,
                }],
                style: BufferBlockStyle::PlainText,
                content_length: CharOffset::from(6),
            });

            let delta = EditDelta {
                new_lines: Arc::new(vec![block]),
                old_offset: CharOffset::from(1)..CharOffset::from(1),
                ..EditDelta::default()
            };

            // Simulate two editors sharing the same buffer, each holding their own clone of the
            // delta emitted by the shared `ContentChanged` event.
            let editor_a_delta = delta.clone();
            let editor_b_delta = delta.clone();
            drop(delta);
            assert_eq!(Arc::strong_count(&editor_a_delta.new_lines), 2);

            let laid_out_a = editor_a_delta.layout_delta(
                &text_layout,
                None,
                None,
            );
            assert_eq!(laid_out_a.laid_out_line.len(), 1);
            assert_eq!(
                Arc::strong_count(&editor_a_delta.new_lines),
                2,
                "layout_delta must not take ownership of new_lines"
            );

            let laid_out_b = editor_b_delta.layout_delta(
                &text_layout,
                None,
                None,
            );
            assert_eq!(laid_out_b.laid_out_line.len(), 1);
            assert_eq!(
                Arc::strong_count(&editor_a_delta.new_lines),
                2,
                "both clones of the delta must remain valid, sharing the same allocation, after layout"
            );
            assert!(Arc::ptr_eq(&editor_a_delta.new_lines, &editor_b_delta.new_lines));
        });
    })
}

#[test]
fn test_layout_partial_url() {
    // Regression test for laying out a partially-styled autodetected URL (CLD-871).
    App::test((), |app| async move {
        let runs = vec![
            StyledBufferRun {
                run: "A link: https://www.".to_string(),
                text_styles: Default::default(),
                block_style: BufferBlockStyle::PlainText,
            },
            StyledBufferRun {
                run: "example.com".to_string(),
                text_styles: TextStylesWithMetadata::default().bold(),
                block_style: BufferBlockStyle::PlainText,
            },
            StyledBufferRun {
                run: "/path text".to_string(),
                text_styles: Default::default(),
                block_style: BufferBlockStyle::PlainText,
            },
        ];

        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );

            let mut line = LayOutArgs::new();
            line.highlighted_urls = highlight_urls(&runs);
            line.next_url_index = 0;

            for run in runs.iter() {
                line.layout_run(
                    &text_layout,
                    run,
                    &text_layout.paragraph_styles(&BufferBlockStyle::PlainText),
                );
            }

            let family_id = TEST_STYLES.base_text.font_family;
            let base_styles =
                StyleAndFont::new(family_id, Properties::default(), TextStyle::default());

            assert_eq!(&line.text, "A link: https://www.example.com/path text");
            assert_eq!(
                &line.style_runs,
                &[
                    (0..8, base_styles),
                    (8..20, add_link_to_style_and_font(base_styles)),
                    (
                        20..31,
                        add_link_to_style_and_font(StyleAndFont::new(
                            family_id,
                            Properties::default().weight(Weight::Bold),
                            TextStyle::default()
                        ))
                    ),
                    (31..36, add_link_to_style_and_font(base_styles)),
                    (36..41, base_styles)
                ]
            )
        });
    })
}

#[test]
fn test_resolve_asset_source_relative_to_directory_uses_base_directory() {
    let asset_source =
        resolve_asset_source_relative_to_directory("diagram.png", Some(Path::new("/tmp/session")));

    match asset_source {
        AssetSource::LocalFile { path, .. } => {
            assert_eq!(Path::new(&path), Path::new("/tmp/session/diagram.png"));
        }
        source => panic!("expected local file asset source, got {source:?}"),
    }
}
fn unique_markdown_image_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after the Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "warp_editor_markdown_image_{}_{nonce}.png",
        std::process::id()
    ))
}
#[test]
fn test_resolve_asset_source_versions_local_files_for_markdown_layout() {
    let image_path = unique_markdown_image_path();
    std::fs::write(&image_path, b"old").expect("write initial image contents");
    let document_path = image_path.with_file_name("document.md");
    let image_name = image_path
        .file_name()
        .expect("image path should have a file name")
        .to_string_lossy();

    let initial = resolve_asset_source(&image_name, Some(&document_path));
    assert!(matches!(
        &initial,
        AssetSource::LocalFile {
            content_version: Some(_),
            ..
        }
    ));

    std::fs::write(&image_path, b"updated image contents").expect("update image contents");
    let updated = resolve_asset_source(&image_name, Some(&document_path));
    assert_ne!(
        initial, updated,
        "an updated Markdown image should produce a new cache key"
    );

    let _ = std::fs::remove_file(image_path);
}

#[test]
fn test_resolve_asset_source_leaves_non_local_markdown_images_unchanged() {
    let document_path = Path::new("/tmp/document.md");
    let base_directory = document_path.parent();

    for source in [
        "https://example.com/image.png",
        "data:image/png;base64,iVBORw0KGgo=",
    ] {
        assert_eq!(
            resolve_asset_source(source, Some(document_path)),
            resolve_asset_source_relative_to_directory(source, base_directory),
            "non-local source should not be changed: {source}"
        );
    }
}

#[test]
fn test_layout_code_block_urls() {
    // Regression test for laying out URLs in a code block, which contains multiple lines.
    App::test((), |app| async move {
        let runs = vec![
            StyledBufferRun {
                run: "curl -o myfile.txt http://example.com/myfile.txt\n".to_string(),
                text_styles: Default::default(),
                block_style: BufferBlockStyle::CodeBlock {
                    code_block_type: CodeBlockType::Shell,
                },
            },
            StyledBufferRun {
                run: "vim myfile.txt\n".to_string(),
                text_styles: Default::default(),
                block_style: BufferBlockStyle::CodeBlock {
                    code_block_type: CodeBlockType::Shell,
                },
            },
            StyledBufferRun {
                run: "rsync myfile.txt ssh://user@server.com\n".to_string(),
                text_styles: Default::default(),
                block_style: BufferBlockStyle::CodeBlock {
                    code_block_type: CodeBlockType::Shell,
                },
            },
        ];

        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );
            let paragraph_styles = text_layout.paragraph_styles(&BufferBlockStyle::CodeBlock {
                code_block_type: CodeBlockType::Shell,
            });
            let family_id = TEST_STYLES.code_text.font_family;
            let base_styles =
                StyleAndFont::new(family_id, Properties::default(), TextStyle::default());

            let mut line = LayOutArgs::new();
            line.highlighted_urls = highlight_urls(&runs);
            line.next_url_index = 0;

            // First, make sure that we detected the URLs correctly.
            assert_eq!(
                &line.highlighted_urls,
                &[
                    ParsedUrl {
                        url_range: 19..48,
                        link: "http://example.com/myfile.txt".to_string()
                    },
                    ParsedUrl {
                        // URL offsets count painted characters, not newlines.
                        url_range: 79..100,
                        link: "ssh://user@server.com".to_string()
                    }
                ]
            );

            // Lay out each line of code 1 by 1 to verify the intermediate state.

            assert!(line.layout_run(&text_layout, &runs[0], &paragraph_styles));
            assert_eq!(
                &line.text,
                "curl -o myfile.txt http://example.com/myfile.txt"
            );
            assert_eq!(
                &line.style_runs,
                &[
                    (0..19, base_styles),
                    (19..48, add_link_to_style_and_font(base_styles)),
                ]
            );

            line.reset_for_newline();
            assert!(line.layout_run(&text_layout, &runs[1], &paragraph_styles));
            assert_eq!(&line.text, "vim myfile.txt");
            assert_eq!(&line.style_runs, &[(0..14, base_styles)]);

            line.reset_for_newline();
            assert!(line.layout_run(&text_layout, &runs[2], &paragraph_styles));
            assert_eq!(&line.text, "rsync myfile.txt ssh://user@server.com");
            assert_eq!(
                &line.style_runs,
                &[
                    (0..17, base_styles),
                    (17..38, add_link_to_style_and_font(base_styles)),
                ]
            );
        });
    })
}

#[test]
fn test_chunk_layout_tasks_bounds_by_task_count() {
    let make_tasks = |count: usize| -> Vec<(LayoutTask, bool, usize)> {
        (0..count)
            .map(|_| {
                (
                    LayoutTask::temporary_block(String::new(), None, vec![]),
                    false,
                    1,
                )
            })
            .collect()
    };

    let chunks = chunk_layout_tasks(make_tasks(MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK + 5));
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].len(), MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK);
    assert_eq!(chunks[1].len(), 5);

    // Exactly at the cap should still be a single chunk.
    let chunks = chunk_layout_tasks(make_tasks(MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK));
    assert_eq!(chunks.len(), 1);
}

#[test]
fn test_chunk_layout_tasks_bounds_by_content_length() {
    let make_tasks = |lengths: &[usize]| -> Vec<(LayoutTask, bool, usize)> {
        lengths
            .iter()
            .map(|&len| {
                (
                    LayoutTask::temporary_block(String::new(), None, vec![]),
                    false,
                    len,
                )
            })
            .collect()
    };

    // Each task is over half the content-length cap, so every task should start a new chunk
    // well before the task-count cap is reached.
    let oversized = MAX_LAYOUT_CONTENT_CHARS_PER_PARALLEL_CHUNK / 2 + 1;
    let chunks = chunk_layout_tasks(make_tasks(&[oversized, oversized, oversized]));
    assert_eq!(
        chunks.len(),
        3,
        "each oversized task should start a new chunk"
    );
    assert!(chunks.iter().all(|chunk| chunk.len() == 1));

    // A single task larger than the cap must still get its own chunk rather than stalling.
    let huge = MAX_LAYOUT_CONTENT_CHARS_PER_PARALLEL_CHUNK * 4;
    let chunks = chunk_layout_tasks(make_tasks(&[huge]));
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].len(), 1);
}

/// Builds a single-line, single-run PlainText block whose content is exactly `content_len`
/// characters (including the trailing newline), so its laid-out `content_length` uniquely
/// identifies the block for order verification.
fn identifiable_text_block(content_len: usize) -> StyledBufferBlock {
    let run = "a".repeat(content_len - 1) + "\n";
    StyledBufferBlock::Text(StyledTextBlock {
        block: vec![StyledBufferRun {
            run,
            text_styles: TextStylesWithMetadata::default(),
            block_style: BufferBlockStyle::PlainText,
        }],
        style: BufferBlockStyle::PlainText,
        content_length: CharOffset::from(content_len),
    })
}

#[test]
fn test_layout_delta_chunk_boundary_preserves_order_hidden_collapsing_and_trailing_newline() {
    // Regression test for APP-5392: bounding EditDelta::layout_delta's parallel fan-out into
    // chunks must not change its observable behavior. This delta spans multiple chunks (given
    // MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK), with a hidden run that straddles a chunk boundary.
    App::test((), |app| async move {
        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );

            const TOTAL_BLOCKS: usize = MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK * 2 + 12;
            // Straddles the boundary between the first and second chunks.
            const HIDDEN_START: usize = MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK - 4;
            const HIDDEN_END: usize = MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK + 11;

            let mut new_lines = Vec::with_capacity(TOTAL_BLOCKS);
            let mut block_starts = Vec::with_capacity(TOTAL_BLOCKS);
            let mut content_lengths = Vec::with_capacity(TOTAL_BLOCKS);
            let mut offset = CharOffset::from(1);

            for i in 0..TOTAL_BLOCKS {
                let content_len = i + 3;
                block_starts.push(offset);
                content_lengths.push(content_len);
                new_lines.push(identifiable_text_block(content_len));
                offset += content_len;
            }

            let mut hidden_ranges = RangeSet::new();
            hidden_ranges.insert(block_starts[HIDDEN_START]..block_starts[HIDDEN_END]);

            let delta = EditDelta {
                old_offset: CharOffset::from(1)..offset,
                new_lines: Arc::new(new_lines),
                ..Default::default()
            };

            let laid_out = delta.layout_delta(&text_layout, None, Some(hidden_ranges));

            // The contiguous hidden run should collapse into exactly one BlockItem::Hidden.
            let expected_len = HIDDEN_START + 1 + (TOTAL_BLOCKS - HIDDEN_END);
            assert_eq!(laid_out.laid_out_line.len(), expected_len);

            for (item, &expected_block_len) in laid_out.laid_out_line[0..HIDDEN_START]
                .iter()
                .zip(&content_lengths[0..HIDDEN_START])
            {
                assert!(
                    matches!(item, BlockItem::Paragraph(_)),
                    "expected a visible block, got {item:?}"
                );
                assert_eq!(item.content_length(), CharOffset::from(expected_block_len));
            }

            let hidden_item = &laid_out.laid_out_line[HIDDEN_START];
            assert!(
                matches!(hidden_item, BlockItem::Hidden(_)),
                "the hidden run should collapse to a single item, got {hidden_item:?}"
            );
            let expected_hidden_length: usize =
                content_lengths[HIDDEN_START..HIDDEN_END].iter().sum();
            assert_eq!(
                hidden_item.content_length(),
                CharOffset::from(expected_hidden_length)
            );

            for (item, &expected_block_len) in laid_out.laid_out_line[HIDDEN_START + 1..]
                .iter()
                .zip(&content_lengths[HIDDEN_END..TOTAL_BLOCKS])
            {
                assert!(
                    matches!(item, BlockItem::Paragraph(_)),
                    "expected a visible block, got {item:?}"
                );
                assert_eq!(item.content_length(), CharOffset::from(expected_block_len));
            }

            // The last block is visible and ends with a newline, so the delta should report a
            // trailing newline, matching what an unchunked single-pass layout would produce.
            assert!(laid_out.trailing_newline.is_some());
        });
    })
}

#[test]
fn test_layout_delta_single_chunk_matches_direct_layout() {
    // A delta that fits within a single chunk should behave identically to laying out each
    // block directly: no hidden collapsing, and a trailing newline exactly when the last block
    // ends in one.
    App::test((), |app| async move {
        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );

            let new_lines = vec![
                identifiable_text_block(3),
                identifiable_text_block(4),
                identifiable_text_block(5),
            ];
            let total_len: usize = new_lines
                .iter()
                .map(StyledBufferBlock::content_length)
                .map(CharOffset::as_usize)
                .sum();

            let delta = EditDelta {
                old_offset: CharOffset::from(1)..CharOffset::from(1 + total_len),
                new_lines: Arc::new(new_lines),
                ..Default::default()
            };

            let laid_out = delta.layout_delta(&text_layout, None, None);

            assert_eq!(laid_out.laid_out_line.len(), 3);
            assert_eq!(
                laid_out.laid_out_line[0].content_length(),
                CharOffset::from(3)
            );
            assert_eq!(
                laid_out.laid_out_line[1].content_length(),
                CharOffset::from(4)
            );
            assert_eq!(
                laid_out.laid_out_line[2].content_length(),
                CharOffset::from(5)
            );
            assert!(laid_out.trailing_newline.is_some());
        });
    })
}

/// Builds a hidden, isolated `CodeBlock`-styled block whose gutter-button count (and thus its
/// laid-out `line_count`) directly observes the `BlockLocation` it was laid out with: Start/End
/// always get one button, but a genuine Middle location with `run_count >=
/// CODE_EDITOR_HIDDEN_SECTION_EXPANSION_LINES` gets two. `run_count` only matters for the Middle
/// case; the run contents are never read since the block is hidden.
fn isolated_hidden_code_block(run_count: usize, content_len: usize) -> StyledBufferBlock {
    let style = BufferBlockStyle::CodeBlock {
        code_block_type: CodeBlockType::Shell,
    };
    StyledBufferBlock::Text(StyledTextBlock {
        block: vec![
            StyledBufferRun {
                run: String::new(),
                text_styles: TextStylesWithMetadata::default(),
                block_style: style.clone(),
            };
            run_count
        ],
        style,
        content_length: CharOffset::from(content_len),
    })
}

#[test]
fn test_layout_delta_block_location_is_global_across_chunk_boundaries() {
    // Regression test for APP-5392: BlockLocation must be computed from the delta's global
    // index, not a chunk-local one. A hidden block's gutter-button count only depends on its
    // BlockLocation when it's genuinely Middle with a large enough hidden run (2 buttons) vs.
    // Start/End (always 1 button), so an isolated hidden block at a later chunk's first index
    // makes a chunk-local-index regression directly observable.
    App::test((), |app| async move {
        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );

            const TOTAL_BLOCKS: usize = MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK * 2 + 10;
            const HIDDEN_AT_START: usize = 0;
            const HIDDEN_AT_TRUE_MIDDLE: usize = 5;
            // The first index of the second chunk: local index 0, but not the global start.
            const HIDDEN_AT_CHUNK_BOUNDARY: usize = MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK;
            const HIDDEN_AT_END: usize = TOTAL_BLOCKS - 1;
            const RUN_COUNT: usize = CODE_EDITOR_HIDDEN_SECTION_EXPANSION_LINES + 5;

            let hidden_indices = [
                HIDDEN_AT_START,
                HIDDEN_AT_TRUE_MIDDLE,
                HIDDEN_AT_CHUNK_BOUNDARY,
                HIDDEN_AT_END,
            ];

            let mut new_lines = Vec::with_capacity(TOTAL_BLOCKS);
            let mut block_starts = Vec::with_capacity(TOTAL_BLOCKS);
            let mut offset = CharOffset::from(1);

            for i in 0..TOTAL_BLOCKS {
                block_starts.push(offset);
                if hidden_indices.contains(&i) {
                    new_lines.push(isolated_hidden_code_block(RUN_COUNT, 1));
                    offset += 1;
                } else {
                    new_lines.push(identifiable_text_block(3));
                    offset += 3;
                }
            }

            // Each hidden index is isolated (its neighbors are visible), so none of them merge.
            let mut hidden_ranges = RangeSet::new();
            for &i in &hidden_indices {
                hidden_ranges.insert(block_starts[i]..block_starts[i] + CharOffset::from(1));
            }

            let delta = EditDelta {
                old_offset: CharOffset::from(1)..offset,
                new_lines: Arc::new(new_lines),
                ..Default::default()
            };

            let laid_out = delta.layout_delta(
                &text_layout,
                None,
                Some(hidden_ranges),
            );

            // No collapsing occurred, so output indices line up with input indices.
            assert_eq!(laid_out.laid_out_line.len(), TOTAL_BLOCKS);

            let line_count_at = |global_idx: usize| match &laid_out.laid_out_line[global_idx] {
                BlockItem::Hidden(config) => config.line_count(),
                other => panic!("expected a Hidden item at index {global_idx}, got {other:?}"),
            };

            assert_eq!(
                line_count_at(HIDDEN_AT_START),
                LineCount::from(1),
                "genuine Start should always get a single gutter button"
            );
            assert_eq!(
                line_count_at(HIDDEN_AT_TRUE_MIDDLE),
                LineCount::from(2),
                "genuine Middle with a large hidden block should get two gutter buttons"
            );
            assert_eq!(
                line_count_at(HIDDEN_AT_CHUNK_BOUNDARY),
                LineCount::from(2),
                "a later chunk's first task is still Middle (global index), not Start (chunk-local index)"
            );
            assert_eq!(
                line_count_at(HIDDEN_AT_END),
                LineCount::from(1),
                "genuine End should always get a single gutter button"
            );
        });
    })
}

#[test]
fn test_layout_delta_trailing_newline_carries_over_when_final_chunk_fully_fails() {
    // Regression test for APP-5392: when every task in the final chunk fails, the
    // trailing-newline result must still come from the last *successful* task in an earlier
    // chunk, matching the old single-pass find_last() semantics over the whole (possibly
    // filtered) sequence, rather than resetting to the default because the last chunk
    // contributed nothing.
    App::test((), |app| async move {
        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );

            const CHUNK_SIZE: usize = MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK;
            const FAILING_TASKS: usize = 5;
            const TOTAL_BLOCKS: usize = CHUNK_SIZE * 2 + FAILING_TASKS;
            // The last successful task, at the end of the second chunk.
            const LAST_SUCCESSFUL_INDEX: usize = CHUNK_SIZE * 2 - 1;

            let mut new_lines = Vec::with_capacity(TOTAL_BLOCKS);
            let mut offset = CharOffset::from(1);

            for i in 0..TOTAL_BLOCKS {
                if i >= CHUNK_SIZE * 2 {
                    // The entire final chunk fails: an empty CodeBlock has no runs, so no
                    // paragraph is ever pushed and layout_text_block errors instead of
                    // producing a trailing-newline value.
                    new_lines.push(StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![],
                        style: BufferBlockStyle::CodeBlock {
                            code_block_type: CodeBlockType::Shell,
                        },
                        content_length: CharOffset::from(3),
                    }));
                    offset += 3;
                } else if i == LAST_SUCCESSFUL_INDEX {
                    // No trailing newline, so this is distinguishable from the `true` default.
                    new_lines.push(StyledBufferBlock::Text(StyledTextBlock {
                        block: vec![StyledBufferRun {
                            run: "ab".to_string(),
                            text_styles: TextStylesWithMetadata::default(),
                            block_style: BufferBlockStyle::PlainText,
                        }],
                        style: BufferBlockStyle::PlainText,
                        content_length: CharOffset::from(2),
                    }));
                    offset += 2;
                } else {
                    new_lines.push(identifiable_text_block(3));
                    offset += 3;
                }
            }

            let delta = EditDelta {
                old_offset: CharOffset::from(1)..offset,
                new_lines: Arc::new(new_lines),
                ..Default::default()
            };

            let laid_out = delta.layout_delta(
                &text_layout,
                None,
                None,
            );

            // Every task in the final chunk failed and was dropped.
            assert_eq!(laid_out.laid_out_line.len(), TOTAL_BLOCKS - FAILING_TASKS);
            assert!(
                laid_out.trailing_newline.is_none(),
                "trailing newline should come from the last successful task (none), not the default that would result from losing an earlier chunk's result"
            );
        });
    })
}

#[test]
fn test_layout_temporary_blocks_preserves_order_across_chunk_boundary() {
    // layout_temporary_blocks shares chunk_layout_tasks with EditDelta::layout_delta (APP-5392);
    // verify a batch spanning multiple chunks still groups its blocks by destination line in
    // their original order.
    App::test((), |app| async move {
        app.read(|ctx| {
            let text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );

            const TOTAL_BLOCKS: usize = MAX_LAYOUT_TASKS_PER_PARALLEL_CHUNK * 2 + 3;
            let insert_before = LineCount::from(5);

            let blocks: Vec<_> = (0..TOTAL_BLOCKS)
                .map(|i| TemporaryBlock {
                    content: format!("line-{i}\n"),
                    insert_before,
                    line_decoration: None,
                    inline_text_decorations: Vec::new(),
                })
                .collect();

            let mut result = layout_temporary_blocks(blocks, &text_layout);
            let items = result
                .remove(&insert_before)
                .expect("all blocks share the same destination line");

            assert_eq!(items.len(), TOTAL_BLOCKS);
            for item in &items {
                assert!(matches!(item, BlockItem::TemporaryBlock { .. }));
            }
        });
    })
}
