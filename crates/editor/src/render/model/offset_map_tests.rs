use string_offset::CharOffset;

use super::{OffsetMap, SelectableTextRun};
use crate::render::model::FrameOffset;

#[test]
fn test_offset_map_basic() {
    // Baseline test for a no-placeholder OffsetMap. The content_start is non-zero to mimic
    // paragraphs within a code block.
    let map = OffsetMap::new(vec![SelectableTextRun {
        content_start: CharOffset::from(12),
        frame_start: FrameOffset::zero(),
        length: 10,
    }]);

    // The returned offset should be adjusted by the content start.
    assert_eq!(map.to_content(FrameOffset::from(4)), 16.into());
    // Mapping should clamp to run bounds.
    assert_eq!(map.to_content(FrameOffset::from(12)), 22.into());
}

#[test]
fn test_offset_map_placeholders() {
    // Set up an offset map for the following structure:
    //           |placeholder|text|placeholder|text|placeholder...
    // Frame:    0           6    14          24   28
    // Content:  0           1     9          10   14
    let map = OffsetMap::new(vec![
        SelectableTextRun {
            // Even in the zero-state placeholder case, there's an empty content run just before it.
            content_start: CharOffset::zero(),
            frame_start: FrameOffset::zero(),
            length: 0,
        },
        SelectableTextRun {
            content_start: CharOffset::from(1),
            frame_start: FrameOffset::from(6),
            length: 8,
        },
        SelectableTextRun {
            content_start: CharOffset::from(10),
            frame_start: FrameOffset::from(24),
            length: 4,
        },
    ]);

    // Depending on what they're closer to, characters at the start of the frame map to either
    // the start of the line or the first content run.
    assert_eq!(map.to_content(FrameOffset::from(2)), CharOffset::zero());
    assert_eq!(map.to_content(FrameOffset::from(4)), CharOffset::from(1));
    assert_eq!(map.to_frame(CharOffset::zero()), FrameOffset::zero());
    assert_eq!(map.to_frame(CharOffset::from(1)), FrameOffset::from(6));

    // Characters within the first text range map within the range.
    assert_eq!(map.to_content(FrameOffset::from(7)), CharOffset::from(2));
    assert_eq!(map.to_frame(CharOffset::from(2)), FrameOffset::from(7));

    // Characters within the second placeholder map to the closer run.
    assert_eq!(map.to_content(FrameOffset::from(16)), CharOffset::from(9));
    assert_eq!(map.to_content(FrameOffset::from(20)), CharOffset::from(10));
    assert_eq!(map.to_frame(CharOffset::from(9)), FrameOffset::from(14));
    assert_eq!(map.to_frame(CharOffset::from(10)), FrameOffset::from(24));

    // Characters in the last placeholder map to the end of the last text run.
    assert_eq!(map.to_content(FrameOffset::from(28)), CharOffset::from(14));
    assert_eq!(map.to_content(FrameOffset::from(50)), CharOffset::from(14));
    assert_eq!(map.to_frame(CharOffset::from(14)), FrameOffset::from(28));
}
