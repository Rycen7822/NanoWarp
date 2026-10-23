use warpui::elements::{Flex, MainAxisSize, ParentElement};
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Element, Entity, SingletonEntity, View};

use super::style;
use crate::appearance::Appearance;

const SESSION_LIMIT_REACHED_MESSAGE: &str =
    "Session sharing has reached its limit for this workspace.";

pub struct DeniedBody;

impl Entity for DeniedBody {
    type Event = ();
}

impl View for DeniedBody {
    fn ui_name() -> &'static str {
        "ShareModalDeniedBody"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let text = appearance
            .ui_builder()
            .wrappable_text(SESSION_LIMIT_REACHED_MESSAGE, true)
            .with_style(style::subheader_styles(appearance))
            .build()
            .finish();
        Flex::column()
            .with_child(text)
            .with_main_axis_size(MainAxisSize::Min)
            .finish()
    }
}
