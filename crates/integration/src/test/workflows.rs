use std::time::Duration;

use warp::integration_testing::assertions::{go_offline, go_online, join_a_workspace};
use warp::integration_testing::command_palette::{
    TestStepsExt, open_command_palette_and_run_action,
};
use warp::integration_testing::terminal::wait_until_bootstrapped_single_pane_for_tab;
use warp::integration_testing::window::save_active_window_id;
use warp::integration_testing::workflow::{
    assert_no_team_workflow_pane_open, assert_no_workflow_pane_open,
    assert_open_team_workflow_pane_count_equals, assert_open_workflow_pane_count_equals,
    assert_workflow_id, create_a_personal_workflow, open_workflow,
};
use warpui_core::integration::TestStep;

use super::new_builder;
use crate::Builder;

pub fn test_open_workflow_in_pane() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(
            create_a_personal_workflow("workflow_2_key")
                .add_assertion(save_active_window_id("first window")),
        )
        .with_step(
            open_workflow("first window", "workflow_2_key")
                .add_named_assertion_with_data_from_prior_step(
                    "Verify workflow is open",
                    assert_workflow_id(0, 0, "workflow_2_key"),
                ),
        )
}

pub fn test_create_personal_workflow_pane_from_command_palette() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(TestStep::new("Noop step").add_named_assertion(
            "Make sure no workflow panes are open",
            assert_no_workflow_pane_open(),
        ))
        .with_steps(
            open_command_palette_and_run_action("Create a New Personal Workflow")
                .add_named_assertion(
                    "There should be one workflow pane open",
                    assert_open_workflow_pane_count_equals(1),
                ),
        )
}

pub fn test_create_team_workflow_pane_from_command_palette() -> Builder {
    new_builder()
        .with_step(wait_until_bootstrapped_single_pane_for_tab(0))
        .with_step(TestStep::new("Noop step").add_named_assertion(
            "Make sure no workflow panes are open",
            assert_no_workflow_pane_open(),
        ))
        .with_step(join_a_workspace())
        .with_step(go_offline())
        .with_step(
            TestStep::new("delay for test consistency")
                .set_post_step_pause(Duration::from_millis(250)),
        )
        .with_steps(
            open_command_palette_and_run_action("Create a New Team Workflow").add_named_assertion(
                "There should still not be any panes open",
                assert_no_team_workflow_pane_open(),
            ),
        )
        .with_step(go_online())
        .with_steps(
            open_command_palette_and_run_action("Create a New Team Workflow").add_named_assertion(
                "There should be an open workflow pane",
                assert_open_team_workflow_pane_count_equals(1),
            ),
        )
}
