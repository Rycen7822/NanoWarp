use warpui_core::fonts::Weight;

use super::{BufferTextStyle, TextStyles};

#[test]
fn test_text_style_xor() {
    // This test makes sure that the `TextStyles` XOR implementations are updated as we add new styles.
    for style in enum_iterator::all::<BufferTextStyle>() {
        let mut with_style = TextStyles::default();

        match style {
            BufferTextStyle::Weight(weight) => {
                with_style.set_weight(Weight::from_custom_weight(Some(weight)));
            }
            style => {
                if let Some(style_mut) = with_style.style_mut(&style) {
                    *style_mut = true;
                } else {
                    panic!("Impossible code path -- style {style:?} not handled");
                }
            }
        }

        assert!(
            (with_style ^ TextStyles::default()).colliding_style(&style),
            "Set ^ Unset = Set failed for {style:?}"
        );
        assert!(
            (TextStyles::default() ^ with_style).colliding_style(&style),
            "Unset ^ Set = Set failed for {style:?}"
        );
        assert!(
            !(with_style ^ with_style).colliding_style(&style),
            "Set ^ Set = Unset failed for {style:?}"
        );
        assert!(
            !(TextStyles::default() ^ TextStyles::default()).colliding_style(&style),
            "Unset ^ Unset = Unset failed for {style:?}"
        );

        let mut editable = with_style;

        editable ^= with_style;
        assert!(
            !editable.colliding_style(&style),
            "Set ^= Set -> Unset failed for {style:?}"
        );

        editable ^= TextStyles::default();
        assert!(
            !editable.colliding_style(&style),
            "Unset ^= Unset -> Unset failed for {style:?}"
        );

        editable ^= with_style;
        assert!(
            editable.colliding_style(&style),
            "Unset ^= Set -> Set failed for {style:?}"
        );

        editable ^= TextStyles::default();
        assert!(
            editable.colliding_style(&style),
            "Set ^= Unset -> Set failed for {style:?}"
        );
    }
}
