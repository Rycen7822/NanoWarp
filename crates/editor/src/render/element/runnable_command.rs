use warpui_core::AppContext;
use warpui_core::elements::{CornerRadius, Radius};

use super::{RenderContext, RenderableBlock};
use crate::extract_block;
use crate::render::model::viewport::ViewportItem;
use crate::render::model::{BlockItem, RenderState};

/// [`RenderableBlock`] implementation for runnable command blocks.
pub struct RenderableRunnableCommand {
    viewport_item: ViewportItem,
}

impl RenderableRunnableCommand {
    pub fn new(viewport_item: ViewportItem) -> Self {
        Self { viewport_item }
    }
}

impl RenderableBlock for RenderableRunnableCommand {
    fn viewport_item(&self) -> &ViewportItem {
        &self.viewport_item
    }

    fn layout(
        &mut self,
        _model: &RenderState,
        _ctx: &mut warpui_core::LayoutContext,
        _app: &AppContext,
    ) {
    }

    fn paint(&mut self, model: &RenderState, ctx: &mut RenderContext, _app: &AppContext) {
        let content = model.content();
        let code_block = extract_block!(self.viewport_item, content, (block, BlockItem::RunnableCodeBlock{paragraph_block, ..}) => block.code_block(paragraph_block));

        let styles = model.styles();
        let code_style = &styles.code_text;

        let background_rect = self.viewport_item.visible_bounds(ctx);

        ctx.paint
            .scene
            .draw_rect_without_hit_recording(background_rect)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(8.)))
            .with_border(styles.code_border)
            .with_background(model.styles().code_background);

        for paragraph in code_block.paragraphs() {
            ctx.draw_paragraph(&paragraph, code_style, model);
        }
    }
}
