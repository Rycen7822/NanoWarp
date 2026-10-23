use std::borrow::Cow;
use std::boxed::Box;
use std::cell::RefCell;
use std::rc::Rc;

use pathfinder_color::ColorU;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::color::contrast::{
    MinimumAllowedContrast, foreground_color_with_minimum_contrast,
};
use warp_core::ui::color::{Opacity, Rgb, coloru_with_opacity};
use warp_core::ui::theme;
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    ChildView, Clipped, Container, CornerRadius, CrossAxisAlignment, Fill, Flex, MainAxisAlignment,
    MainAxisSize, ParentElement, Radius, Rect, Shrinkable,
};
use warpui::ui_components::components::UiComponentStyles;
use warpui::ui_components::segmented_control::{
    RenderableOptionConfig, SegmentedControl, SegmentedControlEvent, TooltipConfig,
};
use warpui::{
    AppContext, Element, Entity, EntityId, ModelHandle, SingletonEntity as _, TypedActionView,
    View, ViewContext, ViewHandle,
};

use crate::BlocklistAIHistoryModel;
use crate::ai::AIRequestUsageModel;
use crate::ai::blocklist::block::cli_controller::CLISubagentController;
use crate::ai::blocklist::prompt::PromptIconButtonTheme;
use crate::ai::blocklist::prompt::prompt_alert::{PromptAlertEvent, PromptAlertView};
use crate::ai::blocklist::{BlocklistAIHistoryEvent, BlocklistAIInputModel, InputType};
use crate::network::NetworkStatus;
#[cfg(not(target_family = "wasm"))]
use crate::settings::{AISettings, AISettingsChangedEvent};
use crate::terminal::keys::TerminalKeybindings;
use crate::terminal::shared_session::permissions_manager::SessionPermissionsManager;
use crate::ui_components::icons::Icon;
use crate::view_components::action_button::{
    ActionButton, ActionButtonTheme, ButtonSize, NakedTheme, TooltipAlignment,
};
use crate::workspaces::user_workspaces::UserWorkspaces;

const BLURRED_OPACITY: Opacity = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputToggleMode {
    Terminal,
    AgentMode,
    AutoDetection,
}

/// Custom disabled theme for UDI buttons that preserves background but changes font color
struct UDIDisabledButtonTheme;

impl ActionButtonTheme for UDIDisabledButtonTheme {
    fn background(&self, hovered: bool, appearance: &Appearance) -> Option<theme::Fill> {
        // Use the same background as the enabled state
        NakedTheme.background(hovered, appearance)
    }

    fn text_color(
        &self,
        _hovered: bool,
        _background: Option<theme::Fill>,
        appearance: &Appearance,
    ) -> ColorU {
        internal_colors::text_disabled(appearance.theme(), appearance.theme().surface_1())
    }
}

impl From<&BlocklistAIInputModel> for InputToggleMode {
    fn from(input_model: &BlocklistAIInputModel) -> Self {
        if input_model.is_input_type_locked() {
            match input_model.input_type() {
                InputType::Shell => InputToggleMode::Terminal,
                InputType::AI => InputToggleMode::AgentMode,
            }
        } else {
            InputToggleMode::AutoDetection
        }
    }
}

/// Denormalized state that is required to render UDI styles that isn't easily directly accessible
/// because its owned by other views in other parts of the hierarchy and was never extracted to a
/// Model.
///
/// Kind of an 80/20 situation here.
struct CachedUIState {
    is_input_empty: bool,
    is_hovered: bool,
    is_in_active_terminal: bool,
}

impl CachedUIState {
    fn is_button_bar_blurred(&self) -> bool {
        !self.is_hovered && !self.is_in_active_terminal
    }
}

pub struct UniversalDeveloperInputButtonBar {
    terminal_view_id: EntityId,
    file_button: ViewHandle<ActionButton>,
    segmented_control: ViewHandle<SegmentedControl<InputToggleMode>>,
    prompt_alert: ViewHandle<PromptAlertView>,

    cached_ui_state: Rc<RefCell<CachedUIState>>,
    terminal_model: std::sync::Arc<parking_lot::FairMutex<crate::terminal::TerminalModel>>,
}

#[derive(Debug, Clone)]
pub enum UniversalDeveloperInputButtonBarAction {
    SelectFile,
}

pub enum UniversalDeveloperInputButtonBarEvent {
    InputTypeSelected(InputType),
    EnableAutoDetection,
    SelectFile,
    PromptAlert(PromptAlertEvent),
}

impl UniversalDeveloperInputButtonBar {
    pub fn new(
        terminal_view_id: EntityId,
        input_model: ModelHandle<BlocklistAIInputModel>,
        cli_subagent_controller: ModelHandle<CLISubagentController>,
        terminal_model: std::sync::Arc<parking_lot::FairMutex<crate::terminal::TerminalModel>>,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let button_size = ButtonSize::UDIButton;

        let file_button_view = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", PromptIconButtonTheme::new(false))
                .with_icon(Icon::Plus)
                .with_tooltip("Attach file")
                .with_size(button_size)
                .with_disabled_theme(UDIDisabledButtonTheme)
                .with_tooltip_alignment(TooltipAlignment::Left)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(UniversalDeveloperInputButtonBarAction::SelectFile);
                })
        });

        // Create segmented control options based on auto-detection setting
        let ai_settings = AISettings::as_ref(ctx);
        let is_autodetection_enabled = ai_settings.is_ai_autodetection_enabled(ctx);

        let mut options = vec![InputToggleMode::Terminal, InputToggleMode::AgentMode];

        let mut default_option = input_model.as_ref(ctx).into();
        if is_autodetection_enabled {
            options.push(InputToggleMode::AutoDetection);
        } else if default_option == InputToggleMode::AutoDetection {
            // Don't set the default to auto-detection if it's not enabled.
            default_option = InputToggleMode::Terminal;
        }

        let cached_ui_state = Rc::new(RefCell::new(CachedUIState {
            is_input_empty: true,
            is_hovered: false,
            is_in_active_terminal: false,
        }));

        let ui_state_clone = cached_ui_state.clone();
        let input_model_clone = input_model.clone();
        let segmented_control_view = ctx.add_typed_action_view(|ctx| {
            SegmentedControl::new(
                options,
                move |option, is_selected, app| {
                    let ui_state = ui_state_clone.borrow();
                    build_renderable_option_config(
                        option,
                        is_selected,
                        &input_model_clone,
                        &ui_state,
                        app,
                    )
                },
                default_option,
                segmented_control_styles(ctx),
            )
        });
        // Subscribe to segmented control events
        ctx.subscribe_to_view(&segmented_control_view, |_, _, event, ctx| match event {
            SegmentedControlEvent::OptionSelected(input_mode) => match input_mode {
                InputToggleMode::Terminal => {
                    ctx.emit(UniversalDeveloperInputButtonBarEvent::InputTypeSelected(
                        InputType::Shell,
                    ));
                }
                InputToggleMode::AgentMode => {
                    ctx.emit(UniversalDeveloperInputButtonBarEvent::InputTypeSelected(
                        InputType::AI,
                    ));
                }
                InputToggleMode::AutoDetection => {
                    ctx.emit(UniversalDeveloperInputButtonBarEvent::EnableAutoDetection);
                }
            },
        });

        ctx.subscribe_to_model(&Appearance::handle(ctx), |me, _, _, ctx| {
            me.segmented_control.update(ctx, |segmented_control, ctx| {
                segmented_control.set_styles(segmented_control_styles(ctx), ctx);
            });
            ctx.notify();
        });

        ctx.subscribe_to_model(&AISettings::handle(ctx), |me, ai_settings, event, ctx| {
            if let AISettingsChangedEvent::AIAutoDetectionEnabled { .. } = event {
                let is_autodection_enabled =
                    ai_settings.as_ref(ctx).is_ai_autodetection_enabled(ctx);
                me.segmented_control.update(ctx, |segmented_control, ctx| {
                    if is_autodection_enabled {
                        segmented_control.update_options(
                            vec![
                                InputToggleMode::Terminal,
                                InputToggleMode::AgentMode,
                                InputToggleMode::AutoDetection,
                            ],
                            ctx,
                        );
                    } else {
                        segmented_control.update_options(
                            vec![InputToggleMode::Terminal, InputToggleMode::AgentMode],
                            ctx,
                        );
                    }
                });
            }
            ctx.notify();
        });

        let prompt_alert = ctx.add_typed_action_view(PromptAlertView::new);
        ctx.subscribe_to_view(&prompt_alert, |_, _, event, ctx| {
            ctx.emit(UniversalDeveloperInputButtonBarEvent::PromptAlert(
                event.clone(),
            ));
        });

        ctx.subscribe_to_model(&NetworkStatus::handle(ctx), |_, _, _, ctx| {
            ctx.notify();
        });
        ctx.subscribe_to_model(&UserWorkspaces::handle(ctx), |_, _, _, ctx| {
            ctx.notify();
        });
        ctx.subscribe_to_model(&AIRequestUsageModel::handle(ctx), |_, _, _, ctx| {
            ctx.notify()
        });

        ctx.subscribe_to_model(
            &BlocklistAIHistoryModel::handle(ctx),
            |me, _, event, ctx| {
                if event
                    .terminal_surface_id()
                    .is_some_and(|id| id != me.terminal_view_id)
                {
                    return;
                }

                match event {
                    BlocklistAIHistoryEvent::StartedNewConversation { .. }
                    | BlocklistAIHistoryEvent::SetActiveConversation { .. }
                    | BlocklistAIHistoryEvent::UpdatedTodoList { .. }
                    | BlocklistAIHistoryEvent::UpdatedConversationStatus { .. } => {
                        ctx.notify();
                    }
                    _ => (),
                }
            },
        );

        ctx.subscribe_to_model(&input_model, move |me, input_model, event, ctx| {
            if !event.did_update_input_config() {
                return;
            }
            let input_mode = InputToggleMode::from(input_model.as_ref(ctx));
            me.segmented_control.update(ctx, |control, ctx| {
                control.set_selected_option(input_mode, ctx);
            });
            me.notify_and_notify_children(ctx);
        });

        // Keep the control disabled state in sync with role changes
        ctx.subscribe_to_model(&SessionPermissionsManager::handle(ctx), |me, _, _, ctx| {
            me.update_segmented_control_disabled_state(ctx);
        });
        // Keep the control disabled state in sync with agent control state
        ctx.subscribe_to_model(&cli_subagent_controller, move |me, _, _, ctx| {
            me.update_segmented_control_disabled_state(ctx);
        });

        let mut me = Self {
            terminal_view_id,
            file_button: file_button_view,
            segmented_control: segmented_control_view,
            prompt_alert,
            cached_ui_state,
            terminal_model,
        };

        // Initialize disabled state based on current role
        me.update_segmented_control_disabled_state(ctx);

        me
    }

    /// Update the input empty state and refresh the autodetection label
    pub fn update_input_empty_state(&mut self, is_empty: bool, ctx: &mut ViewContext<Self>) {
        if self.cached_ui_state.borrow().is_input_empty == is_empty {
            return;
        }
        self.cached_ui_state.borrow_mut().is_input_empty = is_empty;
        self.notify_and_notify_children(ctx);
    }

    fn notify_and_notify_children(&self, ctx: &mut ViewContext<Self>) {
        ctx.notify();
        self.segmented_control.update(ctx, |_, ctx| ctx.notify());
    }

    pub fn update_segmented_control_disabled_state(&mut self, ctx: &mut ViewContext<Self>) {
        let (is_reader, is_agent_in_control) = {
            let terminal_model = self.terminal_model.lock();
            (
                terminal_model.shared_session_status().is_reader(),
                terminal_model
                    .block_list()
                    .active_block()
                    .is_active_and_long_running(),
            )
        };

        let tooltip = if is_reader {
            Some("Request edit access to change input mode".to_string())
        } else if is_agent_in_control {
            Some("Input mode locked while agent is monitoring a command".to_string())
        } else {
            None
        };

        self.segmented_control
            .update(ctx, |segmented_control, ctx| {
                segmented_control.set_disabled_tooltip(tooltip.clone().map(Into::into), ctx);
            });
    }

    pub fn set_is_in_active_terminal(
        &mut self,
        is_in_active_terminal: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        if self.cached_ui_state.borrow().is_in_active_terminal == is_in_active_terminal {
            return;
        }

        self.cached_ui_state.borrow_mut().is_in_active_terminal = is_in_active_terminal;
        self.update_button_bar_styles(ctx);
        self.notify_and_notify_children(ctx);
    }

    /// Set the parent UDI input hovered state.
    /// Used to fade the button bar when the input is not in focus.
    pub fn set_udi_hovered(&mut self, is_hovered: bool, ctx: &mut ViewContext<Self>) {
        if self.cached_ui_state.borrow().is_hovered == is_hovered {
            return;
        }
        self.cached_ui_state.borrow_mut().is_hovered = is_hovered;

        self.update_button_bar_styles(ctx);
        self.notify_and_notify_children(ctx);
    }

    fn update_button_bar_styles(&self, ctx: &mut ViewContext<Self>) {
        self.update_icon_button_themes(ctx);
    }

    /// Update the themes of the icon buttons to reflect the blurred state
    fn update_icon_button_themes(&self, ctx: &mut ViewContext<Self>) {
        let is_blurred = self.cached_ui_state.borrow().is_button_bar_blurred();
        let theme = PromptIconButtonTheme::new(is_blurred);

        self.file_button.update(ctx, |button, ctx| {
            button.set_theme(theme.clone(), ctx);
        });
    }
}

// Implement Entity trait for UniversalDeveloperInputButtonBar
impl Entity for UniversalDeveloperInputButtonBar {
    type Event = UniversalDeveloperInputButtonBarEvent;
}

// Implement View trait for UniversalDeveloperInputButtonBar
impl View for UniversalDeveloperInputButtonBar {
    fn ui_name() -> &'static str {
        "UniversalDeveloperInputButtonBar"
    }

    fn render(&self, app: &AppContext) -> Box<dyn warpui::Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        // Helper function to create a 1px vertical divider
        let create_divider = || {
            Container::new(
                warpui::elements::ConstrainedBox::new(
                    Rect::new().with_background(theme.surface_3()).finish(),
                )
                .with_width(1.0)
                .with_height(20.0) // Match button height approximately
                .finish(),
            )
            .with_margin_left(4.0)
            .with_margin_right(4.0)
            .finish()
        };

        let content = {
            // Create a horizontal layout with buttons arranged in a row
            let mut buttons = Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_alignment(MainAxisAlignment::Start)
                .with_child(
                    Container::new(ChildView::new(&self.segmented_control).finish())
                        .with_padding_right(4.0)
                        .finish(),
                );
            buttons = buttons.with_child(create_divider());

            // Viewers cannot attach files in shared sessions at this point.
            if !self
                .terminal_model
                .lock()
                .shared_session_status()
                .is_viewer()
            {
                buttons = buttons.with_child(ChildView::new(&self.file_button).finish());
            }

            if !self.prompt_alert.as_ref(app).is_no_alert() {
                buttons = buttons.with_child(
                    Shrinkable::new(
                        1.,
                        Clipped::new(ChildView::new(&self.prompt_alert).finish()).finish(),
                    )
                    .finish(),
                );
            }

            buttons.finish()
        };

        Container::new(Clipped::new(content).finish())
            .with_padding_bottom(12.0)
            .with_padding_right(8.0)
            .finish()
    }
}

impl TypedActionView for UniversalDeveloperInputButtonBar {
    type Action = UniversalDeveloperInputButtonBarAction;

    fn handle_action(
        &mut self,
        action: &UniversalDeveloperInputButtonBarAction,
        ctx: &mut ViewContext<Self>,
    ) {
        match action {
            UniversalDeveloperInputButtonBarAction::SelectFile => {
                ctx.emit(UniversalDeveloperInputButtonBarEvent::SelectFile);
            }
        }
    }
}

fn segmented_control_styles(app: &AppContext) -> UiComponentStyles {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let button_size = 5.0 + appearance.monospace_font_size();
    // Use a smaller base font size that scales properly
    let base_font_size = 10.0; // Start with smaller font
    let scaled_ui_font_size = base_font_size * appearance.monospace_ui_scalar();

    let background = Some(internal_colors::fg_overlay_1(theme).into());

    UiComponentStyles {
        width: Some(button_size), // Match InputPrompt button height for square buttons
        height: Some(button_size), // Match InputPrompt button height
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.0))),
        border_width: Some(1.0),
        border_color: Some(Fill::Solid(theme.outline().into_solid())),
        font_family_id: Some(appearance.ui_font_family()),
        font_size: Some(scaled_ui_font_size),
        background,
        ..Default::default()
    }
}

fn build_renderable_option_config(
    option: InputToggleMode,
    is_selected: bool,
    input_model: &ModelHandle<BlocklistAIInputModel>,
    ui_state: &CachedUIState,
    app: &AppContext,
) -> Option<RenderableOptionConfig> {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();

    let input_model = input_model.as_ref(app);
    let current_input_type = input_model.input_type();

    let terminal_keybindings = TerminalKeybindings::as_ref(app);

    let compute_colors = |input_type, accent_color| {
        let fg_color = if is_selected {
            theme.background().into_solid()
        } else if input_type == current_input_type && !ui_state.is_input_empty {
            accent_color
        } else {
            theme.sub_text_color(theme.surface_1()).into_solid()
        };

        let bg_color = if is_selected {
            accent_color
        } else {
            ColorU::transparent_black()
        };

        (fg_color, bg_color)
    };

    let mut config = match option {
        InputToggleMode::Terminal => {
            let accent_color = theme.terminal_colors().normal.blue.into();
            let (fg_color, bg_color) = compute_colors(InputType::Shell, accent_color);

            RenderableOptionConfig {
                icon_path: Icon::Terminal.into(),
                icon_color: fg_color,
                label: None,
                tooltip: Some(tooltip_config(
                    "Terminal",
                    Some(terminal_mode_tooltip_subtext(terminal_keybindings)),
                    app,
                )),
                background: bg_color.into(),
            }
        }
        InputToggleMode::AgentMode => {
            let accent_color = theme.terminal_colors().normal.yellow.into();
            let (fg_color, bg_color) = compute_colors(InputType::AI, accent_color);

            RenderableOptionConfig {
                icon_path: Icon::AgentMode.into(),
                icon_color: fg_color,
                label: None,
                tooltip: Some(tooltip_config(
                    "Agent Mode",
                    Some(agent_mode_tooltip_subtext(terminal_keybindings)),
                    app,
                )),
                background: bg_color.into(),
            }
        }
        InputToggleMode::AutoDetection => {
            // Should not actually render anything, when using the new two-option
            // UDI control.
            return None;
        }
    };

    if ui_state.is_button_bar_blurred() {
        config.background = Fill::Solid(coloru_with_opacity(
            config.background.start_color(),
            BLURRED_OPACITY,
        ));
        config.icon_color = coloru_with_opacity(config.icon_color, BLURRED_OPACITY);

        if let Some(tooltip_config) = config.tooltip.as_mut() {
            tooltip_config.background_color =
                coloru_with_opacity(tooltip_config.background_color, BLURRED_OPACITY);
            tooltip_config.border_color =
                coloru_with_opacity(tooltip_config.background_color, BLURRED_OPACITY);
            // Shift text color to ensure sufficient contrast against the blurred background
            tooltip_config.text_color = foreground_color_with_minimum_contrast(
                tooltip_config.text_color,
                Rgb::from(tooltip_config.background_color),
                MinimumAllowedContrast::Text,
            );
        }
    }

    Some(config)
}

const AGENT_MODE_TOOLTIP_PREFIX: &str = "* + space";
const TERMINAL_MODE_TOOLTIP_PREFIX: &str = "! + space";

fn agent_mode_tooltip_subtext(terminal_keybindings: &TerminalKeybindings) -> String {
    let keybinding = terminal_keybindings.set_input_mode_agent_keybinding();
    let Some(keybinding) = keybinding else {
        return AGENT_MODE_TOOLTIP_PREFIX.into();
    };

    format!("{keybinding} or {AGENT_MODE_TOOLTIP_PREFIX}")
}

fn terminal_mode_tooltip_subtext(terminal_keybindings: &TerminalKeybindings) -> String {
    let keybinding = terminal_keybindings.set_input_mode_terminal_keybinding();
    let Some(keybinding) = keybinding else {
        return TERMINAL_MODE_TOOLTIP_PREFIX.into();
    };

    format!("{keybinding} or {TERMINAL_MODE_TOOLTIP_PREFIX}")
}

fn tooltip_config(
    text: impl Into<Cow<'static, str>>,
    subtext: Option<impl Into<Cow<'static, str>>>,
    app: &AppContext,
) -> TooltipConfig {
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();

    TooltipConfig {
        text: text.into(),
        sub_text: subtext.map(Into::into),
        background_color: theme.tooltip_background(),
        // Match UiBuilder::default_tool_tip_styles text color (same as agent picker tooltip)
        text_color: theme.background().into_solid(),
        border_color: theme.outline().into_solid(),
    }
}
