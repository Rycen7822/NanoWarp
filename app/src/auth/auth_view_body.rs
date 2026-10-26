use lazy_static::lazy_static;
use warp_core::ui::appearance::DEFAULT_COMMAND_PALETTE_FONT_SIZE;
use warp_core::ui::builder::UiBuilder;
use warpui::accessibility::{AccessibilityContent, WarpA11yRole};
use warpui::clipboard::ClipboardContent;
use warpui::color::ColorU;
use warpui::elements::{
    Border, Container, CornerRadius, CrossAxisAlignment, Fill, Flex, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, ParentElement, Radius, Stack,
};
use warpui::fonts::Weight;
use warpui::keymap::FixedBinding;
use warpui::ui_components::components::{Coords, UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::AuthStateProvider;
use super::auth_manager::AuthManager;
use super::auth_view_shared_helpers::{action_button_color_and_variant, render_square_logo};
use crate::appearance::Appearance;
use crate::editor::{
    EditorView, InteractionState, SingleLineEditorOptions, TextColors, TextOptions,
};
use crate::experiments::{AuthFlowInstructions, Experiment};
use crate::modal::MODAL_CORNER_RADIUS;

use crate::themes::theme::Fill as ThemeFill;
use crate::util::color::{darken, lighten};

const COMMON_BODY_UI_FONT_SIZE: f32 = 12.;
const AUTH_MODAL_GAP: f32 = 16.;

const AUTH_TOKEN_INPUT_PLACEHOLDER_TEXT: &str = "Auth Token";
const AUTH_TOKEN_INPUT_PLACEHOLDER_TEXT_EXPERIMENTAL: &str = "Browser auth token";

const AUTH_TOKEN_INPUT_BORDER_RADIUS: Radius = Radius::Pixels(4.);

lazy_static! {
    static ref AUTH_TOKEN_INPUT_BACKGROUND: Fill = ColorU::white().into();
    static ref AUTH_TOKEN_INPUT_TEXT_COLOR: ThemeFill = ThemeFill::Solid(ColorU::black());
    static ref AUTH_TOKEN_INPUT_TEXT_DISABLED: ThemeFill =
        AUTH_TOKEN_INPUT_TEXT_COLOR.with_opacity(20);
    static ref AUTH_TOKEN_INPUT_TEXT_HINT: ThemeFill = AUTH_TOKEN_INPUT_TEXT_COLOR.with_opacity(40);
}

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([FixedBinding::new(
        "enter",
        AuthViewBodyAction::Signup,
        id!("AuthViewBody"),
    )]);
    app.register_fixed_bindings([FixedBinding::new(
        "escape",
        AuthViewBodyAction::Close,
        id!("AuthViewBody"),
    )]);
}

#[derive(Default)]
struct MouseStateHandles {
    show_auth_token_input_mouse_state_handle: MouseStateHandle,
    copy_browser_url_mouse_state_handle: MouseStateHandle,
    sign_up_mouse_state_handle: MouseStateHandle,
    close_button_mouse_state_handle: MouseStateHandle,
}

pub struct AuthViewBody {
    mouse_state_handles: MouseStateHandles,
    auth_token_input: ViewHandle<EditorView>,
    show_auth_token_input: bool,
    auth_step: AuthStep,
}

pub enum AuthStep {
    SelectAuthPathway,
    BrowserOpen,
}

#[derive(Clone, Copy, Debug)]
pub enum AuthViewBodyAction {
    EnterToken,
    CopyLoginUrl,
    Signup,
    SignupAnonymousUser,
    Close,
}

impl AuthViewBody {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let experiment_group = AuthFlowInstructions::get_group(ctx);
        let auth_token_input = ctx.add_typed_action_view(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            let mut editor = EditorView::single_line(
                SingleLineEditorOptions {
                    text: TextOptions {
                        font_size_override: Some(COMMON_BODY_UI_FONT_SIZE),
                        font_family_override: Some(appearance.ui_font_family()),
                        text_colors_override: Some(TextColors {
                            default_color: *AUTH_TOKEN_INPUT_TEXT_COLOR,
                            disabled_color: *AUTH_TOKEN_INPUT_TEXT_DISABLED,
                            hint_color: *AUTH_TOKEN_INPUT_TEXT_HINT,
                        }),
                        ..Default::default()
                    },
                    soft_wrap: false,
                    ..Default::default()
                },
                ctx,
            );

            let placeholder_text =
                if matches!(experiment_group, Some(AuthFlowInstructions::Experiment)) {
                    AUTH_TOKEN_INPUT_PLACEHOLDER_TEXT_EXPERIMENTAL
                } else {
                    AUTH_TOKEN_INPUT_PLACEHOLDER_TEXT
                };

            editor.set_placeholder_text(placeholder_text, ctx);
            editor
        });

        ctx.subscribe_to_view(&auth_token_input, |me, _, event, ctx| {
            use crate::editor::Event::{AltEnter, CmdEnter, Enter, Paste, ShiftEnter};
            match event {
                AltEnter | CmdEnter | Enter | Paste | ShiftEnter => me.emit_token_entered(ctx),
                _ => {}
            };
            ctx.notify();
        });

        AuthViewBody {
            mouse_state_handles: Default::default(),
            auth_token_input,
            show_auth_token_input: false,
            auth_step: AuthStep::SelectAuthPathway,
        }
    }

    pub fn handle_paste(&mut self, ctx: &mut ViewContext<Self>) {
        self.show_auth_token_input = true;
        self.auth_token_input
            .update(ctx, |editor, ctx| editor.paste(ctx));
    }

    pub fn reset_login_screen(&mut self, ctx: &mut ViewContext<Self>) {
        self.reset_auth_token_input(ctx);
        self.auth_step = AuthStep::SelectAuthPathway;
    }

    fn reset_auth_token_input(&mut self, ctx: &mut ViewContext<Self>) {
        self.set_input_editable(true, ctx);
        self.auth_token_input
            .update(ctx, |editor, ctx| editor.clear_buffer(ctx));
        self.show_auth_token_input = false;
    }

    pub fn set_input_editable(&mut self, is_editable: bool, ctx: &mut ViewContext<Self>) {
        let interaction_state = match is_editable {
            false => InteractionState::Disabled,
            true => InteractionState::Editable,
        };
        self.auth_token_input.update(ctx, |editor, ctx| {
            editor.set_interaction_state(interaction_state, ctx)
        });
    }

    fn emit_token_entered(&self, ctx: &mut ViewContext<Self>) {
        let text = self.auth_token_input.as_ref(ctx).buffer_text(ctx);
        ctx.emit(AuthViewBodyEvent::AuthTokenEntered(text));
    }

    fn render_auth_token_suggest(&self, ui_builder: &UiBuilder) -> Box<dyn Element> {
        Flex::row()
            .with_child(
                ui_builder
                    .link(
                        "Click here to paste your token from the browser".into(),
                        None,
                        Some(Box::new(|ctx| {
                            ctx.dispatch_typed_action(AuthViewBodyAction::EnterToken);
                        })),
                        self.mouse_state_handles
                            .show_auth_token_input_mouse_state_handle
                            .clone(),
                    )
                    .soft_wrap(false)
                    .build()
                    .finish(),
            )
            .finish()
    }

    fn render_auth_token_input(&self, appearance: &Appearance) -> Option<Box<dyn Element>> {
        if !self.show_auth_token_input {
            return None;
        }

        Some(
            appearance
                .ui_builder()
                .text_input(self.auth_token_input.clone())
                .with_style(UiComponentStyles {
                    background: Some(*AUTH_TOKEN_INPUT_BACKGROUND),
                    border_width: Some(0.),
                    border_radius: Some(CornerRadius::with_all(AUTH_TOKEN_INPUT_BORDER_RADIUS)),
                    padding: Some(Coords {
                        top: 12.,
                        bottom: 12.,
                        left: 16.,
                        right: 16.,
                    }),
                    margin: Some(Coords {
                        top: 8.,
                        bottom: 0.,
                        left: 0.,
                        right: 0.,
                    }),
                    ..Default::default()
                })
                .build()
                .finish(),
        )
    }

    fn render_sign_up_button(
        &self,
        is_anonymous: bool,
        appearance: &Appearance,
        ui_builder: &UiBuilder,
    ) -> Box<dyn Element> {
        let (button_color, button_variant) = action_button_color_and_variant(appearance);
        let button_styles = UiComponentStyles {
            font_size: Some(14.),
            font_family_id: Some(appearance.ui_font_family()),
            font_weight: Some(Weight::Bold),
            background: Some(Fill::Solid(button_color)),
            border_width: Some(2.),
            border_color: Some(Fill::Solid(ColorU::transparent_black())),
            border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
            padding: Some(Coords {
                top: 0.,
                bottom: 0.,
                left: 12., // Unequal padding for optical centering
                right: 8.,
            }),
            height: Some(40.),
            ..Default::default()
        };

        let hover_button_style = UiComponentStyles {
            border_color: Some(Fill::Solid(lighten(button_color))),
            ..button_styles
        };

        let click_button_style = UiComponentStyles {
            background: Some(Fill::Solid(darken(button_color))),
            ..hover_button_style
        };

        let on_click_action = if is_anonymous {
            AuthViewBodyAction::SignupAnonymousUser
        } else {
            AuthViewBodyAction::Signup
        };

        ui_builder
            .button_with_custom_styles(
                button_variant,
                self.mouse_state_handles.sign_up_mouse_state_handle.clone(),
                button_styles,
                Some(hover_button_style),
                Some(click_button_style),
                None,
            )
            .with_centered_text_label("Sign up".into())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(on_click_action);
            })
            .finish()
    }

    fn render_force_login_disclaimer(
        &self,
        appearance: &Appearance,
        ui_builder: &UiBuilder,
    ) -> Box<dyn Element> {
        let disclaimer_color = appearance
            .theme()
            .sub_text_color(appearance.theme().background())
            .into();

        let disclaimer_styles = UiComponentStyles {
            font_color: Some(disclaimer_color),
            ..Default::default()
        };

        let text = "To collaborate with others, please create an account.";

        Container::new(
            ui_builder
                .paragraph(text)
                .with_style(disclaimer_styles)
                .build()
                .finish(),
        )
        .with_margin_bottom(AUTH_MODAL_GAP)
        .finish()
    }

    fn render_header(&self, appearance: &Appearance, ui_builder: &UiBuilder) -> Box<dyn Element> {
        let header_styles = UiComponentStyles {
            font_family_id: Some(appearance.header_font_family()),
            font_color: Some(appearance.theme().active_ui_text_color().into()),
            font_size: Some(20.),
            font_weight: Some(Weight::Semibold),
            ..Default::default()
        };

        let text = "Sign up for Warp";

        ui_builder
            .span(text)
            .with_style(header_styles)
            .build()
            .finish()
    }

    fn render_logo_row(&self, appearance: &Appearance, ui_builder: &UiBuilder) -> Box<dyn Element> {
        let logo = render_square_logo(appearance);
        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_child(logo);

        let close_button = ui_builder
            .close_button(
                24.,
                self.mouse_state_handles
                    .close_button_mouse_state_handle
                    .clone(),
            )
            .build()
            .on_click(|ctx, _, _| ctx.dispatch_typed_action(AuthViewBodyAction::Close))
            .finish();
        row = row.with_child(close_button);

        row.finish()
    }

    fn render_select_auth_pathway_content(
        &self,
        is_anonymous: bool,
        appearance: &Appearance,
        ui_builder: &UiBuilder,
    ) -> Vec<Box<dyn Element>> {
        let logo = Container::new(self.render_logo_row(appearance, ui_builder))
            .with_margin_bottom(AUTH_MODAL_GAP)
            .finish();
        let header = Container::new(self.render_header(appearance, ui_builder))
            .with_margin_bottom(AUTH_MODAL_GAP)
            .finish();
        let sign_up_button = self.render_sign_up_button(is_anonymous, appearance, ui_builder);
        let force_login_disclaimer = self.render_force_login_disclaimer(appearance, ui_builder);
        vec![logo, header, force_login_disclaimer, sign_up_button]
    }

    fn render_browser_open_content(
        &self,
        appearance: &Appearance,
        ui_builder: &UiBuilder,
    ) -> Vec<Box<dyn Element>> {
        let logo = Container::new(self.render_logo_row(appearance, ui_builder))
            .with_margin_bottom(AUTH_MODAL_GAP)
            .finish();

        let header_styles = UiComponentStyles {
            font_family_id: Some(appearance.header_font_family()),
            font_color: Some(appearance.theme().active_ui_text_color().into()),
            font_size: Some(20.),
            font_weight: Some(Weight::Semibold),
            ..Default::default()
        };

        let header = Container::new(
            ui_builder
                .paragraph("Sign in on your browser \nto continue")
                .with_style(header_styles)
                .build()
                .finish(),
        )
        .with_margin_bottom(AUTH_MODAL_GAP)
        .finish();

        let hint = Container::new(
            Flex::column()
                .with_child(
                    Flex::row()
                        .with_child(
                            ui_builder
                                .span("If your browser hasn't launched, ")
                                .build()
                                .finish(),
                        )
                        .with_child(
                            ui_builder
                                .link(
                                    "copy the URL".into(),
                                    None,
                                    Some(Box::new(|event_ctx| {
                                        event_ctx.dispatch_typed_action(
                                            AuthViewBodyAction::CopyLoginUrl,
                                        );
                                    })),
                                    self.mouse_state_handles
                                        .copy_browser_url_mouse_state_handle
                                        .clone(),
                                )
                                .soft_wrap(false)
                                .build()
                                .finish(),
                        )
                        .finish(),
                )
                .with_child(
                    ui_builder
                        .span("and open the page manually.")
                        .build()
                        .finish(),
                )
                .finish(),
        )
        .finish();

        let mut contents = vec![logo, header, hint];

        let auth_token = Container::new(match self.render_auth_token_input(appearance) {
            Some(auth_token_input) => auth_token_input,
            _ => self.render_auth_token_suggest(ui_builder),
        })
        .with_margin_top(AUTH_MODAL_GAP)
        .finish();

        contents.push(auth_token);
        contents
    }

    pub fn set_auth_step(&mut self, step: AuthStep) {
        self.auth_step = step;
    }
}

pub enum AuthViewBodyEvent {
    SignUpButtonClicked,
    AuthTokenEntered(String),
    Close,
}

impl Entity for AuthViewBody {
    type Event = AuthViewBodyEvent;
}

impl TypedActionView for AuthViewBody {
    type Action = AuthViewBodyAction;

    fn handle_action(&mut self, action: &AuthViewBodyAction, ctx: &mut ViewContext<Self>) {
        match action {
            AuthViewBodyAction::EnterToken => {
                self.auth_token_input
                    .update(ctx, |editor, ctx| editor.paste(ctx));
                self.show_auth_token_input = true;

                ctx.notify();
            }
            AuthViewBodyAction::CopyLoginUrl => {
                if AuthStateProvider::as_ref(ctx)
                    .get()
                    .is_user_anonymous()
                    .unwrap_or_default()
                {
                    AuthManager::handle(ctx).update(ctx, |auth_manager, ctx| {
                        auth_manager.copy_anonymous_user_linking_url_to_clipboard(ctx);
                    });
                } else {
                    AuthManager::handle(ctx).update(ctx, |auth_manager, inner_ctx| {
                        let sign_in_url = auth_manager.sign_in_url();
                        inner_ctx.clipboard().write(ClipboardContent {
                            plain_text: sign_in_url.clone(),
                            paths: Some(vec![sign_in_url]),
                            ..Default::default()
                        });
                    });
                }
            }
            AuthViewBodyAction::Signup => {
                self.auth_step = AuthStep::BrowserOpen;

                AuthManager::handle(ctx).update(ctx, |auth_manager, ctx| {
                    let sign_up_url = auth_manager.sign_up_url();
                    ctx.open_url(&sign_up_url);
                });
            }
            AuthViewBodyAction::SignupAnonymousUser => {
                AuthManager::handle(ctx).update(ctx, |auth_manager, ctx| {
                    auth_manager.initiate_anonymous_user_linking(ctx);
                });
                self.auth_step = AuthStep::BrowserOpen;
                ctx.emit(AuthViewBodyEvent::SignUpButtonClicked);
            }
            AuthViewBodyAction::Close => {
                ctx.emit(AuthViewBodyEvent::Close);
            }
        }
    }
}

impl View for AuthViewBody {
    fn ui_name() -> &'static str {
        "AuthViewBody"
    }

    fn accessibility_contents(&self, _: &AppContext) -> Option<AccessibilityContent> {
        Some(AccessibilityContent::new(
            "Welcome to Warp!",
            "Press enter to open your browser to Sign Up or Sign In.",
            WarpA11yRole::HelpRole,
        ))
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.notify();
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let ui_builder = UiBuilder::new(
            appearance.theme().clone(),
            appearance.ui_font_family(),
            COMMON_BODY_UI_FONT_SIZE,
            DEFAULT_COMMAND_PALETTE_FONT_SIZE,
            appearance.line_height_ratio(),
        );

        let is_anonymous = AuthStateProvider::as_ref(app)
            .get()
            .is_user_anonymous()
            .unwrap_or_default();

        let mut content = Flex::column().with_cross_axis_alignment(CrossAxisAlignment::Stretch);
        content = content.with_children(match self.auth_step {
            AuthStep::SelectAuthPathway => {
                self.render_select_auth_pathway_content(is_anonymous, appearance, &ui_builder)
            }
            AuthStep::BrowserOpen => self.render_browser_open_content(appearance, &ui_builder),
        });

        let content = content.finish();

        let mut stack = Stack::new();
        stack.add_child(
            Container::new(content)
                .with_background(appearance.theme().surface_1())
                .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
                .with_corner_radius(CornerRadius::with_all(MODAL_CORNER_RADIUS))
                .with_uniform_padding(32.)
                .finish(),
        );

        stack.finish()
    }
}
