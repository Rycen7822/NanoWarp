//! Shared context menu implementation for notebooks.

use pathfinder_geometry::vector::Vector2F;
use warp_core::context_flag::ContextFlag;
use warpui::clipboard::ClipboardContent;
use warpui::elements::{ChildAnchor, OffsetPositioning, ParentAnchor, ParentOffsetBounds, Stack};
use warpui::keymap::Trigger;
use warpui::presenter::ChildView;
use warpui::{Action, Element, EventContext, TypedActionView, View, ViewContext, ViewHandle};

use super::editor::keys::custom_action_to_display;
use super::editor::view::RichTextEditorView;
use super::telemetry::ActionEntrypoint;
use crate::menu::{self, Menu, MenuItem, MenuItemFields};
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::{PaneEvent, SplitPaneState};
use crate::util::bindings::{
    CustomAction, keybinding_name_to_display_string, trigger_to_keystroke,
};

const CONTEXT_MENU_WIDTH: f32 = 200.;

pub struct ContextMenuState<V: TypedActionView + View>
where
    V::Action: Clone + From<ContextMenuAction>,
{
    /// The editor and position of the open menu. If `None`, the menu is closed.
    source: Option<MenuSource>,
    menu: ViewHandle<Menu<V::Action>>,
    /// Focus state of the pane containing this context menu.
    focus_handle: Option<PaneFocusHandle>,
    /// The display path of the file backing this pane, if any. When set, the menu offers a
    /// "Copy file path" item. If the file viewer has no path, the item is hidden.
    copy_file_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MenuSource {
    pub parent_offset: Vector2F,
    pub editor: ViewHandle<RichTextEditorView>,
}

impl<V: TypedActionView + View> ContextMenuState<V>
where
    V::Event: From<PaneEvent>,
    V::Action: Clone + From<ContextMenuAction>,
{
    pub fn new(ctx: &mut ViewContext<V>) -> Self {
        let menu = ctx.add_typed_action_view(|_| Menu::new().with_width(CONTEXT_MENU_WIDTH));

        ctx.subscribe_to_view(&menu, |view, _, event, ctx| match event {
            menu::Event::ItemSelected | menu::Event::ItemHovered => (),
            menu::Event::Close { via_select_item } => {
                view.handle_action(
                    &V::Action::from(ContextMenuAction::Close {
                        via_select_item: *via_select_item,
                    }),
                    ctx,
                );
            }
        });

        Self {
            source: None,
            menu,
            focus_handle: None,
            copy_file_path: None,
        }
    }

    pub(super) fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle) {
        self.focus_handle = Some(focus_handle);
    }

    /// Set the display path used by the "Copy file path" menu item. Pass `None` to hide the item
    /// when the file viewer has no path.
    pub(super) fn set_copy_file_path(&mut self, path: Option<String>) {
        self.copy_file_path = path;
    }

    /// Renders the context menu, if it's open.
    pub fn render(&self, stack: &mut Stack) {
        let offset = match self.source {
            Some(MenuSource { parent_offset, .. }) => parent_offset,
            None => return,
        };

        stack.add_positioned_overlay_child(
            ChildView::new(&self.menu).finish(),
            OffsetPositioning::offset_from_parent(
                offset,
                ParentOffsetBounds::WindowByPosition,
                ParentAnchor::TopLeft,
                ChildAnchor::TopLeft,
            ),
        );
    }

    /// Show the context menu.
    pub fn show_context_menu(&mut self, source: MenuSource, ctx: &mut ViewContext<V>) {
        let mut items = vec![];

        // Section 1: text selection actions.
        let editor = source.editor.as_ref(ctx);
        let has_selection =
            !editor.selection_is_single_cursor(ctx) || editor.has_command_selection(ctx);

        if has_selection {
            let item = MenuItemFields::new("Copy")
                .with_on_select_action(V::Action::from(ContextMenuAction::CopySelectedText))
                .with_key_shortcut_label(custom_action_to_display(CustomAction::Copy));
            items.push(item.into_item());
        }
        // Section 1b: Copy file path (only for file-backed panes).
        if self.copy_file_path.is_some() {
            if !items.is_empty() {
                items.push(MenuItem::Separator);
            }
            items.push(
                MenuItemFields::new("Copy file path")
                    .with_on_select_action(V::Action::from(ContextMenuAction::CopyFilePath))
                    .into_item(),
            );
        }

        // Section 2: Split-pane actions
        let split_pane_menu_items = self.split_pane_menu_items(ctx);
        if !items.is_empty() && !split_pane_menu_items.is_empty() {
            items.push(MenuItem::Separator);
        }
        if !split_pane_menu_items.is_empty() {
            items.extend(split_pane_menu_items);
        }

        self.menu.update(ctx, move |menu, ctx| {
            menu.set_items(items, ctx); // This also resets the selection.
        });
        self.source = Some(source);
        ctx.focus(&self.menu);
        ctx.notify();
    }

    fn split_pane_menu_items(&self, ctx: &mut ViewContext<V>) -> Vec<MenuItem<V::Action>> {
        let mut items = vec![];
        if ContextFlag::CreateNewSession.is_enabled() {
            items.extend([
                MenuItemFields::new("Split pane right")
                    .with_on_select_action(V::Action::from(ContextMenuAction::EmitPaneEvent(
                        PaneEvent::SplitRight(None),
                    )))
                    .with_key_shortcut_label(keybinding_name_to_display_string(
                        "pane_group:add_right",
                        ctx,
                    ))
                    .into_item(),
                MenuItemFields::new("Split pane left")
                    .with_on_select_action(V::Action::from(ContextMenuAction::EmitPaneEvent(
                        PaneEvent::SplitLeft(None),
                    )))
                    .with_key_shortcut_label(keybinding_name_to_display_string(
                        "pane_group:add_left",
                        ctx,
                    ))
                    .into_item(),
                MenuItemFields::new("Split pane down")
                    .with_on_select_action(V::Action::from(ContextMenuAction::EmitPaneEvent(
                        PaneEvent::SplitDown(None),
                    )))
                    .with_key_shortcut_label(keybinding_name_to_display_string(
                        "pane_group:add_down",
                        ctx,
                    ))
                    .into_item(),
                MenuItemFields::new("Split pane up")
                    .with_on_select_action(V::Action::from(ContextMenuAction::EmitPaneEvent(
                        PaneEvent::SplitUp(None),
                    )))
                    .with_key_shortcut_label(keybinding_name_to_display_string(
                        "pane_group:add_up",
                        ctx,
                    ))
                    .into_item(),
            ]);
        }

        let split_pane_state = self
            .focus_handle
            .as_ref()
            .map_or(SplitPaneState::NotInSplitPane, |h| h.split_pane_state(ctx));
        if split_pane_state.is_in_split_pane() {
            let is_maximized = split_pane_state.is_maximized();
            items.push(
                MenuItemFields::toggle_pane_action(is_maximized)
                    .with_on_select_action(V::Action::from(ContextMenuAction::EmitPaneEvent(
                        PaneEvent::ToggleMaximized,
                    )))
                    .with_key_shortcut_label(keybinding_name_to_display_string(
                        "pane_group:toggle_maximize_pane",
                        ctx,
                    ))
                    .into_item(),
            );

            items.push(
                MenuItemFields::new("Close pane")
                    .with_on_select_action(V::Action::from(ContextMenuAction::EmitPaneEvent(
                        PaneEvent::Close,
                    )))
                    .with_key_shortcut_label(
                        trigger_to_keystroke(&Trigger::Custom(
                            CustomAction::CloseCurrentSession.into(),
                        ))
                        .map(|keystroke| keystroke.displayed()),
                    )
                    .into_item(),
            );
        }
        items
    }

    /// Close the context menu. If `focus_parent` is true, the parent view (either the editor that
    /// triggered the context menu or the notebook view) will be focused.
    pub fn close_context_menu(&mut self, focus_parent: bool, ctx: &mut ViewContext<V>) {
        if focus_parent {
            match &self.source {
                Some(MenuSource { editor, .. }) => ctx.focus(editor),
                None => ctx.focus_self(),
            }
        }
        self.source = None;
        ctx.notify();
    }

    #[cfg(test)]
    /// List out the context menu items by name.
    pub fn item_names<'a>(&self, ctx: &'a impl warpui::ViewAsRef) -> Vec<&'a str> {
        self.menu
            .as_ref(ctx)
            .items()
            .iter()
            .map(|item| match item {
                MenuItem::Item(item) => item.label(),
                MenuItem::Separator => "----",
                MenuItem::ItemsRow { .. } => panic!("ItemsRow not supported"),
                MenuItem::Submenu { fields, .. } => fields.label(),
                MenuItem::Header { fields, .. } => fields.label(),
            })
            .collect()
    }

    pub fn handle_action(&mut self, action: &ContextMenuAction, ctx: &mut ViewContext<V>) {
        match action {
            ContextMenuAction::Open(source) => self.show_context_menu(source.clone(), ctx),
            ContextMenuAction::Close { via_select_item } => {
                self.close_context_menu(!*via_select_item, ctx)
            }
            ContextMenuAction::CopySelectedText => {
                if let Some(MenuSource { editor, .. }) = &self.source {
                    editor.update(ctx, |editor, ctx| editor.copy(ActionEntrypoint::Menu, ctx))
                }
            }
            ContextMenuAction::CopyFilePath => {
                if let Some(path) = self.copy_file_path.clone() {
                    ctx.clipboard().write(ClipboardContent::plain_text(path));
                }
            }
            ContextMenuAction::EmitPaneEvent(event) => ctx.emit(V::Event::from(event.clone())),
        }
    }
}

/// Dispatch an action to show the notebook context menu for a rich text editor view.
pub fn show_rich_editor_context_menu<A>(
    ctx: &mut EventContext,
    position: Vector2F,
    parent_position_id: &str,
    editor: &ViewHandle<RichTextEditorView>,
) where
    A: Action + From<ContextMenuAction>,
{
    if let Some(parent_bounds) = ctx.element_position_by_id(parent_position_id) {
        let offset = position - parent_bounds.origin();
        ctx.dispatch_typed_action(A::from(ContextMenuAction::Open(MenuSource {
            parent_offset: offset,
            editor: editor.clone(),
        })));
    }
}

#[derive(Debug, Clone)]
pub enum ContextMenuAction {
    Open(MenuSource),
    Close { via_select_item: bool },
    CopySelectedText,
    CopyFilePath,
    EmitPaneEvent(PaneEvent),
}
