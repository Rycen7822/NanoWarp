use warp_graphql::ai::AgentTaskState;

use super::{LocalTaskUpdate, LocalTaskUpdateQueue};
use crate::ai::ambient_agents::AmbientAgentTaskId;

fn task_id() -> AmbientAgentTaskId {
    "550e8400-e29b-41d4-a716-446655440a00".parse().unwrap()
}

fn progress_update() -> LocalTaskUpdate {
    LocalTaskUpdate {
        task_state: Some(AgentTaskState::InProgress),
        ..Default::default()
    }
}

#[test]
fn normal_progress_remains_deduplicated() {
    let mut queue = LocalTaskUpdateQueue::default();
    assert!(queue.enqueue(task_id(), progress_update()).is_some());
    assert!(queue.record_result(task_id(), true).is_none());
    assert!(queue.enqueue(task_id(), progress_update()).is_none());
}
