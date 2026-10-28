use rangemap::RangeSet;
use string_offset::CharOffset;
use sum_tree::SumTree;
use vec1::{Vec1, vec1};
use warpui_core::geometry::rect::RectF;
use warpui_core::geometry::vector::vec2f;
use warpui_core::units::{IntoPixels, Pixels};

use super::debug::Describe;
use super::test_utils::layout_paragraph;
use super::{BlockItem, BlockLocation, HiddenBlockConfig, ParagraphBlock, RenderState};
use crate::content::text::BufferBlockStyle;
use crate::render::model::test_utils::{TEST_STYLES, laid_out_paragraph, mock_paragraph};
use crate::render::model::{
    Height, LayoutSummary, LineCount, RenderedSelection, SoftWrapPoint, TEXT_SPACING,
};

#[test]
fn test_height() {
    let mut render_state =
        RenderState::new_for_test(TEST_STYLES, 10.0.into_pixels(), 10.0.into_pixels());
    let mut content = SumTree::new();
    // Height: 24
    content.push(mock_paragraph(24., 1., 1));
    // Height: 48
    content.push(mock_paragraph(48., 1., 2));
    // Height: 24
    content.push(mock_paragraph(24., 1., 3));
    // Height: 24
    content.push(mock_paragraph(24., 1., 4));
    // Height: 32
    content.push(mock_paragraph(32., 1., 5));
    render_state.set_content(content);

    // This includes all content plus the trailing newline marker.
    assert_eq!(render_state.height(), 176.0.into_pixels());
    let content = render_state.content.borrow();
    let mut cursor = content.cursor::<Height, Height>();
    // Ensure we can seek in between items for scrolling.
    cursor.seek(&Height::from(64.), sum_tree::SeekBias::Left);
    assert_eq!(
        cursor.item().expect("Seek succeeded").height().as_f32(),
        48.
    );
    assert_eq!(cursor.start().into_pixels().as_f32(), 24.);
    assert_eq!(cursor.end().into_pixels().as_f32(), 72.);

    let end = cursor.slice(&Height::from(152.), sum_tree::SeekBias::Right);
    assert_eq!(
        end.summary(),
        LayoutSummary {
            content_length: 14.into(),
            height: 48. + 24. + 24. + 32.,
            width: (17.).into_pixels(),
            lines: LineCount(4),
        }
    );
}

#[test]
fn test_width() {
    let mut render_state =
        RenderState::new_for_test(TEST_STYLES, 10.0.into_pixels(), 10.0.into_pixels());
    let mut content = SumTree::new();
    // Width 25.
    content.push(mock_paragraph(24., 10., 1));
    // Width: 10.
    content.push(mock_paragraph(48., 25., 2));
    render_state.set_content(content);

    // This includes all content plus the trailing newline marker.
    assert_eq!(render_state.width(), (41.).into_pixels());
    let content = render_state.content.borrow();
    let mut cursor = content.cursor::<Height, Height>();
    let end = cursor.slice(&Height::from(40.), sum_tree::SeekBias::Right);
    assert_eq!(
        end.summary(),
        LayoutSummary {
            content_length: 1.into(),
            height: 24.,
            width: (26.).into_pixels(),
            lines: LineCount(1),
        }
    );
}

#[test]
fn test_soft_wrap_point() {
    /// Helper to convert a character count to a pixel x-offset, accounting for plain-text spacing.
    fn char_x(chars: usize) -> Pixels {
        TEXT_SPACING.left_offset() + (chars as f32 * TEST_STYLES.base_text.font_size).into_pixels()
    }

    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 60.0.into_pixels());
    let mut content = SumTree::new();
    // This paragraph soft-wraps to 2 lines and includes chars 0-7.
    content.push(laid_out_paragraph("ABCDEFG\n", &TEST_STYLES, 40.));
    // This paragraph fits on a single line and includes chars 8-12.
    content.push(laid_out_paragraph("ABCD\n", &TEST_STYLES, 40.));
    // This paragraph soft-wraps to 2 lines and includes chars 13-20.
    content.push(laid_out_paragraph("ABCDEFG\n", &TEST_STYLES, 40.));
    // This line is empty and includes char 21.
    content.push(laid_out_paragraph("\n", &TEST_STYLES, 40.));
    // This paragraph fits on a single line and includes chars 22-25.
    content.push(laid_out_paragraph("ABC\n", &TEST_STYLES, 40.));
    assert_eq!(content.extent::<CharOffset>(), CharOffset::from(26));
    assert_eq!(content.extent::<LineCount>().as_usize(), 7);
    model.set_content(content);

    // Last point on the first softwrapped line.
    assert_eq!(
        model.offset_to_softwrap_point(CharOffset::from(3)),
        SoftWrapPoint::new(0, char_x(3))
    );

    // A point slightly closer to 2 than 3 should round to 2.
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(0, char_x(2) + 4.0.into_pixels())),
        CharOffset::from(2)
    );

    // A point slightly closer to 3 than 2 should round to 3.
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(0, char_x(3) - 4.0.into_pixels())),
        CharOffset::from(3)
    );

    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(0, char_x(4))),
        CharOffset::from(4)
    );

    // Point on the second softwrapped line in the first paragraph.
    assert_eq!(
        model.offset_to_softwrap_point(CharOffset::from(7)),
        SoftWrapPoint::new(1, char_x(3))
    );
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(1, char_x(3))),
        CharOffset::from(7)
    );

    // Non-softwrapped line should work as well.
    assert_eq!(
        model.offset_to_softwrap_point(CharOffset::from(10)),
        SoftWrapPoint::new(2, char_x(2))
    );
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(2, char_x(2))),
        CharOffset::from(10)
    );

    assert_eq!(
        model.offset_to_softwrap_point(CharOffset::from(19)),
        SoftWrapPoint::new(4, char_x(2))
    );
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(4, char_x(2))),
        CharOffset::from(19)
    );

    // Softwrapping on an empty line should work.
    assert_eq!(
        model.offset_to_softwrap_point(CharOffset::from(21)),
        SoftWrapPoint::new(5, TEXT_SPACING.left_offset())
    );
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(5, Pixels::zero())),
        CharOffset::from(21)
    );

    // Out of bound points should be bounded to the trailing newline.
    assert_eq!(
        model.offset_to_softwrap_point(CharOffset::from(40)),
        SoftWrapPoint::new(8, Pixels::zero())
    );
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(7, Pixels::zero())),
        CharOffset::from(26)
    );

    // Points are bounded to their line's contents.
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(5, char_x(3))),
        CharOffset::from(21)
    );
    assert_eq!(
        model.softwrap_point_to_offset(SoftWrapPoint::new(5, char_x(2))),
        CharOffset::from(21)
    );
}

#[test]
fn test_character_bounds() {
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 60.0.into_pixels());
    let mut content = SumTree::new();
    // This paragraph soft-wraps to 2 lines and includes chars 0-7.
    content.push(laid_out_paragraph(
        "ABCDEFG\n",
        &TEST_STYLES,
        model.viewport.width().as_f32(),
    ));
    // This paragraph soft-wraps to 2 lines and includes chars 8-14.
    content.push(laid_out_paragraph(
        "HIJKLMN\n",
        &TEST_STYLES,
        model.viewport.width().as_f32(),
    ));
    model.set_content(content);

    // Due to the minimum block height, there is 2px of top spacing.

    let char_size = vec2f(10., 10.);

    // The middle of the first line.
    assert_eq!(
        model.character_bounds(2.into()),
        Some(RectF::new(vec2f(20., 2.), char_size))
    );

    // The first character of the second soft-wrapped line.
    assert_eq!(
        model.character_bounds(4.into()),
        Some(RectF::new(vec2f(0., 12.), char_size))
    );

    // The middle of the first line of the second paragraph.
    assert_eq!(
        model.character_bounds(9.into()),
        Some(RectF::new(vec2f(10., 26.), char_size))
    );

    // The end of the first line of the second paragraph.
    assert_eq!(
        model.character_bounds(11.into()),
        Some(RectF::new(vec2f(30., 26.), char_size))
    );

    // The middle of the second line of the second paragraph.
    assert_eq!(
        model.character_bounds(13.into()),
        Some(RectF::new(vec2f(10., 36.), char_size))
    );
}

#[test]
fn test_non_empty_content_can_hide_final_trailing_newline() {
    let mut model = RenderState::new_for_test(
        TEST_STYLES.clone(),
        100.0.into_pixels(),
        200.0.into_pixels(),
    );
    model.set_show_final_trailing_newline_when_non_empty(false);

    let mut content = SumTree::new();
    content.push(laid_out_paragraph(
        "First\n",
        &TEST_STYLES,
        model.viewport.width().as_f32(),
    ));
    model.set_content(content);

    assert_eq!(model.blocks(), 1);
    assert_eq!(model.height(), 24.0.into_pixels());
}

#[test]
fn test_empty_content_keeps_final_trailing_newline_when_suppressed() {
    let mut model = RenderState::new_for_test(
        TEST_STYLES.clone(),
        100.0.into_pixels(),
        200.0.into_pixels(),
    );
    model.set_show_final_trailing_newline_when_non_empty(false);

    assert_eq!(model.blocks(), 1);
    assert_eq!(model.height(), 24.0.into_pixels());
}

#[test]
fn test_first_line_bounds() {
    let mut model = RenderState::new_for_test(
        TEST_STYLES.clone(),
        100.0.into_pixels(),
        200.0.into_pixels(),
    );
    let mut content = SumTree::new();
    content.push(laid_out_paragraph(
        "This is a soft-wrapped paragraph\n",
        &TEST_STYLES,
        model.viewport.width().as_f32(),
    ));
    model.set_content(content);
    let content = model.content();
    let text_block = content
        .block_at_offset(CharOffset::zero())
        .expect("Paragraph should exist");
    assert_eq!(
        text_block.first_line_bounds().expect("Bounds should exist"),
        RectF::new(vec2f(0., 0.), vec2f(100., 10.))
    );
    assert_eq!(text_block.item.height().as_f32(), 40.);
    let trailing_block = content
        .block_at_offset(CharOffset::from(33))
        .expect("Trailing cursor should exist");
    assert_eq!(
        trailing_block
            .first_line_bounds()
            .expect("Bounds should exist"),
        RectF::new(vec2f(0., 47.), vec2f(1., 10.))
    );
}

#[test]
fn test_scroll_snapshot() {
    // Lay out the content at the current viewport width.
    fn layout_content(model: &mut RenderState) {
        let mut content = SumTree::new();
        content.push(laid_out_paragraph(
            "AAAABBBBCCCC\n",
            &TEST_STYLES,
            model.viewport().width().as_f32(),
        ));
        content.push(laid_out_paragraph(
            "DDDDEEEEFFFFGGGG\n",
            &TEST_STYLES,
            model.viewport().width().as_f32(),
        ));
        model.set_content(content);
    }

    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 60.0.into_pixels());
    layout_content(&mut model);

    let content = model.content();
    // Verify the height of each block. Each text paragraph has 10px per soft-wrapped line with a
    // 24px minimum height. The trailing newline block is 24px high.
    assert_eq!(
        content
            .block_at_offset(CharOffset::zero())
            .expect("Block should exist")
            .item
            .height()
            .as_f32(),
        30.
    );
    assert_eq!(
        content
            .block_at_offset(13.into())
            .expect("Block should exist")
            .item
            .height()
            .as_f32(),
        40.
    );
    assert_eq!(
        content
            .block_at_offset(30.into())
            .expect("Block should exist")
            .item
            .height()
            .as_f32(),
        24.
    );
    drop(content);

    // Scroll so that the EEEE line is at the top of the viewport.
    model.viewport.scroll((-44.).into_pixels(), model.height());
    let scroll_position = model.snapshot_scroll_position();
    assert_eq!(scroll_position.first_character_offset(), 13.into());

    // Now, double the viewport width, halving the number of soft-wrapped lines.
    model
        .viewport
        .set_size(vec2f(80., 60.), model.width(), model.height());

    // At first, the content will not have been laid out again, so the scroll position is
    // unaffected.
    assert_eq!(model.viewport.scroll_top(), 34.0.into_pixels());
    // After laying out again, each block is exactly 24px high (the two soft-wrapped blocks are
    // below the minimum height otherwise).
    layout_content(&mut model);
    assert_eq!(model.height().as_f32(), 24. * 3.);

    // Restore the scroll position at the new height. It should still start at the same content.
    assert!(
        model
            .viewport
            .scroll_to(scroll_position.to_scroll_top(&model), model.height())
    );
    // The reduced content height clamps the restored position to the last viewport.
    assert_eq!(model.viewport.scroll_top().as_f32(), 12.);

    // Halve the original viewport width, leading to twice as many soft-wrapped lines.
    model
        .viewport
        .set_size(vec2f(20., 60.), model.width(), model.height());
    layout_content(&mut model);
    assert_eq!(model.height().as_f32(), 60. + 80. + 24.);

    // Restore the scroll position at the new height.
    assert!(
        model
            .viewport
            .scroll_to(scroll_position.to_scroll_top(&model), model.height())
    );
    // The new scroll position is at the start of the second paragraph.
    assert_eq!(model.viewport.scroll_top().as_f32(), 60.);
}

#[test]
fn test_offset_in_active_selection() {
    let render_state =
        RenderState::new_for_test(TEST_STYLES, 10.0.into_pixels(), 10.0.into_pixels());
    let selection_vec: Vec1<RenderedSelection> = vec1![
        RenderedSelection::new(2.into(), 4.into()),
        RenderedSelection::new(6.into(), 8.into()),
        RenderedSelection::new(12.into(), 10.into())
    ];
    let selections = selection_vec.into();
    *render_state.selections.borrow_mut() = selections;

    assert!(render_state.offset_in_active_selection(3.into()));
    assert!(!render_state.offset_in_active_selection(1.into()));
    assert!(render_state.offset_in_active_selection(7.into()));
    assert!(!render_state.offset_in_active_selection(9.into()));
    assert!(!render_state.offset_in_active_selection(2.into()));
    assert!(render_state.offset_in_active_selection(4.into()));
    assert!(!render_state.offset_in_active_selection(10.into()));
    assert!(render_state.offset_in_active_selection(12.into()));
    assert!(render_state.offset_in_active_selection(11.into()));
}

#[test]
fn test_is_selection_head() {
    let render_state =
        RenderState::new_for_test(TEST_STYLES, 10.0.into_pixels(), 10.0.into_pixels());
    let selection_vec: Vec1<RenderedSelection> = vec1![
        RenderedSelection::new(2.into(), 4.into()),
        RenderedSelection::new(6.into(), 8.into()),
        RenderedSelection::new(12.into(), 10.into())
    ];
    let selections = selection_vec.into();
    *render_state.selections.borrow_mut() = selections;

    assert!(render_state.is_selection_head(2.into()));
    assert!(!render_state.is_selection_head(1.into()));
    assert!(!render_state.is_selection_head(4.into()));
    assert!(render_state.is_selection_head(6.into()));
    assert!(render_state.is_selection_head(12.into()));
}

#[test]
fn test_multiselect_autoscroll_bounding_box() {
    // Test that the computation for the autoscroll bounding box work correctly.
    let view_height = 800.0.into_pixels();

    // One selection, on screen.
    assert_eq!(
        RenderState::multiselect_autoscroll_bounding_box(
            vec1![(vec2f(0., 0.), vec2f(0., 0.))],
            view_height,
            0.0.into_pixels(),
        ),
        (vec2f(0., 0.), vec2f(0., 0.))
    );

    // One selection, on screen.
    assert_eq!(
        RenderState::multiselect_autoscroll_bounding_box(
            vec1![(vec2f(100., 100.), vec2f(100., 100.))],
            view_height,
            0.0.into_pixels(),
        ),
        (vec2f(100., 100.), vec2f(100., 100.))
    );

    // Two selections, on screen.
    assert_eq!(
        RenderState::multiselect_autoscroll_bounding_box(
            vec1![
                (vec2f(100., 100.), vec2f(100.0, 100.0)),
                (vec2f(200., 200.), vec2f(200., 200.))
            ],
            view_height,
            0.0.into_pixels(),
        ),
        (vec2f(100., 100.), vec2f(200., 200.))
    );

    // Three selections, top two on screen, but the third one is too far to fit.
    // Pick a selection that isn't larger than the viewport
    assert_eq!(
        RenderState::multiselect_autoscroll_bounding_box(
            vec1![
                (vec2f(100., 100.), vec2f(100.0, 100.0)),
                (vec2f(200., 200.), vec2f(200., 200.)),
                (vec2f(300., 1000.), vec2f(300., 1000.))
            ],
            view_height,
            0.0.into_pixels(),
        ),
        (vec2f(100., 100.), vec2f(200., 200.))
    );

    // Three selections, one on screen, so the other two should not be scrolled to.
    // Pick a selection that isn't larger than the viewport
    assert_eq!(
        RenderState::multiselect_autoscroll_bounding_box(
            vec1![
                (vec2f(100., 700.), vec2f(100.0, 700.0)),
                (vec2f(200., 900.), vec2f(200., 900.)),
                (vec2f(300., 1000.), vec2f(300., 1000.))
            ],
            view_height,
            0.0.into_pixels(),
        ),
        (vec2f(100., 700.), vec2f(100., 700.))
    );

    // Three selections, all off screen to the bottom, so we should fit as many as we can.
    assert_eq!(
        RenderState::multiselect_autoscroll_bounding_box(
            vec1![
                (vec2f(100., 1000.), vec2f(100.0, 1000.0)),
                (vec2f(200., 1400.), vec2f(200., 1400.)),
                (vec2f(300., 1900.), vec2f(300., 1900.))
            ],
            view_height,
            0.0.into_pixels(),
        ),
        (vec2f(100., 1000.), vec2f(200., 1400.))
    );

    // Three selections, all off screen to the top, so we should fit as many as we can from the bottom up.
    assert_eq!(
        RenderState::multiselect_autoscroll_bounding_box(
            vec1![
                (vec2f(100., 0.), vec2f(100.0, 0.0)),
                (vec2f(200., 500.), vec2f(200., 500.)),
                (vec2f(300., 1200.), vec2f(300., 1200.))
            ],
            view_height,
            1500.0.into_pixels(),
        ),
        (vec2f(200., 500.), vec2f(300., 1200.))
    );
}

// 18:09:15 [INFO] [warp_editor::render::model] Initial tree:
// -------- 0.00px / 0 characters --------
// Hidden (3067 characters, 87 lines, 20.00px tall)
// -------- 20.00px / 3067 characters --------
// Paragraph (32 characters, 1 lines, 18.20px tall)
// -------- 38.20px / 3099 characters --------
// Paragraph (28 characters, 1 lines, 18.20px tall)
// -------- 56.40px / 3127 characters --------
// Paragraph (28 characters, 1 lines, 18.20px tall)
// -------- 74.60px / 3155 characters --------
// Paragraph (37 characters, 1 lines, 18.20px tall)
// -------- 92.80px / 3192 characters --------
// Paragraph (13 characters, 1 lines, 18.20px tall)
// -------- 111.00px / 3205 characters --------
// Paragraph (6 characters, 1 lines, 18.20px tall)
// -------- 129.20px / 3211 characters --------
// Paragraph (2 characters, 1 lines, 18.20px tall)
// -------- 147.40px / 3213 characters --------
// Hidden (406 characters, 15 lines, 20.00px tall)
// -------- 167.40px / 3619 characters --------
// Paragraph (41 characters, 1 lines, 18.20px tall)
// -------- 185.60px / 3660 characters --------
// Paragraph (73 characters, 1 lines, 18.20px tall)
// -------- 203.80px / 3733 characters --------
// Paragraph (57 characters, 1 lines, 18.20px tall)
// -------- 222.00px / 3790 characters --------
// Paragraph (17 characters, 1 lines, 18.20px tall)
// -------- 240.20px / 3807 characters --------
// Paragraph (36 characters, 1 lines, 18.20px tall)
// -------- 258.40px / 3843 characters --------
// Paragraph (29 characters, 1 lines, 18.20px tall)
// -------- 276.60px / 3872 characters --------
// Temporary Paragraph (0 characters, 0 lines, 18.20px tall)
// -------- 294.80px / 3872 characters --------
// Temporary Paragraph (0 characters, 0 lines, 18.20px tall)
// -------- 313.00px / 3872 characters --------
// Paragraph (10 characters, 1 lines, 18.20px tall)
// -------- 331.20px / 3882 characters --------
// Paragraph (6 characters, 1 lines, 18.20px tall)
// -------- 349.40px / 3888 characters --------
// Hidden (1 characters, 1 lines, 20.00px tall)
//
// Nothing needs to be changed here. There is no overlapping hidden ranges.
#[test]
fn test_dedupe_hidden_ranges_logged_tree_is_unchanged() {
    // This is a "golden" structure derived from the logs in the prompt.
    // The observed behavior was that `dedupe_hidden_ranges` is a no-op for this tree.

    let mut tree = SumTree::new();

    tree.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(87),
        CharOffset::from(3066),
        BlockLocation::Start,
    )));

    for len in [32usize, 28, 28, 37, 13, 6, 2] {
        tree.push(mock_paragraph(18.2, 0., len));
    }

    tree.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(15),
        CharOffset::from(406),
        BlockLocation::Middle,
    )));

    for len in [41usize, 73, 57, 17, 36, 29] {
        tree.push(mock_paragraph(18.2, 0., len));
    }

    let temporary_paragraph =
        layout_paragraph("\n", &TEST_STYLES, &BufferBlockStyle::PlainText, 80.);
    let temporary_block = BlockItem::TemporaryBlock {
        paragraph_block: ParagraphBlock::new(vec1![temporary_paragraph]),
        text_decoration: Vec::new(),
        decoration: None,
    };
    tree.push(temporary_block.clone());
    tree.push(temporary_block);

    for len in [10usize, 6] {
        tree.push(mock_paragraph(18.2, 0., len));
    }

    tree.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(1),
        CharOffset::from(1),
        BlockLocation::End,
    )));

    let mut hidden_ranges = RangeSet::new();
    hidden_ranges.insert(CharOffset::from(1)..CharOffset::from(3067));
    hidden_ranges.insert(CharOffset::from(3213)..CharOffset::from(3619));
    hidden_ranges.insert(CharOffset::from(3888)..CharOffset::from(3889));

    let initial = tree.describe().to_string();
    let resulting = RenderState::dedupe_hidden_ranges(tree, hidden_ranges)
        .describe()
        .to_string();

    assert_eq!(initial, resulting);
}

// 18:09:14 [INFO] [warp_editor::render::model] Initial tree:
// -------- 0.00px / 0 characters --------
// Hidden (3066 characters, 87 lines, 20.00px tall)
// -------- 20.00px / 3067 characters --------
// Paragraph (32 characters, 1 lines, 18.20px tall)
// -------- 38.20px / 3099 characters --------
// Paragraph (28 characters, 1 lines, 18.20px tall)
// -------- 56.40px / 3127 characters --------
// Paragraph (28 characters, 1 lines, 18.20px tall)
// -------- 74.60px / 3155 characters --------
// Paragraph (37 characters, 1 lines, 18.20px tall)
// -------- 92.80px / 3192 characters --------
// Paragraph (13 characters, 1 lines, 18.20px tall)
// -------- 111.00px / 3205 characters --------
// Paragraph (6 characters, 1 lines, 18.20px tall)
// -------- 129.20px / 3211 characters --------
// Paragraph (2 characters, 1 lines, 18.20px tall)
// -------- 147.40px / 3213 characters --------
// Hidden (406 characters, 15 lines, 20.00px tall)
// -------- 167.40px / 3619 characters --------
// Paragraph (41 characters, 1 lines, 18.20px tall)
// -------- 185.60px / 3660 characters --------
// Paragraph (73 characters, 1 lines, 18.20px tall)
// -------- 203.80px / 3733 characters --------
// Paragraph (57 characters, 1 lines, 18.20px tall)
// -------- 222.00px / 3790 characters --------
// Paragraph (17 characters, 1 lines, 18.20px tall)
// -------- 240.20px / 3807 characters --------
// Paragraph (36 characters, 1 lines, 18.20px tall)
// -------- 258.40px / 3843 characters --------
// Paragraph (29 characters, 1 lines, 18.20px tall)
// -------- 276.60px / 3872 characters --------
// Hidden (1 characters, 1 lines, 20.00px tall)
// -------- 296.60px / 3873 characters --------
// Hidden (1944 characters, 45 lines, 20.00px tall)
//
// The last two hidden sections should be collapsed.
#[test]
fn test_dedupe_hidden_ranges_merges_adjacent_hidden_blocks() {
    let mut tree = SumTree::new();

    // Pushing a hidden range that actually exceed what is expected from the canonical range.
    tree.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(87),
        CharOffset::from(3067),
        BlockLocation::Start,
    )));

    for len in [32usize, 28, 28, 37, 13, 6, 2] {
        tree.push(mock_paragraph(18.2, 0., len));
    }

    tree.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(15),
        CharOffset::from(406),
        BlockLocation::Middle,
    )));

    for len in [41usize, 73, 57, 17, 36, 29] {
        tree.push(mock_paragraph(18.2, 0., len));
    }

    // Two adjacent hidden blocks.
    tree.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(1),
        CharOffset::from(1),
        BlockLocation::Middle,
    )));
    tree.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(45),
        CharOffset::from(1944),
        BlockLocation::End,
    )));

    let mut hidden_ranges = RangeSet::new();
    hidden_ranges.insert(CharOffset::from(1)..CharOffset::from(3067));
    hidden_ranges.insert(CharOffset::from(3213)..CharOffset::from(3619));

    // Covers both adjacent hidden blocks (3872 + 1 + 1944 = 5817 total content length).
    hidden_ranges.insert(CharOffset::from(3872)..CharOffset::from(5818));

    let resulting = RenderState::dedupe_hidden_ranges(tree, hidden_ranges);

    let mut expected = SumTree::new();

    expected.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(87),
        CharOffset::from(3066),
        BlockLocation::Start,
    )));

    for len in [32usize, 28, 28, 37, 13, 6, 2] {
        expected.push(mock_paragraph(18.2, 0., len));
    }

    expected.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(15),
        CharOffset::from(406),
        BlockLocation::Middle,
    )));

    for len in [41usize, 73, 57, 17, 36, 29] {
        expected.push(mock_paragraph(18.2, 0., len));
    }

    expected.push(BlockItem::Hidden(HiddenBlockConfig::new(
        LineCount(46),
        CharOffset::from(1946),
        BlockLocation::End,
    )));

    assert_eq!(
        expected.describe().to_string(),
        resulting.describe().to_string()
    );
}

#[test]
fn test_layout_delta_block_location_is_global_across_chunk_boundaries() {
    use std::sync::Arc;

    use warpui_core::App;

    use crate::content::buffer::{StyledBufferBlock, StyledBufferRun, StyledTextBlock};
    use crate::content::edit::EditDelta;
    use crate::content::text::{BufferBlockStyle, TextStylesWithMetadata};
    use crate::render::layout::TextLayout;
    App::test((), |app| async move {
        app.read(|ctx| {
            let layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &TEST_STYLES,
                f32::MAX,
            );
            // 256 begins a chunk (the existing limit is 64 tasks); each real line occupies one task.
            // Large hidden runs occur at the start, in the middle, across that boundary, and at the end.
            let mut lines = Vec::with_capacity(600);
            for _ in 0..600 {
                lines.push(StyledBufferBlock::Text(StyledTextBlock {
                    block: vec![StyledBufferRun {
                        run: "a\n".into(),
                        text_styles: TextStylesWithMetadata::default(),
                        block_style: BufferBlockStyle::PlainText,
                    }],
                    style: BufferBlockStyle::PlainText,
                    content_length: 2.into(),
                }));
            }
            let mut hidden = RangeSet::new();
            for range in [1..61, 101..161, 501..561, 1141..1201] {
                hidden.insert(CharOffset::from(range.start)..CharOffset::from(range.end));
            }
            let delta = EditDelta {
                old_offset: 1.into()..1201.into(),
                new_lines: Arc::new(lines),
                ..Default::default()
            };
            let laid_out = delta.layout_delta(&layout, Some(hidden.clone()));
            let mut tree = SumTree::new();
            tree.extend(laid_out.laid_out_line);
            let tree = RenderState::dedupe_hidden_ranges(tree, hidden);
            let hidden: Vec<_> = tree
                .cursor::<(), ()>()
                .filter_map(|item| match item {
                    BlockItem::Hidden(config) => Some(config),
                    _ => None,
                })
                .collect();
            assert_eq!(hidden.len(), 4);
            assert_eq!(hidden[0].line_count(), LineCount::from(30));
            assert_eq!(hidden[1].line_count(), LineCount::from(30));
            assert_eq!(hidden[2].line_count(), LineCount::from(30));
            assert_eq!(hidden[3].line_count(), LineCount::from(30));
            assert_eq!(hidden[0].height(), 20.0.into_pixels());
            assert_eq!(hidden[1].height(), 40.0.into_pixels());
            assert_eq!(hidden[2].height(), 40.0.into_pixels());
            assert_eq!(hidden[3].height(), 20.0.into_pixels());
        });
    });
}
