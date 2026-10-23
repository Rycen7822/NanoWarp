use std::collections::HashMap;

use chrono::Utc;
use persistence::model::{ConversationUsageMetadata, PRIMARY_AGENT_CATEGORY};
use warp_multi_agent_api as api;
use warpui::{App, SingletonEntity};

use super::*;
use crate::ai::agent::api::ServerConversationToken;
use crate::ai::agent::conversation::{AIAgentHarness, ServerAIConversationMetadata};
use crate::ai::blocklist::history_model::BlocklistAIHistoryModel;
use crate::cloud_object::{Owner, Revision, ServerMetadata, ServerPermissions};
use crate::server::ids::ServerId;
use crate::settings::UsageDisplayUnit;
use crate::test_util::add_window_with_terminal;
use crate::test_util::billing_unit::{set_charge_unit, set_usage_display_unit};
use crate::test_util::terminal::initialize_app_for_terminal_view;
use crate::workspaces::user_workspaces::UserWorkspaces;
use crate::workspaces::workspace::ChargeUnit;

fn native_conversation_metadata() -> ServerAIConversationMetadata {
    ServerAIConversationMetadata {
        title: "Conversation".to_string(),
        working_directory: None,
        harness: AIAgentHarness::Oz,
        usage: ConversationUsageMetadata::default(),
        metadata: ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        },
        creator: None,
        permissions: ServerPermissions {
            space: Owner::mock_current_user(),
            guests: Vec::new(),
            anyone_link_sharing: None,
            permissions_last_updated_ts: Utc::now().into(),
        },
        ambient_agent_task_id: None,
        server_conversation_token: ServerConversationToken::new(
            "server-conversation-token".to_string(),
        ),
        artifacts: Vec::new(),
    }
}

fn charged_usage_metadata()
-> warp_multi_agent_api::response_event::stream_finished::ConversationUsageMetadata {
    use warp_multi_agent_api::response_event::stream_finished;

    stream_finished::ConversationUsageMetadata {
        credits_spent: 4.5,
        total_charges: Some(api::RequestCharges {
            usage_by_category: HashMap::from([(
                PRIMARY_AGENT_CATEGORY.to_string(),
                api::ChargedUsage {
                    direct_api_inference_usage: HashMap::from([(
                        "gpt-5.5".to_string(),
                        api::InferenceUsage {
                            token_count: Some(api::TokenCount {
                                input: 1000,
                                ..Default::default()
                            }),
                            token_cost: Some(api::TokenCost {
                                input_cost_in_cents: 45.0,
                                input_cost_in_credits: 4.5,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )]),
                    ..Default::default()
                },
            )]),
        }),
        ..Default::default()
    }
}

/// The footer's usage tooltip must track real usage events: the
/// server-seeded billed cost makes the conversation count as having usage
/// (so the popover can open), and a later usage event carrying charged usage
/// moves the tooltip's figure.
#[test]
fn agent_footer_usage_tooltip_updates_on_usage_events() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);

        // Events flush when the outermost update finishes, so the usage events
        // are driven at the app level and asserted afterwards.
        let conversation_id = app.update(|ctx| {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |model, ctx| {
                let conversation_id =
                    model.start_new_conversation(terminal.id(), false, false, false, ctx);
                model.set_active_conversation_id(conversation_id, terminal.id(), ctx);
                let mut metadata = native_conversation_metadata();
                metadata.usage.total_billed_cost_in_cents = Some(250.0);
                model.set_server_metadata_for_conversation(conversation_id, metadata, ctx);
                conversation_id
            })
        });

        // Credits mode: a seeded billed cost alone is not a charged-usage
        // figure, so the total is unknown rather than zero.
        let tooltip = terminal.update(&mut app, |view, ctx| {
            let footer = view.input().as_ref(ctx).agent_input_footer().as_ref(ctx);
            footer.usage_tooltip_for_test(ctx)
        });
        assert_eq!(tooltip.as_deref(), Some("Conversation usage: \u{2014}"));

        app.update(|ctx| {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |model, ctx| {
                model.update_conversation_cost_and_usage_for_request(
                    conversation_id,
                    None,
                    None,
                    vec![],
                    Some(charged_usage_metadata()),
                    false,
                    ctx,
                );
            });
        });

        let tooltip = terminal.update(&mut app, |view, ctx| {
            let footer = view.input().as_ref(ctx).agent_input_footer().as_ref(ctx);
            footer.usage_tooltip_for_test(ctx)
        });
        assert_eq!(tooltip.as_deref(), Some("Conversation usage: 4.5 credits"));
    });
}

/// A viewer who prefers dollars sees the tooltip's figure in dollars only once their tier charges
/// in cents, and the tooltip follows a workspaces-metadata refresh that flips the charge unit.
#[test]
fn agent_footer_usage_tooltip_follows_the_tier_charge_unit() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        set_usage_display_unit(&mut app, UsageDisplayUnit::Dollars);
        let terminal = add_window_with_terminal(&mut app, None);

        app.update(|ctx| {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |model, ctx| {
                let conversation_id =
                    model.start_new_conversation(terminal.id(), false, false, false, ctx);
                model.set_active_conversation_id(conversation_id, terminal.id(), ctx);
                model.update_conversation_cost_and_usage_for_request(
                    conversation_id,
                    None,
                    None,
                    vec![],
                    Some(charged_usage_metadata()),
                    false,
                    ctx,
                );
            });
        });
        let tooltip = terminal.update(&mut app, |view, ctx| {
            let footer = view.input().as_ref(ctx).agent_input_footer().as_ref(ctx);
            footer.usage_tooltip_for_test(ctx)
        });
        assert_eq!(tooltip.as_deref(), Some("Conversation usage: 4.5 credits"));

        set_charge_unit(&mut app, ChargeUnit::Cents);
        // A metadata refresh re-applies the workspaces after updating the tier, which is what
        // tells subscribers to re-read it.
        UserWorkspaces::handle(&app).update(&mut app, |workspaces, ctx| {
            let current = workspaces.workspaces().clone();
            workspaces.update_workspaces(current, ctx);
        });
        let tooltip = terminal.update(&mut app, |view, ctx| {
            let footer = view.input().as_ref(ctx).agent_input_footer().as_ref(ctx);
            footer.usage_tooltip_for_test(ctx)
        });
        assert_eq!(tooltip.as_deref(), Some("Conversation usage: $0.45"));
    });
}

/// Opening the popover makes the footer consult the terminal's menu positioning provider,
/// which locks the terminal model. Rendering must not still be holding that lock.
#[test]
fn agent_footer_usage_popover_renders_with_terminal_positioning_provider() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        set_charge_unit(&mut app, ChargeUnit::Cents);
        let terminal = add_window_with_terminal(&mut app, None);

        app.update(|ctx| {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |model, ctx| {
                let conversation_id =
                    model.start_new_conversation(terminal.id(), false, false, false, ctx);
                model.set_active_conversation_id(conversation_id, terminal.id(), ctx);
                model.update_conversation_cost_and_usage_for_request(
                    conversation_id,
                    None,
                    None,
                    vec![],
                    Some(charged_usage_metadata()),
                    false,
                    ctx,
                );
            });
        });

        let footer = terminal.read(&app, |view, ctx| {
            view.input().as_ref(ctx).agent_input_footer().clone()
        });
        footer.read(&app, |footer, ctx| {
            let child_ids = footer.render(ctx).debug_child_view_ids();
            assert!(child_ids.contains(&footer.usage_button.id()));
            assert!(!child_ids.contains(&footer.usage_popover.id()));
        });

        footer.update(&mut app, |footer, ctx| {
            footer.handle_action(&AgentInputFooterAction::ToggleUsagePopover, ctx);
        });

        footer.read(&app, |footer, ctx| {
            assert!(
                footer
                    .render(ctx)
                    .debug_child_view_ids()
                    .contains(&footer.usage_popover.id()),
            );
        });
    });
}
