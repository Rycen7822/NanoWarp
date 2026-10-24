use std::path::PathBuf;

use ai::skills::SkillReference;
use settings::Setting;
use warp_util::local_or_remote_path::LocalOrRemotePath;
#[cfg(feature = "local_fs")]
use warp_util::path::LineAndColumnArg;
use warpui::{App, SingletonEntity};

#[cfg(feature = "local_fs")]
use super::{AIBlockEvent, open_code_action_event};
use super::{
    CollapsibleElementState, CollapsibleExpansionState, UserAvatarInfo,
    user_avatar_info_for_conversation_creator,
};
use crate::auth::UserUid;
#[cfg(feature = "local_fs")]
use crate::code::editor_management::CodeSource;
use crate::settings::AISettings;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::workspaces::user_profiles::{UserProfileWithUID, UserProfiles};

#[test]
fn reasoning_auto_collapses_when_user_has_not_manually_toggled() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        let mut state = CollapsibleElementState::default();
        app.update(|ctx| {
            state.finish_reasoning(ctx);
        });

        assert!(matches!(
            state.expansion_state,
            CollapsibleExpansionState::Collapsed
        ));
    });
}

#[test]
fn collapsed_initializer_starts_collapsed() {
    let state = CollapsibleElementState::collapsed();

    assert!(matches!(
        state.expansion_state,
        CollapsibleExpansionState::Collapsed
    ));
}

#[cfg(feature = "local_fs")]
#[test]
fn open_code_action_routes_links_to_configured_editor_and_non_links_to_warp() {
    let linked_source = CodeSource::Link {
        path: PathBuf::from("/workspace/project/src/main.rs"),
        range_start: Some(LineAndColumnArg {
            line_num: 42,
            column_num: Some(7),
        }),
        range_end: None,
    };

    assert!(matches!(
        open_code_action_event(
            &linked_source,
            crate::util::file::external_editor::settings::EditorLayout::SplitPane,
        ),
        AIBlockEvent::OpenDetectedFilePath {
            absolute_path,
            line_and_column_num: Some(LineAndColumnArg {
                line_num: 42,
                column_num: Some(7),
            }),
            target_override: None,
        } if absolute_path.as_path() == std::path::Path::new("/workspace/project/src/main.rs")
    ));

    let skill_source = CodeSource::Skill {
        reference: SkillReference::Path(LocalOrRemotePath::Local(PathBuf::from(
            "/workspace/project/.warp/skills/example/SKILL.md",
        ))),
        location: LocalOrRemotePath::Local(PathBuf::from(
            "/workspace/project/.warp/skills/example/SKILL.md",
        )),
        origin: crate::ai::skills::SkillOpenOrigin::ReadSkill,
    };

    assert!(matches!(
        open_code_action_event(
            &skill_source,
            crate::util::file::external_editor::settings::EditorLayout::NewTab,
        ),
        AIBlockEvent::OpenCodeInWarp {
            source,
            layout: crate::util::file::external_editor::settings::EditorLayout::NewTab,
        } if source == skill_source
    ));
}
#[test]
fn always_show_thinking_stays_expanded_after_finish() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .thinking_display_mode
                .set_value(crate::settings::ThinkingDisplayMode::AlwaysShow, ctx)
                .unwrap();
        });

        let mut state = CollapsibleElementState::default();
        app.update(|ctx| {
            state.finish_reasoning(ctx);
        });

        assert!(matches!(
            state.expansion_state,
            CollapsibleExpansionState::Expanded {
                is_finished: true,
                scroll_pinned_to_bottom: false
            }
        ));
    });
}

#[test]
fn manual_collapse_while_streaming_stays_collapsed_after_finish() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        let mut state = CollapsibleElementState::default();

        state.toggle_expansion();
        app.update(|ctx| {
            state.finish_reasoning(ctx);
        });

        assert!(matches!(
            state.expansion_state,
            CollapsibleExpansionState::Collapsed
        ));
    });
}

#[test]
fn manual_reexpand_while_streaming_stays_expanded_after_finish() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        let mut state = CollapsibleElementState::default();

        state.toggle_expansion();
        state.toggle_expansion();
        app.update(|ctx| {
            state.finish_reasoning(ctx);
        });

        assert!(matches!(
            state.expansion_state,
            CollapsibleExpansionState::Expanded {
                is_finished: true,
                scroll_pinned_to_bottom: false
            }
        ));
    });
}

#[test]
fn user_avatar_info_prefers_conversation_creator_profile() {
    App::test((), |app| async move {
        let creator = UserProfileWithUID {
            firebase_uid: UserUid::new("creator-uid"),
            display_name: Some("Creator Name".to_string()),
            email: "creator@example.com".to_string(),
            photo_url: "https://example.com/creator.png".to_string(),
        };
        let fallback = UserAvatarInfo {
            display_name: "Current User".to_string(),
            profile_image_path: Some("https://example.com/current.png".to_string()),
        };

        app.read(|ctx| {
            let avatar_info = user_avatar_info_for_conversation_creator(
                Some(&creator),
                Some("fallback-uid"),
                fallback,
                ctx,
            );

            assert_eq!(avatar_info.display_name, "Creator Name");
            assert_eq!(
                avatar_info.profile_image_path.as_deref(),
                Some("https://example.com/creator.png")
            );
        });
    });
}

#[test]
fn user_avatar_info_uses_cached_profile_for_creator_uid() {
    App::test((), |app| async move {
        app.add_singleton_model(|_| {
            UserProfiles::new(vec![UserProfileWithUID {
                firebase_uid: UserUid::new("creator-uid"),
                display_name: Some("Cached Creator".to_string()),
                email: "cached@example.com".to_string(),
                photo_url: "https://example.com/cached.png".to_string(),
            }])
        });
        let fallback = UserAvatarInfo {
            display_name: "Current User".to_string(),
            profile_image_path: Some("https://example.com/current.png".to_string()),
        };

        app.read(|ctx| {
            let avatar_info =
                user_avatar_info_for_conversation_creator(None, Some("creator-uid"), fallback, ctx);

            assert_eq!(avatar_info.display_name, "Cached Creator");
            assert_eq!(
                avatar_info.profile_image_path.as_deref(),
                Some("https://example.com/cached.png")
            );
        });
    });
}

#[test]
fn should_show_agent_mode_ask_user_question_speedbump_defaults_to_true() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        AISettings::handle(&app).read(&app, |settings, _ctx| {
            assert!(*settings.should_show_agent_mode_ask_user_question_speedbump);
        });
    });
}

#[test]
fn should_show_agent_mode_ask_user_question_speedbump_round_trips_to_false() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .should_show_agent_mode_ask_user_question_speedbump
                .set_value(false, ctx)
                .unwrap();
        });
        AISettings::handle(&app).read(&app, |settings, _ctx| {
            assert!(!*settings.should_show_agent_mode_ask_user_question_speedbump);
        });
    });
}
