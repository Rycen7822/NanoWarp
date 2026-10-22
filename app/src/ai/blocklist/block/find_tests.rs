use std::collections::HashMap;
use std::sync::Arc;

use super::compute_find_matches;
use crate::ai::agent::{AIAgentInput, UserQueryMode};
use crate::terminal::find::FindOptions;

fn user_query_input(query: &str, mode: UserQueryMode) -> AIAgentInput {
    AIAgentInput::UserQuery {
        query: query.to_string(),
        context: Arc::new([]),
        static_query_type: None,
        referenced_attachments: HashMap::new(),
        user_query_mode: mode,
        running_command: None,
        intended_agent: None,
        base: None,
    }
}

// Find highlights must align with the displayed query after prefix decoration is removed.
#[test]
fn find_matches_align_with_initial_query_text() {
    let query = "list files".to_string();
    let input = user_query_input(&query, UserQueryMode::Normal);

    let displayed = input
        .display_query()
        .expect("user query should produce displayed text");
    assert_eq!(displayed, "list files");

    let options = FindOptions::default().with_query(Some("list".to_string()));

    // The match starts at the first displayed character.
    assert_eq!(compute_find_matches(&displayed, &options), vec![0..4]);
}

// Transport mode metadata does not add text ahead of the displayed query.
#[test]
fn find_matches_align_with_query_text_without_mode_prefixes() {
    let input = user_query_input("write tests", UserQueryMode::Plan);

    let displayed = input
        .display_query()
        .expect("user query should produce displayed text");
    assert_eq!(displayed, "write tests");

    let options = FindOptions::default().with_query(Some("tests".to_string()));
    assert_eq!(compute_find_matches(&displayed, &options), vec![6..11]);
}
