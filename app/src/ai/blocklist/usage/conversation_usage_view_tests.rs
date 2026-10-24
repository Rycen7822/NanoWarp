use std::collections::HashMap;

use warp_core::ui::appearance::Appearance;
use warpui::App;

use super::*;
use crate::persistence::model::{ModelTokenUsage, PRIMARY_AGENT_CATEGORY};
use crate::settings::UsageDisplayUnit;
use crate::test_util::billing_unit::{set_charge_unit, set_usage_display_unit};
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::ChargeUnit;

fn placeholder_usage_info() -> ConversationUsageInfo {
    ConversationUsageInfo {
        credits_spent: 0.0,
        platform_credits_spent: 0.0,
        credits_spent_for_last_block: None,
        tool_calls: 0,
        models: Vec::new(),
        context_window_usage: 0.0,
        context_window_segments: Vec::new(),
        files_changed: 0,
        lines_added: 0,
        lines_removed: 0,
        commands_executed: 0,
        total_cost_in_cents: None,
        cost_in_cents_for_last_block: None,
    }
}

/// Registers the singletons that the view touches when constructed and
/// when `ctx.notify()` runs (theme lookups, settings, etc.). Keep this
/// minimal: the goal is to satisfy the runtime, not to mirror the full
/// production app.
fn initialize_test_app(app: &mut App) {
    initialize_settings_for_tests(app);
    app.add_singleton_model(|_| Appearance::mock());
    app.add_singleton_model(UserWorkspaces::default_mock);
}

fn build_view() -> ConversationUsageView {
    ConversationUsageView {
        usage_info: placeholder_usage_info(),
        timing_info: None,
        full_terminal_use_tooltip_mouse_state: MouseStateHandle::default(),
        context_window_expanded: false,
        context_window_toggle_mouse_state: MouseStateHandle::default(),
        context_window_other_tooltip_mouse_state: MouseStateHandle::default(),
    }
}

#[test]
fn custom_endpoint_models_use_the_external_key_icon_bucket() {
    let view = ConversationUsageView {
        usage_info: ConversationUsageInfo {
            models: vec![ModelTokenUsage {
                model_id: "Friendly alias".to_string(),
                custom_endpoint_tokens: 6,
                custom_endpoint_token_usage_by_category: HashMap::from([(
                    PRIMARY_AGENT_CATEGORY.to_string(),
                    6,
                )]),
                ..Default::default()
            }],
            ..placeholder_usage_info()
        },
        ..build_view()
    };

    assert_eq!(
        view.collect_models_by_category()
            .get(PRIMARY_AGENT_CATEGORY),
        Some(&vec![("Friendly alias".to_string(), true)])
    );
}

/// A cents-charged viewer who prefers dollars sees each usage figure in dollars where a cents
/// figure exists and in credits where none does.
#[test]
fn usage_summary_follows_the_display_unit_per_figure() {
    App::test((), |mut app| async move {
        initialize_test_app(&mut app);
        set_charge_unit(&mut app, ChargeUnit::Cents);
        set_usage_display_unit(&mut app, UsageDisplayUnit::Dollars);

        let view = ConversationUsageView {
            usage_info: ConversationUsageInfo {
                credits_spent: 20.0,
                credits_spent_for_last_block: Some(5.0),
                total_cost_in_cents: Some(36.0),
                cost_in_cents_for_last_block: None,
                ..placeholder_usage_info()
            },
            ..build_view()
        };

        app.read(|ctx| {
            let text = view
                .render_unified_layout(ctx)
                .debug_text_content()
                .unwrap_or_default();
            assert!(text.contains("Usage charged (total)"), "got {text:?}");
            assert!(text.contains("$0.36"), "got {text:?}");
            assert!(
                text.contains("Credits spent (last response)"),
                "got {text:?}"
            );
            assert!(text.contains("5 credits"), "got {text:?}");
        });
    });
}
