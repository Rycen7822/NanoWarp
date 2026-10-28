use string_offset::CharOffset;
use sum_tree::SumTree;
use warpui_core::units::IntoPixels;

use crate::render::model::location::WrapDirection;
use crate::render::model::test_utils::{TEST_STYLES, laid_out_paragraph};
use crate::render::model::{Location, RenderState};

#[test]
fn test_hit_within_line() {
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 60.0.into_pixels());
    model.set_content(SumTree::from_item(laid_out_paragraph(
        "Hello, world!\n",
        &TEST_STYLES,
        40.,
    )));

    assert_eq!(
        model.render_coordinates_to_location(12.0.into_pixels(), 4.3.into_pixels()),
        Location::Text {
            char_offset: 1.into(),
            clamped: false,
            wrap_direction: WrapDirection::Down,
            block_start: CharOffset::zero(),
            link: None,
        }
    );

    assert_eq!(
        model.render_coordinates_to_location(20.0.into_pixels(), 4.3.into_pixels()),
        Location::Text {
            char_offset: 2.into(),
            clamped: false,
            wrap_direction: WrapDirection::Down,
            block_start: CharOffset::zero(),
            link: None,
        }
    );
}

#[test]
fn test_hit_empty_line() {
    // This is a regression test for CLD-591.
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 42.0.into_pixels());
    let mut tree = SumTree::new();
    tree.extend([
        // Height: 0-32, chars: 0-4
        laid_out_paragraph("1st\n", &TEST_STYLES, 40.0),
        // Height: 32-64, chars: 4-5
        laid_out_paragraph("\n", &TEST_STYLES, 40.0),
        // Height: 64-96, chars: 5-9
        laid_out_paragraph("2nd\n", &TEST_STYLES, 40.0),
    ]);
    model.set_content(tree);

    // A hit on the empty line should clamp to within that line, not the start of the next one.
    assert_eq!(
        model.render_coordinates_to_location(20.0.into_pixels(), 40.0.into_pixels()),
        Location::Text {
            char_offset: 4.into(),
            clamped: true,
            wrap_direction: WrapDirection::Up,
            block_start: CharOffset::from(4),
            link: None,
        }
    );
}

#[test]
fn test_hit_on_soft_wrapped_line() {
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 60.0.into_pixels(), 30.0.into_pixels());
    model.set_content(SumTree::from_item(laid_out_paragraph(
        "Hello, world!\n",
        &TEST_STYLES,
        40., // This is less than the viewport width to account for the 20px of margin.
    )));

    // A hit just after the end of a soft-wrapped line should wrap up.
    assert_eq!(
        model.render_coordinates_to_location(46.0.into_pixels(), 6.0.into_pixels()),
        Location::Text {
            char_offset: 4.into(),
            clamped: true,
            wrap_direction: WrapDirection::Up,
            block_start: CharOffset::zero(),
            link: None,
        }
    );

    // A hit at the start of the next line should have the same char offset, but wrap down.
    assert_eq!(
        model.render_coordinates_to_location(0.0.into_pixels(), 15.0.into_pixels()),
        Location::Text {
            char_offset: 4.into(),
            clamped: false,
            wrap_direction: WrapDirection::Down,
            block_start: CharOffset::zero(),
            link: None,
        }
    );
}

#[test]
fn test_hit_after_end() {
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 50.0.into_pixels(), 60.0.into_pixels());
    model.set_content(SumTree::from_item(laid_out_paragraph(
        "ABCD\n",
        &TEST_STYLES,
        50.,
    )));

    // A hit after the end, but on the same line, is like a soft-wrapped hit.
    assert_eq!(
        model.render_coordinates_to_location(45.0.into_pixels(), 10.0.into_pixels()),
        Location::Text {
            char_offset: 4.into(),
            clamped: true,
            wrap_direction: WrapDirection::Up,
            block_start: CharOffset::zero(),
            link: None,
        }
    );

    // A hit on the line after the last clamps to the end of content, but wraps
    // to the placeholder next line.
    assert_eq!(
        model.render_coordinates_to_location(20.0.into_pixels(), 33.0.into_pixels()),
        Location::Text {
            char_offset: 5.into(),
            clamped: true,
            wrap_direction: WrapDirection::Down,
            block_start: 5.into(),
            link: None,
        }
    );
}

#[test]
fn test_hit_before_start() {
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 60.0.into_pixels());
    model.set_content(SumTree::from_item(laid_out_paragraph(
        "ABCDEFGH\n",
        &TEST_STYLES,
        model.viewport().width().as_f32(),
    )));

    // Hit before the start of the first soft-wrapped line, which should clamp to the first
    // character.
    assert_eq!(
        model.render_coordinates_to_location((-4.).into_pixels(), 10.0.into_pixels()),
        Location::Text {
            char_offset: 0.into(),
            // Not considered clamped, because BlockItem::coordinates_to_location pre-clamps when handling
            // padding.
            clamped: false,
            wrap_direction: WrapDirection::Down,
            block_start: 0.into(),
            link: None,
        }
    );

    // Hit before the start of the second soft-wrapped line, which should clamp to the first
    // character of that line.
    assert_eq!(
        model.render_coordinates_to_location((-4.).into_pixels(), 20.0.into_pixels()),
        Location::Text {
            char_offset: 4.into(),
            clamped: false,
            wrap_direction: WrapDirection::Down,
            block_start: 0.into(),
            link: None
        }
    );
}

#[test]
fn test_hit_scrolled() {
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 42.0.into_pixels());
    let mut tree = SumTree::new();
    tree.extend([
        // Height: 0-32, chars: 0-3
        laid_out_paragraph("1st\n", &TEST_STYLES, 40.0),
        // Height: 32-64, chars: 4-7
        laid_out_paragraph("2nd\n", &TEST_STYLES, 40.0),
        // Height: 64-96, chars: 8-15
        laid_out_paragraph("wrapped\n", &TEST_STYLES, 40.0),
        // Height: 96-128, chars: 16-20
        laid_out_paragraph("last\n", &TEST_STYLES, 40.0),
    ]);
    model.set_content(tree);

    // Scroll the viewport directly since we don't have a ModelContext.
    model.viewport.scroll((-40.0).into_pixels(), model.height());

    // Because of scrolling, this hits the second line.
    assert_eq!(
        model.viewport_coordinates_to_location(22.0.into_pixels(), 0.2.into_pixels()),
        Location::Text {
            char_offset: 6.into(),
            clamped: false,
            wrap_direction: WrapDirection::Down,
            block_start: 4.into(),
            link: None,
        }
    );

    // Now, scroll to the last viewport (containing the last paragraph, part of "ped", and the
    // trailing newline).
    model.viewport.scroll((-47.0).into_pixels(), model.height());

    // This line is soft-wrapped to be partially in-viewport.
    assert_eq!(
        model.viewport_coordinates_to_location(36.0.into_pixels(), 0.5.into_pixels()),
        Location::Text {
            char_offset: 19.into(),
            clamped: false,
            wrap_direction: WrapDirection::Down,
            block_start: 16.into(),
            link: None,
        }
    );

    // We should still be able to hit-test at the last line - accounting for the scroll position,
    // this is the very end of it.
    assert_eq!(
        model.viewport_coordinates_to_location(0.5.into_pixels(), 20.0.into_pixels()),
        Location::Text {
            char_offset: 21.into(),
            clamped: true,
            wrap_direction: WrapDirection::Down,
            block_start: 21.into(),
            link: None,
        }
    );
    // Hits past the last line should resolve to the last character in the buffer. In this case,
    // that's a newline, but in a real editor, it would be the last character of the last line,
    // with a TrailingNewline marker after it.
    assert_eq!(
        model.viewport_coordinates_to_location(2.0.into_pixels(), 42.0.into_pixels()),
        Location::Text {
            char_offset: 21.into(),
            clamped: true,
            wrap_direction: WrapDirection::Down,
            block_start: 21.into(),
            link: None,
        }
    );
}

#[test]
fn test_hit_padding() {
    let mut model =
        RenderState::new_for_test(TEST_STYLES.clone(), 40.0.into_pixels(), 40.0.into_pixels());
    let mut tree = SumTree::new();
    tree.extend([laid_out_paragraph("line\n", &TEST_STYLES, 40.0)]);
    model.set_content(tree);

    // Hit in the padding after the paragraph ends. We should return the character that
    // matches the x-axis pixel position on the last line.
    assert_eq!(
        model.viewport_coordinates_to_location(38.0.into_pixels(), 22.0.into_pixels()),
        Location::Text {
            char_offset: 3.into(),
            clamped: true,
            wrap_direction: WrapDirection::Down,
            block_start: CharOffset::zero(),
            link: None,
        }
    );
}
