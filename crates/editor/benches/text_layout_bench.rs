use std::hint::black_box;
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use rayon::ThreadPoolBuilder;
use string_offset::CharOffset;
use warp_editor::content::buffer::{StyledBufferBlock, StyledBufferRun, StyledTextBlock};
use warp_editor::content::edit::EditDelta;
use warp_editor::content::text::{BufferBlockStyle, TextStylesWithMetadata};
use warp_editor::render::layout::TextLayout;
use warp_editor::render::model::{InlineCodeStyle, ParagraphStyles, RichTextStyles};
#[cfg(target_os = "macos")]
use warpui::platform::mac::FontDB as MacFontDB;
use warpui_core::App;
use warpui_core::color::ColorU;
use warpui_core::elements::Fill;
#[cfg(target_os = "macos")]
use warpui_core::fonts::Cache as FontCache;
use warpui_core::fonts::{FamilyId, Weight};
use warpui_core::units::IntoPixels;

const BLOCK_COUNT: usize = 4_096;
const RAYON_THREAD_COUNT: usize = 6;
const SHAPING_TEXT: &str = concat!(
    "office affinity efficient waffle: ffi ffl fi fl; ",
    "English left-to-right text surrounds العربية: السَّلَامُ عَلَيْكُمْ ورحمة الله, ",
    "then עברית: שלום עולם and mixed account-42 مرحبا status=ready; ",
    "combining marks: café naïve coöperate Ångström; ",
    "Devanagari: नमस्ते दुनिया; Thai: สวัสดีชาวโลก; ",
    "emoji and joiners: 👩‍💻 👨‍👩‍👧‍👦; "
);

fn benchmark_block(text: String) -> StyledBufferBlock {
    let content_length = text.chars().count();
    StyledBufferBlock::Text(StyledTextBlock {
        block: vec![StyledBufferRun {
            run: text,
            text_styles: TextStylesWithMetadata::default(),
            block_style: BufferBlockStyle::PlainText,
        }],
        style: BufferBlockStyle::PlainText,
        content_length: CharOffset::from(content_length),
    })
}

fn benchmark_styles(font_family: FamilyId) -> RichTextStyles {
    let white = ColorU::white();
    let paragraph = ParagraphStyles {
        font_family,
        font_size: 13.,
        font_weight: Weight::Normal,
        line_height_ratio: 1.2,
        text_color: white,
        baseline_ratio: 0.8,
        fixed_width_tab_size: None,
    };
    RichTextStyles {
        base_text: paragraph,
        placeholder_color: white,
        selection_fill: Fill::None,
        cursor_fill: Fill::None,
        inline_code_style: InlineCodeStyle {
            font_family,
            background: white,
            font_color: white,
        },
        block_spacings: Default::default(),
        minimum_paragraph_height: Some(24.0.into_pixels()),
        show_placeholder_text_on_empty_block: false,
        cursor_width: 1.,
        highlight_urls: false,
    }
}

fn benchmark_delta(texts: impl IntoIterator<Item = String>) -> (EditDelta, usize) {
    let blocks: Vec<_> = texts.into_iter().map(benchmark_block).collect();
    let chars = blocks
        .iter()
        .map(StyledBufferBlock::content_length)
        .map(CharOffset::as_usize)
        .sum();
    (
        EditDelta {
            old_offset: CharOffset::from(1)..CharOffset::from(1 + chars),
            new_lines: Arc::new(blocks),
            ..Default::default()
        },
        chars,
    )
}

fn layout_delta(delta: &EditDelta, text_layout: &TextLayout<'_>) {
    black_box(delta.layout_delta(text_layout, None));
}

fn text_layout_benchmarks(criterion: &mut Criterion) {
    ThreadPoolBuilder::new()
        .num_threads(RAYON_THREAD_COUNT)
        .build_global()
        .expect("benchmark should initialize Rayon before first use");
    let (test_delta, test_chars) = benchmark_delta(
        (0..BLOCK_COUNT).map(|index| format!("{SHAPING_TEXT} test-backend-block-{index:04}\n")),
    );
    let test_styles = benchmark_styles(FamilyId(0));
    #[cfg(target_os = "macos")]
    let (core_text_font_cache, core_text_styles, core_text_delta, core_text_chars) = {
        let mut font_cache = FontCache::new(Box::new(MacFontDB::new()));
        let font_family = font_cache
            .load_system_font("Menlo")
            .expect("Menlo should be available on macOS");
        let (delta, chars) = benchmark_delta(
            (0..BLOCK_COUNT).map(|index| format!("{SHAPING_TEXT} core-text-block-{index:04}\n")),
        );
        (font_cache, benchmark_styles(font_family), delta, chars)
    };
    let mut criterion = std::mem::take(criterion);

    App::test((), move |app| async move {
        app.read(|ctx| {
            let test_text_layout = TextLayout::new(
                ctx.font_cache().text_layout_system(),
                &test_styles,
                f32::MAX,
            );
            layout_delta(&test_delta, &test_text_layout);
            #[cfg(target_os = "macos")]
            {
                let core_text_layout = TextLayout::new(
                    core_text_font_cache.text_layout_system(),
                    &core_text_styles,
                    f32::MAX,
                );
                layout_delta(&core_text_delta, &core_text_layout);
            }
            {
                let mut group = criterion.benchmark_group("editor_text_layout/test_backend");
                group.throughput(Throughput::Elements(test_chars as u64));
                group.bench_function("layout_delta_4096_shaping_blocks_6_threads", |bench| {
                    bench.iter(|| {
                        let text_layout = TextLayout::new(
                            ctx.font_cache().text_layout_system(),
                            &test_styles,
                            f32::MAX,
                        );
                        layout_delta(&test_delta, &text_layout)
                    })
                });
                group.finish();
            }
            #[cfg(target_os = "macos")]
            {
                let mut group = criterion.benchmark_group("editor_text_layout/core_text");
                group.throughput(Throughput::Elements(core_text_chars as u64));
                group.bench_function("layout_delta_4096_shaping_blocks_6_threads", |bench| {
                    bench.iter(|| {
                        let text_layout = TextLayout::new(
                            core_text_font_cache.text_layout_system(),
                            &core_text_styles,
                            f32::MAX,
                        );
                        layout_delta(&core_text_delta, &text_layout)
                    })
                });
                group.finish();
            }
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_secs(2))
        .measurement_time(Duration::from_secs(5));
    targets = text_layout_benchmarks
}
criterion_main!(benches);
