use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::code::persisted_workspace::WorkspaceMetadata;
use lsp::supported_servers::LSPServerType;
use lsp::{LspManagerModel, LspManagerModelEvent, LspServerModel, LspState};
use pathfinder_color::ColorU;
use warp_core::ui::theme::AnsiColorIdentifier;
use warp_util::path::user_friendly_path;
use warpui::elements::{
    Container, CornerRadius, CrossAxisAlignment, Element, Expanded, Fill, Flex, MainAxisAlignment,
    MainAxisSize, MouseStateHandle, ParentElement, Radius, Shrinkable,
};
use warpui::fonts::Weight;
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::ui_components::switch::SwitchStateHandle;
use warpui::{
    AppContext, Entity, ModelHandle, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::SettingsSection;
use super::settings_page::{
    MatchData, PageType, SettingsPageMeta, SettingsPageViewHandle, SettingsWidget,
};
use crate::appearance::Appearance;
use crate::code::lsp_telemetry::{LspControlActionType, LspEnablementSource, LspTelemetryEvent};
use crate::code::persisted_workspace::{
    EnablementState, LspRepoStatus, PersistedWorkspace, PersistedWorkspaceEvent,
};
use crate::send_telemetry_from_ctx;
use crate::ui_components::avatar::{Avatar, AvatarContent, StatusElementTypes};
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;

const MAIN_SECTION_MARGIN: f32 = 12.;
const SUB_SECTION_MARGIN: f32 = 8.;

const LSP_STATUS_INDICATOR_SIZE: f32 = 8.;
const PAGE_TITLE: &str = "Projects";
#[derive(Clone, Default)]
struct LspServerRowMouseStates {
    restart: MouseStateHandle,
    #[cfg_attr(target_family = "wasm", allow(dead_code))]
    view_logs: MouseStateHandle,
    toggle: SwitchStateHandle,
    install: MouseStateHandle,
}

#[derive(Clone)]
struct InitializedFoldersMouseStates {
    lsp_rows: Vec<LspServerRowMouseStates>,
}

pub struct ProjectsPageView {
    page: PageType<Self>,
    /// Mouse states for LSP server row buttons.
    /// The states are flattened into a single Vec, indexed by iterating through workspaces
    /// and their enabled servers in order.
    lsp_row_mouse_states: Vec<LspServerRowMouseStates>,
    /// Tracks installation status for suggested LSP servers so the UI can decide
    /// whether to show "Available for download" vs "Installed" and whether the
    /// "+" button should trigger install or just enable.
    suggested_server_statuses: HashMap<(PathBuf, LSPServerType), LspRepoStatus>,
}

impl ProjectsPageView {
    pub fn new(ctx: &mut ViewContext<ProjectsPageView>) -> Self {
        // Calculate total LSP server count across all workspaces (enabled + disabled + suggested)
        let lsp_server_count = PersistedWorkspace::as_ref(ctx).total_lsp_server_count(true);

        // Subscribe to LSP manager events for real-time status updates
        ctx.subscribe_to_model(
            &LspManagerModel::handle(ctx),
            |me, _, event, ctx| match event {
                LspManagerModelEvent::ServerStarted(_)
                | LspManagerModelEvent::ServerStopped(_)
                | LspManagerModelEvent::ServerRemoved { .. } => {
                    // Recalculate LSP server count and resize mouse states if needed
                    let new_count = PersistedWorkspace::as_ref(ctx).total_lsp_server_count(true);
                    if me.lsp_row_mouse_states.len() != new_count {
                        me.lsp_row_mouse_states
                            .resize_with(new_count, Default::default);
                    }

                    ctx.notify();
                }
            },
        );

        // Subscribe to PersistedWorkspaceEvent to handle suggested server detection
        // and installation status updates. We don't scan for suggested servers
        // here — PersistedWorkspace::new() already kicks off detection at startup
        // and emits AvailableServersDetected for each workspace, which the
        // subscription below handles.
        let persisted = PersistedWorkspace::handle(ctx);

        ctx.subscribe_to_model(&persisted, move |me, _model, event, ctx| match event {
            PersistedWorkspaceEvent::AvailableServersDetected {
                workspace_path,
                servers,
            } => {
                // New suggested servers detected — kick off install detection
                // and resize mouse states.
                for &server_type in servers {
                    #[cfg(feature = "local_fs")]
                    let status = PersistedWorkspace::handle(ctx).update(ctx, |model, ctx| {
                        model.detect_lsp_workspace_status(workspace_path.clone(), server_type, ctx)
                    });
                    #[cfg(not(feature = "local_fs"))]
                    let status = LspRepoStatus::CheckingForInstallation;
                    me.suggested_server_statuses
                        .insert((workspace_path.clone(), server_type), status);
                }
                let new_count = PersistedWorkspace::as_ref(ctx).total_lsp_server_count(true);
                if me.lsp_row_mouse_states.len() != new_count {
                    me.lsp_row_mouse_states
                        .resize_with(new_count, Default::default);
                }
                ctx.notify();
            }
            PersistedWorkspaceEvent::InstallStatusUpdate {
                server_type,
                status,
            } => {
                let new_status = LspRepoStatus::from_installation_status(status, *server_type);
                for ((_, st), repo_status) in &mut me.suggested_server_statuses {
                    if *st == *server_type {
                        *repo_status = new_status.clone();
                    }
                }
                ctx.notify();
            }
            PersistedWorkspaceEvent::WorkspaceAdded { .. } => {
                let new_count = PersistedWorkspace::as_ref(ctx).total_lsp_server_count(true);
                me.lsp_row_mouse_states
                    .resize_with(new_count, Default::default);
                ctx.notify();
            }
            PersistedWorkspaceEvent::InstallationSucceeded
            | PersistedWorkspaceEvent::InstallationFailed => {
                ctx.notify();
            }
        });

        Self {
            page: Self::build_page(ctx),
            lsp_row_mouse_states: (0..lsp_server_count).map(|_| Default::default()).collect(),
            suggested_server_statuses: HashMap::new(),
        }
    }

    fn build_page(_ctx: &mut ViewContext<Self>) -> PageType<Self> {
        PageType::new_monolith(CodePageWidget, Some(PAGE_TITLE), true)
    }
}

impl Entity for ProjectsPageView {
    type Event = ProjectsPageEvent;
}

impl View for ProjectsPageView {
    fn ui_name() -> &'static str {
        "ProjectsPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Debug, Clone)]
pub enum ProjectsPageEvent {
    OpenLspLogs { log_path: PathBuf },
}

// Define the code page actions.
#[derive(Debug, Clone)]
pub enum ProjectsPageAction {
    /// Toggle an LSP server on/off for a workspace.
    ToggleLspServer {
        workspace_path: PathBuf,
        server_type: LSPServerType,
        currently_enabled: bool,
    },
    RestartLspServer {
        server: ModelHandle<LspServerModel>,
    },
    OpenLspLogs {
        log_path: PathBuf,
    },
    /// Install (if needed) and enable a suggested LSP server.
    InstallAndEnableLspServer {
        workspace_path: PathBuf,
        server_type: LSPServerType,
    },
    /// Enable a suggested LSP server that is already installed.
    EnableSuggestedLspServer {
        workspace_path: PathBuf,
        server_type: LSPServerType,
    },
}

impl TypedActionView for ProjectsPageView {
    type Action = ProjectsPageAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            ProjectsPageAction::ToggleLspServer {
                workspace_path,
                server_type,
                currently_enabled,
            } => {
                if *currently_enabled {
                    // Toggling OFF: stop and disable
                    send_telemetry_from_ctx!(
                        LspTelemetryEvent::ServerRemoved {
                            server_type: server_type.binary_name().to_string(),
                            source: LspEnablementSource::Settings,
                        },
                        ctx
                    );
                    LspManagerModel::handle(ctx).update(ctx, |manager, ctx| {
                        manager.remove_server(workspace_path, *server_type, ctx);
                    });
                    PersistedWorkspace::handle(ctx).update(ctx, |workspace, _| {
                        workspace.disable_lsp_server_for_path(workspace_path, *server_type);
                    });
                } else {
                    // Toggling ON: enable and spawn
                    send_telemetry_from_ctx!(
                        LspTelemetryEvent::ServerEnabled {
                            server_type: server_type.binary_name().to_string(),
                            source: LspEnablementSource::Settings,
                            needed_install: false,
                        },
                        ctx
                    );
                    let workspace_path = workspace_path.clone();
                    PersistedWorkspace::handle(ctx).update(ctx, |workspace, _ctx| {
                        workspace.enable_lsp_server_for_path(&workspace_path, *server_type);
                        #[cfg(feature = "local_fs")]
                        workspace.execute_lsp_task(
                            crate::code::persisted_workspace::LspTask::Spawn {
                                file_path: workspace_path,
                            },
                            _ctx,
                        );
                    });
                }
                ctx.notify();
            }
            ProjectsPageAction::RestartLspServer { server } => {
                let server_name = server.as_ref(ctx).server_name();
                send_telemetry_from_ctx!(
                    LspTelemetryEvent::ControlAction {
                        action: LspControlActionType::Restart,
                        server_type: Some(server_name),
                    },
                    ctx
                );
                server.update(ctx, |server, ctx| {
                    server.restart(ctx);
                });
            }
            ProjectsPageAction::OpenLspLogs { log_path } => {
                send_telemetry_from_ctx!(
                    LspTelemetryEvent::ControlAction {
                        action: LspControlActionType::OpenLogs,
                        server_type: None,
                    },
                    ctx
                );
                ctx.emit(ProjectsPageEvent::OpenLspLogs {
                    log_path: log_path.clone(),
                });
            }
            ProjectsPageAction::InstallAndEnableLspServer {
                workspace_path,
                server_type,
            } => {
                send_telemetry_from_ctx!(
                    LspTelemetryEvent::ServerEnabled {
                        server_type: server_type.binary_name().to_string(),
                        source: LspEnablementSource::Settings,
                        needed_install: true,
                    },
                    ctx
                );
                #[cfg(feature = "local_fs")]
                {
                    let workspace_path = workspace_path.clone();
                    let server_type = *server_type;
                    PersistedWorkspace::handle(ctx).update(ctx, |workspace, _ctx| {
                        workspace.execute_lsp_task(
                            crate::code::persisted_workspace::LspTask::Install {
                                file_path: workspace_path.clone(),
                                repo_root: workspace_path,
                                server_type,
                            },
                            _ctx,
                        );
                    });
                }
                #[cfg(not(feature = "local_fs"))]
                let _ = workspace_path;
                ctx.notify();
            }
            ProjectsPageAction::EnableSuggestedLspServer {
                workspace_path,
                server_type,
            } => {
                send_telemetry_from_ctx!(
                    LspTelemetryEvent::ServerEnabled {
                        server_type: server_type.binary_name().to_string(),
                        source: LspEnablementSource::Settings,
                        needed_install: false,
                    },
                    ctx
                );
                let workspace_path = workspace_path.clone();
                let server_type = *server_type;
                PersistedWorkspace::handle(ctx).update(ctx, |workspace, _ctx| {
                    workspace.enable_lsp_server_for_path(&workspace_path, server_type);
                    #[cfg(feature = "local_fs")]
                    workspace.execute_lsp_task(
                        crate::code::persisted_workspace::LspTask::Spawn {
                            file_path: workspace_path,
                        },
                        _ctx,
                    );
                });
                ctx.notify();
            }
        }
    }
}

struct CodePageWidget;

impl SettingsWidget for CodePageWidget {
    type View = ProjectsPageView;

    fn search_terms(&self) -> &str {
        "code projects repository path lsp language server"
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut content = Flex::column();

        let mouse_states = InitializedFoldersMouseStates {
            lsp_rows: view.lsp_row_mouse_states.clone(),
        };

        content.add_child(self.render_initialized_folders(
            mouse_states,
            &view.suggested_server_statuses,
            appearance,
            app,
        ));

        content.finish()
    }
}

impl CodePageWidget {
    /// Renders persisted projects and their language servers.
    fn render_initialized_folders(
        &self,
        mouse_states: InitializedFoldersMouseStates,
        suggested_server_statuses: &HashMap<(PathBuf, LSPServerType), LspRepoStatus>,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let InitializedFoldersMouseStates {
            lsp_rows: lsp_row_mouse_states,
        } = mouse_states;

        let mut content = Flex::column();

        // Get workspaces from PersistedWorkspace
        let workspaces: Vec<WorkspaceMetadata> =
            PersistedWorkspace::as_ref(app).workspaces().collect();
        let lsp_manager = LspManagerModel::as_ref(app);
        let persisted_workspace = PersistedWorkspace::as_ref(app);

        let mut lsp_mouse_index = 0;
        let mut rendered_folder = false;

        for workspace in &workspaces {
            let workspace_path = &workspace.path;

            // Get all LSP servers (enabled + disabled + suggested) for this workspace
            let all_servers: Vec<(LSPServerType, EnablementState)> = persisted_workspace
                .all_lsp_servers(workspace_path, true)
                .map(|iter| iter.collect())
                .unwrap_or_default();

            rendered_folder = true;

            // Get LSP server mouse states
            let lsp_mouse_states: Vec<LspServerRowMouseStates> = all_servers
                .iter()
                .map(|_| {
                    let state = lsp_row_mouse_states
                        .get(lsp_mouse_index)
                        .cloned()
                        .unwrap_or_default();

                    lsp_mouse_index += 1;

                    state
                })
                .collect();

            content.add_child(self.render_workspace_row(
                workspace_path,
                &all_servers,
                lsp_manager,
                lsp_mouse_states,
                suggested_server_statuses,
                appearance,
                app,
            ));
        }
        if !rendered_folder {
            content.add_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .paragraph("No folders have been initialized yet.")
                        .build()
                        .finish(),
                )
                .with_margin_bottom(MAIN_SECTION_MARGIN)
                .finish(),
            );
        }

        content.finish()
    }

    /// Renders a persisted workspace and its language servers.
    #[allow(clippy::too_many_arguments)]
    fn render_workspace_row(
        &self,
        workspace_path: &Path,
        all_servers: &[(LSPServerType, EnablementState)],
        lsp_manager: &LspManagerModel,
        lsp_mouse_states: Vec<LspServerRowMouseStates>,
        suggested_server_statuses: &HashMap<(PathBuf, LSPServerType), LspRepoStatus>,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let mut workspace_content = Flex::column().with_spacing(MAIN_SECTION_MARGIN);

        // Workspace path header
        let home_dir =
            dirs::home_dir().and_then(|home_dir| home_dir.to_str().map(|s| s.to_owned()));
        let user_friendly = user_friendly_path(
            workspace_path.to_string_lossy().as_ref(),
            home_dir.as_deref(),
        )
        .to_string();

        workspace_content.add_child(self.render_workspace_header(user_friendly, appearance));

        // LSP Servers section (if any servers known)
        if !all_servers.is_empty() {
            workspace_content.add_child(self.render_lsp_servers_subsection(
                workspace_path,
                all_servers,
                lsp_manager,
                lsp_mouse_states,
                suggested_server_statuses,
                appearance,
                app,
            ));
        }

        self.render_workspace_row_container(workspace_content.finish(), appearance)
    }

    fn render_workspace_header(&self, label: String, appearance: &Appearance) -> Box<dyn Element> {
        let theme = appearance.theme();
        let path_label = Shrinkable::new(
            1.,
            appearance
                .ui_builder()
                .span(label)
                .with_style(UiComponentStyles {
                    font_family_id: Some(appearance.monospace_font_family()),
                    font_size: Some(appearance.ui_font_size()),
                    font_weight: Some(Weight::Bold),
                    font_color: Some(theme.active_ui_text_color().into()),
                    ..Default::default()
                })
                .build()
                .finish(),
        )
        .finish();

        let mut header_row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);
        header_row.add_child(Expanded::new(1., path_label).finish());
        header_row.finish()
    }

    fn render_workspace_row_container(
        &self,
        workspace_content: Box<dyn Element>,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        Container::new(workspace_content)
            .with_uniform_padding(MAIN_SECTION_MARGIN)
            .with_background(theme.surface_1())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .with_margin_bottom(MAIN_SECTION_MARGIN)
            .finish()
    }

    /// Renders the LSP servers subsection within a workspace row.
    #[allow(clippy::too_many_arguments)]
    fn render_lsp_servers_subsection(
        &self,
        workspace_path: &Path,
        all_servers: &[(LSPServerType, EnablementState)],
        lsp_manager: &LspManagerModel,
        lsp_mouse_states: Vec<LspServerRowMouseStates>,
        suggested_server_statuses: &HashMap<(PathBuf, LSPServerType), LspRepoStatus>,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let ui_builder = appearance.ui_builder();
        let theme = appearance.theme();

        let mut content = Flex::column().with_spacing(SUB_SECTION_MARGIN);

        // "LSP SERVERS" label
        content.add_child(
            ui_builder
                .span("LSP SERVERS")
                .with_style(UiComponentStyles {
                    font_size: Some(11.0),
                    font_weight: Some(Weight::Semibold),
                    font_color: Some(theme.disabled_ui_text_color().into()),
                    ..Default::default()
                })
                .build()
                .finish(),
        );

        // Get the actual server models for this workspace
        let server_models = lsp_manager.servers_for_workspace(workspace_path);

        for (idx, (server_type, enablement_state)) in all_servers.iter().enumerate() {
            let mouse_states = lsp_mouse_states.get(idx).cloned().unwrap_or_default();

            if *enablement_state == EnablementState::Suggested {
                // Render the "available for download" suggested server row.
                let repo_status = suggested_server_statuses
                    .get(&(workspace_path.to_path_buf(), *server_type))
                    .cloned();
                content.add_child(self.render_suggested_lsp_server_row(
                    workspace_path,
                    *server_type,
                    repo_status,
                    mouse_states,
                    appearance,
                ));
            } else {
                let is_enabled = *enablement_state == EnablementState::Yes;

                // Find the corresponding server model (only exists if enabled and running)
                let server_model = server_models.and_then(|servers| {
                    servers
                        .iter()
                        .find(|s| s.as_ref(app).server_type() == *server_type)
                });

                content.add_child(self.render_lsp_server_row(
                    workspace_path,
                    *server_type,
                    server_model,
                    is_enabled,
                    mouse_states,
                    appearance,
                    app,
                ));
            }
        }

        content.finish()
    }

    /// Renders a suggested LSP server row with "+" install/enable button.
    fn render_suggested_lsp_server_row(
        &self,
        workspace_path: &Path,
        server_type: LSPServerType,
        repo_status: Option<LspRepoStatus>,
        mouse_states: LspServerRowMouseStates,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder();

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        // Left side: language initial badge + name/description column
        let mut left_content = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);

        // Language initial badge (no status dot for suggested servers)
        let badge_size = 36.0;
        let avatar = Avatar::new(
            AvatarContent::DisplayName(server_type.binary_name().to_string()),
            UiComponentStyles {
                width: Some(badge_size),
                height: Some(badge_size),
                border_radius: Some(CornerRadius::with_all(Radius::Percentage(50.))),
                font_family_id: Some(appearance.ui_font_family()),
                font_weight: Some(Weight::Bold),
                background: Some(theme.surface_3().into()),
                font_size: Some(16.),
                font_color: Some(theme.active_ui_text_color().into()),
                ..Default::default()
            },
        );

        left_content.add_child(
            Container::new(avatar.build().finish())
                .with_margin_right(8.)
                .finish(),
        );

        // Name + description
        let mut name_desc_column = Flex::column().with_spacing(4.);

        name_desc_column.add_child(
            ui_builder
                .span(server_type.binary_name())
                .with_style(UiComponentStyles {
                    font_size: Some(12.0),
                    font_color: Some(theme.active_ui_text_color().into()),
                    ..Default::default()
                })
                .build()
                .finish(),
        );

        let (description, is_installing) = match &repo_status {
            Some(LspRepoStatus::DisabledAndInstalled { .. }) => ("Installed", false),
            Some(LspRepoStatus::Installing { .. }) => ("Installing...", true),
            Some(LspRepoStatus::CheckingForInstallation) => ("Checking...", true),
            _ => ("Available for download", false),
        };

        name_desc_column.add_child(
            ui_builder
                .label(description)
                .with_style(UiComponentStyles {
                    font_color: Some(theme.disabled_ui_text_color().into()),
                    font_size: Some(12.),
                    ..Default::default()
                })
                .build()
                .finish(),
        );

        left_content.add_child(name_desc_column.finish());
        row.add_child(left_content.finish());

        // Right side: "+" button to install/enable
        if !is_installing {
            let workspace_path_clone = workspace_path.to_path_buf();
            let needs_install = matches!(
                &repo_status,
                None | Some(LspRepoStatus::DisabledAndNotInstalled { .. })
            );
            let install_button = icon_button(appearance, Icon::Plus, false, mouse_states.install)
                .with_style(UiComponentStyles {
                    border_width: Some(1.),
                    border_color: Some(theme.surface_3().into()),
                    ..Default::default()
                })
                .build()
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    if needs_install {
                        ctx.dispatch_typed_action(ProjectsPageAction::InstallAndEnableLspServer {
                            workspace_path: workspace_path_clone.clone(),
                            server_type,
                        });
                    } else {
                        ctx.dispatch_typed_action(ProjectsPageAction::EnableSuggestedLspServer {
                            workspace_path: workspace_path_clone.clone(),
                            server_type,
                        });
                    }
                })
                .finish();

            row.add_child(install_button);
        }

        Container::new(row.finish())
            .with_uniform_padding(12.)
            .with_background(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
    }

    /// Renders a single LSP server row with language initial icon, status, and toggle.
    #[allow(clippy::too_many_arguments)]
    fn render_lsp_server_row(
        &self,
        workspace_path: &Path,
        server_type: LSPServerType,
        server_model: Option<&warpui::ModelHandle<LspServerModel>>,
        is_enabled: bool,
        mouse_states: LspServerRowMouseStates,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder();

        let mut row = Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        // Left side: language initial badge + name/status column
        let mut left_content = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);

        // Language initial badge with status dot overlay (using Avatar component)
        let (status_color, status_text) = self.get_lsp_status_info(server_model, app, theme);
        let is_failed = server_model
            .is_some_and(|model| matches!(model.as_ref(app).state(), LspState::Failed { .. }));

        // Language initial badge with status dot overlay (using Avatar component)
        let badge_size = 36.0;
        let mut avatar = Avatar::new(
            AvatarContent::DisplayName(server_type.binary_name().to_string()),
            UiComponentStyles {
                width: Some(badge_size),
                height: Some(badge_size),
                border_radius: Some(CornerRadius::with_all(Radius::Percentage(50.))),
                font_family_id: Some(appearance.ui_font_family()),
                font_weight: Some(Weight::Bold),
                background: Some(theme.surface_3().into()),
                font_size: Some(16.),
                font_color: Some(theme.active_ui_text_color().into()),
                ..Default::default()
            },
        );

        avatar = avatar.with_status_element_with_offset(
            StatusElementTypes::Circle,
            UiComponentStyles {
                width: Some(LSP_STATUS_INDICATOR_SIZE),
                height: Some(LSP_STATUS_INDICATOR_SIZE),
                border_radius: Some(CornerRadius::with_all(Radius::Percentage(50.))),
                background: Some(Fill::Solid(status_color)),
                ..Default::default()
            },
            -5.,
            5.,
        );

        left_content.add_child(
            Container::new(avatar.build().finish())
                .with_margin_right(8.)
                .finish(),
        );

        // Name + status on separate lines
        let mut name_status_column = Flex::column().with_spacing(4.);

        // Server name
        name_status_column.add_child(
            ui_builder
                .span(server_type.binary_name())
                .with_style(UiComponentStyles {
                    font_size: Some(12.0),
                    font_color: Some(theme.active_ui_text_color().into()),
                    ..Default::default()
                })
                .build()
                .finish(),
        );

        // Status text
        let status_text_color = if is_failed {
            Some(status_color)
        } else {
            Some(theme.disabled_ui_text_color().into())
        };

        name_status_column.add_child(
            ui_builder
                .label(status_text)
                .with_style(UiComponentStyles {
                    font_color: status_text_color,
                    font_size: Some(12.),
                    ..Default::default()
                })
                .build()
                .finish(),
        );

        left_content.add_child(name_status_column.finish());
        row.add_child(left_content.finish());

        // Right side: restart/logs buttons (if failed) + toggle switch (always)
        let mut right_content = Flex::row()
            .with_spacing(8.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center);

        if is_failed && let Some(server_handle) = server_model.cloned() {
            let server_for_action = server_handle.clone();
            let restart_button = ui_builder
                .button(ButtonVariant::Secondary, mouse_states.restart)
                .with_style(UiComponentStyles {
                    font_size: Some(12.),
                    ..Default::default()
                })
                .with_hovered_styles(UiComponentStyles {
                    background: Some(theme.surface_3().into()),
                    ..Default::default()
                })
                .with_text_label("Restart server".to_owned())
                .build()
                .with_cursor(Cursor::PointingHand)
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ProjectsPageAction::RestartLspServer {
                        server: server_for_action.clone(),
                    });
                })
                .finish();

            right_content.add_child(restart_button);
        }

        // Show "View logs" when the server has been started (Available, Starting/Busy, or Failed)
        #[cfg(not(target_family = "wasm"))]
        {
            let has_logs = server_model.is_some_and(|model| {
                matches!(
                    model.as_ref(app).state(),
                    LspState::Available { .. } | LspState::Starting | LspState::Failed { .. }
                )
            });
            if has_logs {
                let log_path = crate::code::lsp_logs::log_file_path(server_type, workspace_path);
                let view_logs_button = ui_builder
                    .button(ButtonVariant::Accent, mouse_states.view_logs)
                    .with_style(UiComponentStyles {
                        font_size: Some(12.),
                        ..Default::default()
                    })
                    .with_text_label("View logs".to_owned())
                    .build()
                    .with_cursor(Cursor::PointingHand)
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(ProjectsPageAction::OpenLspLogs {
                            log_path: log_path.clone(),
                        });
                    })
                    .finish();

                right_content.add_child(view_logs_button);
            }
        }

        // Toggle switch (always shown)
        let workspace_path_clone = workspace_path.to_path_buf();
        let server_type_clone = server_type;
        right_content.add_child(
            ui_builder
                .switch(mouse_states.toggle)
                .check(is_enabled)
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(ProjectsPageAction::ToggleLspServer {
                        workspace_path: workspace_path_clone.clone(),
                        server_type: server_type_clone,
                        currently_enabled: is_enabled,
                    });
                })
                .finish(),
        );

        row.add_child(right_content.finish());

        Container::new(row.finish())
            .with_uniform_padding(12.)
            .with_background(theme.surface_2())
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .finish()
    }

    /// Gets the status color and text for an LSP server.
    fn get_lsp_status_info(
        &self,
        server_model: Option<&warpui::ModelHandle<LspServerModel>>,
        app: &AppContext,
        theme: &warp_core::ui::theme::WarpTheme,
    ) -> (ColorU, &'static str) {
        match server_model {
            Some(model) => {
                let server = model.as_ref(app);
                match server.state() {
                    LspState::Available { .. } if !server.has_pending_tasks() => (
                        AnsiColorIdentifier::Green
                            .to_ansi_color(&theme.terminal_colors().normal)
                            .into(),
                        "Available",
                    ),
                    LspState::Starting | LspState::Available { .. } => (
                        AnsiColorIdentifier::Yellow
                            .to_ansi_color(&theme.terminal_colors().normal)
                            .into(),
                        "Busy",
                    ),
                    LspState::Failed { .. } => (
                        AnsiColorIdentifier::Red
                            .to_ansi_color(&theme.terminal_colors().normal)
                            .into(),
                        "Failed",
                    ),
                    LspState::Stopped { .. } | LspState::Stopping { .. } => {
                        (theme.disabled_ui_text_color().into_solid(), "Stopped")
                    }
                }
            }
            None => (theme.disabled_ui_text_color().into_solid(), "Not running"),
        }
    }
}

impl SettingsPageMeta for ProjectsPageView {
    fn section() -> SettingsSection {
        SettingsSection::Projects
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<ProjectsPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<ProjectsPageView>) -> Self {
        SettingsPageViewHandle::Projects(view_handle)
    }
}
