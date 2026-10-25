use futures::Future;
use warp_errors::report_error;
use warpui::elements::{
    Align, Flex, Hoverable, MouseStateHandle, ParentElement, SavePosition, Shrinkable,
};
use warpui::presenter::ChildView;
use warpui::windowing::{StateEvent, WindowManager};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::CloudObjectTypeAndId;
use super::index::{DriveIndex, DriveIndexAction, DriveIndexEvent};
use super::items::WarpDriveItemId;
use crate::cloud_object::model::persistence::CloudModel;
use crate::cloud_object::{Owner, Space};
use crate::server::cloud_objects::update_manager::{InitiatedBy, UpdateManager};
use crate::server::ids::{ClientId, SyncId};
use crate::server::telemetry::SharingDialogSource;
use crate::workspaces::user_workspaces::UserWorkspaces;

pub const MIN_SIDEBAR_WIDTH: f32 = 250.;
pub const MAX_SIDEBAR_WIDTH_RATIO: f32 = 0.75;

pub const WARP_DRIVE_POSITION_ID: &str = "warp_drive";

/// The sidebar that houses Warp Drive.
/// `DrivePanel` is different from `DriveIndex` in that it is responsible for
/// how Warp Drive interacts with the workspace and the rest of the app, whereas
/// `DriveIndex` is the main warp drive view and responsible for the internals of Warp Drive.
pub struct DrivePanel {
    index_view: ViewHandle<DriveIndex>,
    mouse_state_handles: MouseStateHandles,
}

#[derive(Clone, Default)]
struct MouseStateHandles {
    focus_panel_mouse_state: MouseStateHandle,
}

#[derive(Clone, Debug)]
pub enum DrivePanelAction {
    /// Focus WD panel (via single click)
    FocusDriveIndex,
}

#[derive(Clone, Debug)]
pub enum DrivePanelEvent {
    FocusWarpDrive,
}

impl DrivePanel {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let index_view = ctx.add_typed_action_view(move |ctx| {
            let mut index = DriveIndex::new(ctx);
            index.initialize_section_states(ctx);
            index
        });
        ctx.subscribe_to_view(&index_view, |me, _, event, ctx| {
            me.handle_index_view_event(event, ctx);
        });

        // Subscribe to window state changes for focus dimming updates
        let state_handle = WindowManager::handle(ctx);
        ctx.subscribe_to_model(&state_handle, |_me, _, event, ctx| {
            match &event {
                StateEvent::ValueChanged { current, previous } => {
                    // Re-render if this window's focus state has changed
                    if WindowManager::did_window_change_focus(ctx.window_id(), current, previous) {
                        ctx.notify();
                    }
                }
            }
        });

        Self {
            index_view,
            mouse_state_handles: Default::default(),
        }
    }

    /// Helper to get the [`Owner`] for a new object created from the index.
    fn new_object_owner(
        space: Space,
        initial_folder_id: Option<&SyncId>,
        app: &AppContext,
    ) -> Option<Owner> {
        match initial_folder_id {
            Some(folder_id) => CloudModel::as_ref(app)
                .get_folder(folder_id)
                .map(|folder| folder.permissions.owner),
            None => UserWorkspaces::as_ref(app).space_to_owner(space, app),
        }
    }

    /// Event handler for actions that occur within the index view
    fn handle_index_view_event(&mut self, event: &DriveIndexEvent, ctx: &mut ViewContext<Self>) {
        match event {
            DriveIndexEvent::FocusWarpDrive => ctx.emit(DrivePanelEvent::FocusWarpDrive),
            DriveIndexEvent::CreateFolder {
                space,
                title,
                initial_folder_id,
            } => match Self::new_object_owner(*space, initial_folder_id.as_ref(), ctx) {
                Some(owner) => {
                    let client_id = ClientId::default();
                    UpdateManager::handle(ctx).update(ctx, |update_manager, ctx| {
                        update_manager.create_folder(
                            title.clone(),
                            owner,
                            client_id,
                            *initial_folder_id,
                            true,
                            InitiatedBy::User,
                            ctx,
                        );
                    });
                }
                None => {
                    report_error!(
                        "Cannot identify a folder owner",
                        extra: { "space" => ?space }
                    );
                }
            },
        }
    }

    /// Sets the child view back to a default state
    fn save_and_clear_child_view(&mut self, ctx: &mut ViewContext<Self>) {
        self.reset_all_menus(ctx);
    }

    /// Reset all context menus in all views
    fn reset_all_menus(&mut self, ctx: &mut ViewContext<Self>) {
        self.index_view.update(ctx, |index_view, ctx| {
            index_view.reset_and_open_to_main_index(ctx);
            index_view.reset_menus(ctx);
        });
    }

    pub fn set_selected_object(
        &mut self,
        id: Option<WarpDriveItemId>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.index_view.update(ctx, |index_view, ctx| {
            index_view.set_selected_object(id, ctx);
        });
    }

    pub fn open_cloud_object_dialog(
        &mut self,
        space: Space,
        initial_folder_id: Option<SyncId>,
        ctx: &mut ViewContext<Self>,
    ) {
        self.save_and_clear_child_view(ctx);
        self.index_view.update(ctx, |index_view, ctx| {
            index_view.handle_action(
                &DriveIndexAction::create_object(space, initial_folder_id),
                ctx,
            )
        });
        ctx.notify();
    }

    /// Recomputes and initializes the section states for the WD Index. This is needed after
    /// we directly change anything about the state of the index (such as folders being open/closed).
    ///
    /// This should only be called if we immeidiately need to update and rely on the updated state.
    pub fn initialize_drive_section_states(&mut self, ctx: &mut ViewContext<Self>) {
        self.index_view.update(ctx, |index, ctx| {
            index.initialize_section_states(ctx);
        })
    }

    pub fn expand_section_for_drive_item_id(
        &mut self,
        item_id: WarpDriveItemId,
        ctx: &mut ViewContext<Self>,
    ) {
        self.index_view.update(ctx, |index, ctx| {
            index.expand_section_for_drive_item_id(item_id, ctx);
        })
    }

    /// This functions scrolls the relevant Warp Drive item into view.
    pub fn scroll_item_into_view(&mut self, item_id: WarpDriveItemId, ctx: &mut ViewContext<Self>) {
        self.index_view.update(ctx, |index, ctx| {
            index.scroll_item_into_view(item_id, ctx);
        })
    }

    /// This functions sets the index of a focused Warp Drive item.
    pub fn set_focused_index(&mut self, focused_index: Option<usize>, ctx: &mut ViewContext<Self>) {
        self.index_view.update(ctx, |index, ctx| {
            index.set_focused_index(focused_index, true, ctx);
        })
    }

    pub fn set_focused_item(&mut self, item_id: WarpDriveItemId, ctx: &mut ViewContext<Self>) {
        self.index_view.update(ctx, |index, ctx| {
            ctx.focus(&self.index_view);
            index.set_focused_item(item_id, true, ctx);
        })
    }

    pub fn open_object_sharing_settings(
        &mut self,
        object_id: CloudObjectTypeAndId,
        invitee_email: Option<String>,
        source: SharingDialogSource,
        ctx: &mut ViewContext<Self>,
    ) {
        let warp_drive_item_id = WarpDriveItemId::Object(object_id);
        self.index_view.update(ctx, |index, ctx| {
            index.set_focused_item(warp_drive_item_id, true, ctx);
            index.toggle_share_dialog(&warp_drive_item_id, invitee_email, source, ctx);
        });
    }

    pub fn has_warp_drive_initialized_sections(
        &self,
        app: &AppContext,
    ) -> impl Future<Output = ()> + use<> {
        self.index_view.as_ref(app).has_initialized_sections()
    }

    pub fn reset_focused_index_in_warp_drive(
        &mut self,
        should_scroll: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        self.index_view.update(ctx, |index, ctx| {
            index.reset_focused_index_in_warp_drive(should_scroll, ctx);
        })
    }

    pub fn reset_and_open_to_main_index(&mut self, ctx: &mut ViewContext<Self>) {
        self.index_view.update(ctx, |index, ctx| {
            index.reset_and_open_to_main_index(ctx);
        })
    }

    pub fn undo_trash(
        &mut self,
        cloud_object_type_and_id: &CloudObjectTypeAndId,
        ctx: &mut ViewContext<Self>,
    ) {
        self.index_view.update(ctx, |index_view, ctx| {
            index_view.untrash_object(cloud_object_type_and_id, ctx)
        });
    }
}

impl View for DrivePanel {
    fn ui_name() -> &'static str {
        "WarpDrivePanel"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.index_view);
        }
    }

    fn render(&self, _app: &warpui::AppContext) -> Box<dyn warpui::Element> {
        let body = Hoverable::new(
            self.mouse_state_handles.focus_panel_mouse_state.clone(),
            |_| {
                Align::new(
                    SavePosition::new(
                        ChildView::new(&self.index_view).finish(),
                        WARP_DRIVE_POSITION_ID,
                    )
                    .finish(),
                )
                .top_center()
                .finish()
            },
        )
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(DrivePanelAction::FocusDriveIndex);
        })
        .finish();

        let mut col = Flex::column();
        col.add_child(Shrinkable::new(1., body).finish());
        col.with_main_axis_size(warpui::elements::MainAxisSize::Max)
            .finish()
    }
}

impl Entity for DrivePanel {
    type Event = DrivePanelEvent;
}

impl TypedActionView for DrivePanel {
    type Action = DrivePanelAction;

    fn handle_action(&mut self, action: &DrivePanelAction, ctx: &mut ViewContext<Self>) {
        match action {
            DrivePanelAction::FocusDriveIndex => {
                ctx.focus(&self.index_view);
                // should_scroll is set to false here in order to not let menu clicks autoscroll WD index
                self.reset_focused_index_in_warp_drive(false, ctx);
            }
        }
    }
}

#[cfg(test)]
#[path = "panel_tests.rs"]
mod tests;
