//! Integration tests for settings sidebar navigation and search.
//!
//! These pin down the user-visible behavior of the settings nav rail —
//! clicking rows, arrow-key cycling, umbrella expand/collapse, and search
//! filtering across both top-level pages and umbrella subpages — so that
//! refactors of the settings page model cannot silently regress them.

use warp::integration_testing::settings::{
    assert_settings_nav_page_visible, assert_settings_nav_subpage_visible, assert_settings_section,
    assert_settings_widget_rendered, assert_umbrella_expanded, clear_settings_search,
    click_settings_nav_subpage, click_settings_umbrella, open_settings_page,
    press_settings_nav_down, press_settings_nav_up, type_settings_search,
};
use warp::integration_testing::terminal::wait_until_bootstrapped_single_pane_for_tab;
use warp::settings_view::{SettingsSection, settings_widget_deeplink_target};

use super::{Builder, new_builder};

/// Label of the umbrella that groups the code subpages.
const CODE_UMBRELLA: &str = "Code";

// ---------------------------------------------------------------------------
// Mouse navigation
// ---------------------------------------------------------------------------

/// Clicking an umbrella header expands it without changing the selection,
/// clicking a subpage selects it, and collapsing the umbrella again keeps the
/// selection even though the row is hidden.
pub fn test_settings_mouse_navigation_through_umbrella() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::Appearance))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        // Expanding the umbrella reveals its subpages but must not move the
        // selection off Appearance.
        .with_step(click_settings_umbrella(CODE_UMBRELLA))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, true))
        .with_step(assert_settings_section(SettingsSection::Appearance))
        .with_step(assert_settings_nav_subpage_visible(
            SettingsSection::EditorAndCodeReview,
            true,
        ))
        // Clicking a subpage selects it.
        .with_step(click_settings_nav_subpage(
            SettingsSection::EditorAndCodeReview,
        ))
        .with_step(assert_settings_section(
            SettingsSection::EditorAndCodeReview,
        ))
        // Collapsing while still on a subpage hides the row but keeps the
        // selection, so the content pane does not change out from under us.
        .with_step(click_settings_umbrella(CODE_UMBRELLA))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        .with_step(assert_settings_section(
            SettingsSection::EditorAndCodeReview,
        ))
        .with_step(assert_settings_nav_subpage_visible(
            SettingsSection::EditorAndCodeReview,
            false,
        ))
}

// ---------------------------------------------------------------------------
// Keyboard navigation
// ---------------------------------------------------------------------------

/// Arrowing Down into a collapsed umbrella enters it at its first subpage and
/// expands it, rather than skipping over the whole group.
pub fn test_settings_keyboard_navigation_down_into_collapsed_umbrella() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::Appearance))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        .with_step(press_settings_nav_down())
        .with_step(assert_settings_section(SettingsSection::Projects))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, true))
}

/// Arrowing Up into a collapsed umbrella enters it at its *last* subpage,
/// matching the reading order the user was moving through.
pub fn test_settings_keyboard_navigation_up_into_collapsed_umbrella() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        // Features follows the Code umbrella.
        .with_step(open_settings_page(SettingsSection::Features))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        .with_step(press_settings_nav_up())
        .with_step(assert_settings_section(
            SettingsSection::EditorAndCodeReview,
        ))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, true))
}

/// Collapsing an umbrella while one of its subpages is selected keeps arrow
/// navigation anchored to the umbrella's position in the nav order, instead of
/// falling back to the top of the list.
pub fn test_settings_keyboard_navigation_after_manual_collapse() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::EditorAndCodeReview))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, true))
        // Collapse the umbrella while still viewing one of its subpages.
        .with_step(click_settings_umbrella(CODE_UMBRELLA))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        .with_step(assert_settings_section(
            SettingsSection::EditorAndCodeReview,
        ))
        // Down should continue past the umbrella, not restart from the top.
        .with_step(press_settings_nav_down())
        .with_step(assert_settings_section(SettingsSection::Features))
}

// ---------------------------------------------------------------------------
// Search filtering
// ---------------------------------------------------------------------------

/// A query that only matches a top-level page hides the other top-level rows
/// and moves the selection onto the surviving page.
pub fn test_settings_search_filters_top_level_pages() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::Appearance))
        .with_step(type_settings_search("keyboard shortcut"))
        .with_step(assert_settings_nav_page_visible(
            SettingsSection::Keybindings,
            true,
        ))
        .with_step(assert_settings_nav_page_visible(
            SettingsSection::About,
            false,
        ))
        // Account no longer matches, so the selection follows the filter.
        .with_step(assert_settings_section(SettingsSection::Keybindings))
}

/// A query that only matches one umbrella subpage auto-expands the umbrella,
/// hides its sibling subpages, and selects the surviving one.
pub fn test_settings_search_filters_subpages() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::Appearance))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        .with_step(type_settings_search("language server"))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, true))
        .with_step(assert_settings_nav_subpage_visible(
            SettingsSection::Projects,
            true,
        ))
        .with_step(assert_settings_nav_subpage_visible(
            SettingsSection::EditorAndCodeReview,
            false,
        ))
        .with_step(assert_settings_section(SettingsSection::Projects))
}

/// A search that matches only one subpage must still render that subpage's
/// content, not an empty pane.
///
/// Sidebar visibility and content rendering are decided separately, so a page
/// can keep its row while the content pane renders nothing. No sidebar-only
/// assertion would catch that, which is why this asserts on a widget.
pub fn test_settings_search_subpage_still_renders_content() -> Builder {
    let (_, widget_id) = settings_widget_deeplink_target("global_hotkey")
        .expect("global_hotkey must resolve to the existing settings widget");

    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::Appearance))
        // The global hotkey widget has not rendered yet.
        .with_step(assert_settings_widget_rendered(widget_id, false))
        .with_step(type_settings_search("global hotkey"))
        .with_step(assert_settings_section(SettingsSection::Features))
        .with_step(assert_settings_widget_rendered(widget_id, true))
}

/// Clearing the search restores the umbrella expansion state the user had
/// before searching, rather than leaving auto-expanded umbrellas open.
pub fn test_settings_search_clear_restores_umbrella_state() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::Appearance))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        .with_step(type_settings_search("language server"))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, true))
        .with_step(clear_settings_search())
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, false))
        .with_step(assert_settings_nav_page_visible(
            SettingsSection::About,
            true,
        ))
}

/// Clicking a sidebar row while a search is active keeps the query, so the
/// user does not lose their filter by navigating within the results.
pub fn test_settings_search_preserved_on_sidebar_click() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(open_settings_page(SettingsSection::Appearance))
        .with_step(type_settings_search("code"))
        .with_step(assert_umbrella_expanded(CODE_UMBRELLA, true))
        .with_step(click_settings_nav_subpage(SettingsSection::Projects))
        .with_step(assert_settings_section(SettingsSection::Projects))
        // The query survives the click, and so does the filtered sidebar.
        .with_step(assert_settings_nav_page_visible(
            SettingsSection::About,
            false,
        ))
}
