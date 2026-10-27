use warpui::elements::{
    Align, ClippedScrollStateHandle, ClippedScrollable, Container, Element, Empty, Fill, Flex,
    Hoverable, MainAxisSize, MouseStateHandle, ParentElement, Shrinkable,
};
use warpui::platform::Cursor;
use warpui::presenter::ChildView;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Entity, EntityId, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle, WindowId,
};

use super::section_views::feature_section::FeatureSectionEvent;
use super::section_views::{
    BUTTON_PADDING, DETAIL_FONT_SIZE, FOOTER_ICON_SIZE, SCROLLBAR_OFFSET, SCROLLBAR_WIDTH,
    SECTION_SPACING,
};
use super::sections::sections;
use super::{FeatureSection, FeatureSectionData, FeatureSectionView, TipsCompleted};
use crate::appearance::Appearance;
use crate::resource_center::skip_tips_and_write_to_user_defaults;

use crate::workspace::WorkspaceAction;

#[derive(Default)]
struct MouseStateHandles {
    copy_version: MouseStateHandle,
    skip_tips: MouseStateHandle,
}

pub enum ResourceCenterMainEvent {
    Close,
}

pub struct ResourceCenterMainView {
    button_mouse_states: MouseStateHandles,
    clipped_scroll_state: ClippedScrollStateHandle,
    section_views: Vec<ViewHandle<FeatureSectionView>>,
    tips_completed: ModelHandle<TipsCompleted>,
}

#[derive(Debug, Clone)]
pub enum ResourceCenterMainAction {
    Close,
    SkipTips,
}

impl ResourceCenterMainView {
    pub fn new(ctx: &mut ViewContext<Self>, tips_completed: ModelHandle<TipsCompleted>) -> Self {
        let action_target = ctx.add_model(|_| ActionTarget::None);
        let section_views =
            Self::initialize_section_views(tips_completed.clone(), action_target.clone(), ctx);
        Self {
            button_mouse_states: Default::default(),
            clipped_scroll_state: Default::default(),
            section_views,
            tips_completed,
        }
    }

    fn initialize_section_views(
        tips_completed: ModelHandle<TipsCompleted>,
        action_target: ModelHandle<ActionTarget>,
        ctx: &mut ViewContext<Self>,
    ) -> Vec<ViewHandle<FeatureSectionView>> {
        let sections = sections(ctx);
        let gamified_tips_count = sections.iter().map(|section| section.items.len()).sum();
        tips_completed.update(ctx, |tips_completed, _ctx| {
            tips_completed.set_gamified_tips_count(gamified_tips_count);
        });
        let is_onboarded = sections.iter().any(|section| {
            section.section_name == FeatureSection::GettingStarted
                && section.is_section_completed(tips_completed.as_ref(ctx))
        });
        sections
            .iter()
            .map(|data| {
                let is_tips_completed = tips_completed.as_ref(ctx).skipped_or_completed;
                let is_expanded = match data.section_name {
                    FeatureSection::GettingStarted => !is_tips_completed && !is_onboarded,
                    FeatureSection::MaximizeWarp => is_tips_completed || is_onboarded,
                };
                Self::build_feature_section_view(
                    data,
                    action_target.clone(),
                    ctx,
                    tips_completed.clone(),
                    true,
                    is_expanded,
                )
            })
            .collect()
    }

    fn build_feature_section_view(
        section_data: &FeatureSectionData,
        action_target: ModelHandle<ActionTarget>,
        ctx: &mut ViewContext<ResourceCenterMainView>,
        tips_completed: ModelHandle<TipsCompleted>,
        show_tips_progress: bool,
        is_expanded: bool,
    ) -> ViewHandle<FeatureSectionView> {
        let feature_section_view = ctx.add_typed_action_view(|ctx| {
            FeatureSectionView::new(
                section_data.clone(),
                action_target,
                ctx,
                tips_completed.clone(),
                show_tips_progress,
                is_expanded,
            )
        });

        ctx.subscribe_to_view(&feature_section_view, move |me, _, event, ctx| {
            me.handle_feature_section_event(event, ctx);
        });

        feature_section_view
    }

    fn handle_feature_section_event(
        &mut self,
        event: &FeatureSectionEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            FeatureSectionEvent::CloseResourceCenter => {
                ctx.emit(ResourceCenterMainEvent::Close);
                ctx.notify();
            }
            FeatureSectionEvent::ExpandSection(section_name) => {
                for section_view in &self.section_views {
                    if section_view.as_ref(ctx).feature_section_data.section_name == *section_name {
                        section_view.update(ctx, |view, ctx| view.expand_section(ctx));
                    }
                }
                ctx.notify();
            }
        }
    }

    pub fn set_action_target(
        &mut self,
        window_id: WindowId,
        input_id: Option<EntityId>,
        ctx: &mut ViewContext<Self>,
    ) {
        for section_view in &self.section_views {
            section_view.update(ctx, |view, ctx| {
                view.set_action_target(window_id, input_id, ctx)
            });
        }
    }

    fn render_body(&self, appearance: &Appearance) -> Box<dyn Element> {
        let mut body = Flex::column();

        for section_view in &self.section_views {
            body.add_child(ChildView::new(section_view).finish());
        }

        let theme = appearance.theme();

        ClippedScrollable::vertical(
            self.clipped_scroll_state.clone(),
            body.finish(),
            SCROLLBAR_WIDTH,
            theme.disabled_text_color(theme.background()).into(),
            theme.main_text_color(theme.background()).into(),
            Fill::None,
        )
        .finish()
    }

    fn render_current_version(&self, appearance: &Appearance) -> Box<dyn Element> {
        let version = warp_cli::version_string();

        let style = UiComponentStyles {
            font_color: Some(appearance.theme().nonactive_ui_text_color().into()),
            ..Default::default()
        };

        let text = appearance
            .ui_builder()
            .wrappable_text(version, true)
            .with_style(style)
            .build()
            .finish();

        let copy_icon = appearance
            .ui_builder()
            .copy_button(
                FOOTER_ICON_SIZE,
                self.button_mouse_states.copy_version.clone(),
            )
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::CopyVersion(version))
            })
            .finish();

        Container::new(
            Flex::row()
                .with_child(Shrinkable::new(1., Align::new(text).left().finish()).finish())
                .with_child(Shrinkable::new(0.2, Align::new(copy_icon).finish()).finish())
                .with_main_axis_size(MainAxisSize::Max)
                .finish(),
        )
        .with_margin_left(SECTION_SPACING)
        .with_margin_bottom(BUTTON_PADDING)
        .with_uniform_padding(BUTTON_PADDING)
        .finish()
    }

    fn render_skip_tips_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        Container::new(
            Align::new(
                Hoverable::new(self.button_mouse_states.skip_tips.clone(), |state| {
                    let text_color = if state.is_hovered() {
                        appearance.theme().active_ui_text_color().into_solid()
                    } else {
                        appearance.theme().nonactive_ui_text_color().into_solid()
                    };

                    let style = UiComponentStyles {
                        font_size: Some(DETAIL_FONT_SIZE),
                        font_color: Some(text_color),
                        ..Default::default()
                    };

                    appearance
                        .ui_builder()
                        .wrappable_text("Mark all as read", false)
                        .with_style(style)
                        .build()
                        .finish()
                })
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ResourceCenterMainAction::SkipTips)
                })
                .with_cursor(Cursor::PointingHand)
                .finish(),
            )
            .right()
            .finish(),
        )
        .with_margin_bottom(SECTION_SPACING)
        .with_margin_right(SCROLLBAR_OFFSET + SECTION_SPACING)
        .finish()
    }
}

/// A model for tracking where the events from the resource center view should be dispatched
///
/// Similar to command palette - we need a model to cache the information of where
/// we should send the actions from the resource center features. When the resource center is opened,
/// we cache the current active window ID as well as the input ID of the active
/// tab/pane. By sending all the actions to the input view, we ensure that
/// they propagate correctly. This propagation assumes that each feature action
/// must be in the responder chain. If an action is not in the responder chain
/// (such as a block navigation action) then it won't propagate correctly.
pub enum ActionTarget {
    None,
    View {
        window_id: WindowId,
        input_id: Option<EntityId>,
    },
}

impl Entity for ActionTarget {
    type Event = ();
}

impl Entity for ResourceCenterMainView {
    type Event = ResourceCenterMainEvent;
}

impl TypedActionView for ResourceCenterMainView {
    type Action = ResourceCenterMainAction;

    fn handle_action(&mut self, action: &ResourceCenterMainAction, ctx: &mut ViewContext<Self>) {
        use ResourceCenterMainAction::*;
        match action {
            Close => {
                ctx.notify();
                ctx.emit(ResourceCenterMainEvent::Close);
            }
            SkipTips => {
                self.tips_completed.update(ctx, |tips_completed, ctx| {
                    skip_tips_and_write_to_user_defaults(tips_completed, ctx);
                    ctx.notify();
                });
            }
        }
    }
}

impl View for ResourceCenterMainView {
    fn ui_name() -> &'static str {
        "ResourceCenterMain"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let body = self.render_body(appearance);
        let skip_tips = self.render_skip_tips_button(appearance);

        let mut main_page = Flex::column();

        if !self.tips_completed.as_ref(app).skipped_or_completed {
            main_page.add_child(skip_tips);
        }

        main_page = main_page
            .with_child(Shrinkable::new(20., body).finish())
            .with_child(Shrinkable::new(0.1, Empty::new().finish()).finish()); // placeholder to ensure pane extends to bottom of the window

        let current_version = self.render_current_version(appearance);
        main_page.add_child(current_version);

        main_page.finish()
    }
}
