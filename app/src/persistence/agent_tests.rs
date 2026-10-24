use std::collections::HashMap;

use chrono::{NaiveDate, NaiveDateTime};
use diesel_migrations::MigrationHarness;
use prost::Message as _;

use super::*;
use crate::ai::agent::conversation::{AIConversation, AIConversationId};

/// Builds an in-memory SQLite database with all migrations applied.
fn test_connection() -> SqliteConnection {
    let mut conn =
        SqliteConnection::establish(":memory:").expect("in-memory sqlite connection should open");
    conn.run_pending_migrations(::persistence::MIGRATIONS)
        .expect("migrations should run");
    conn
}

fn task_with_user_query(task_id: &str, query: &str, description: &str) -> api::Task {
    api::Task {
        id: task_id.to_string(),
        description: description.to_string(),
        dependencies: None,
        messages: vec![api::Message {
            id: format!("{task_id}-user-query"),
            task_id: task_id.to_string(),
            message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                query: query.to_string(),
                ..Default::default()
            })),
            ..Default::default()
        }],
        summary: String::new(),
        server_data: String::new(),
    }
}

fn empty_conversation_data() -> AgentConversationData {
    serde_json::from_str(r#"{"server_conversation_token":null}"#)
        .expect("minimal conversation data should deserialize")
}

fn summary_column(conn: &mut SqliteConnection, conversation: &str) -> Option<String> {
    use schema::agent_conversations::dsl::*;
    agent_conversations
        .filter(conversation_id.eq(conversation))
        .select(summary)
        .first::<Option<String>>(conn)
        .expect("conversation row should exist")
}

fn last_modified_column(conn: &mut SqliteConnection, conversation: &str) -> NaiveDateTime {
    use schema::agent_conversations::dsl::*;
    agent_conversations
        .filter(conversation_id.eq(conversation))
        .select(last_modified_at)
        .first::<NaiveDateTime>(conn)
        .expect("conversation row should exist")
}

#[test]
fn upsert_writes_summary_and_metadata_read_skips_tasks() {
    let mut conn = test_connection();
    let task = task_with_user_query("task-1", "Initial query", "Root title");
    upsert_agent_conversation(&mut conn, "conv-1", [&task], empty_conversation_data())
        .expect("upsert should succeed");

    let (conversations, backfills) =
        read_agent_conversation_metadata(&mut conn).expect("metadata read should succeed");

    assert!(
        backfills.is_empty(),
        "rows written with a summary must not be backfilled"
    );
    assert_eq!(conversations.len(), 1);
    assert!(
        conversations[0].tasks.is_empty(),
        "metadata read must not hydrate task payloads"
    );
    let summary: AgentConversationSummary = serde_json::from_str(
        conversations[0]
            .conversation
            .summary
            .as_deref()
            .expect("summary column should be written at upsert time"),
    )
    .expect("summary column should hold valid summary JSON");
    assert_eq!(summary.initial_query, "Initial query");
    assert_eq!(summary.title, "Root title");
    assert!(summary.is_restorable);
}
#[test]
fn request_metadata_round_trips_through_conversation_and_persistence() {
    let request_metadata = api::message::RequestMetadata {
        timing: Some(api::RequestTiming {
            request_timespan: Some(api::TimeSpan {
                started_at: Some(prost_types::Timestamp {
                    seconds: 1_000,
                    nanos: 100,
                }),
                ended_at: Some(prost_types::Timestamp {
                    seconds: 1_010,
                    nanos: 200,
                }),
            }),
            first_token_at: Some(prost_types::Timestamp {
                seconds: 1_002,
                nanos: 300,
            }),
            llm_generation_timespans: vec![api::TimeSpan {
                started_at: Some(prost_types::Timestamp {
                    seconds: 1_001,
                    nanos: 400,
                }),
                ended_at: Some(prost_types::Timestamp {
                    seconds: 1_009,
                    nanos: 500,
                }),
            }],
        }),
        charges: Some(api::RequestCharges {
            usage_by_category: HashMap::from([(
                "primary_agent".to_string(),
                api::ChargedUsage {
                    direct_api_inference_usage: HashMap::from([(
                        "model".to_string(),
                        api::InferenceUsage {
                            token_count: Some(api::TokenCount {
                                input: 100,
                                output: 20,
                                input_cache_read: 30,
                                input_cache_write: 40,
                            }),
                            token_cost: Some(api::TokenCost {
                                input_cost_in_cents: 1.0,
                                output_cost_in_cents: 2.0,
                                input_cache_read_cost_in_cents: 3.0,
                                input_cache_write_cost_in_cents: 4.0,
                                input_cost_in_credits: 1.5,
                                output_cost_in_credits: 2.5,
                                input_cache_read_cost_in_credits: 3.5,
                                input_cache_write_cost_in_credits: 4.5,
                            }),
                            web_search_count: 2,
                            web_search_cost_in_cents: 5.0,
                            web_search_cost_in_credits: 5.5,
                            ..Default::default()
                        },
                    )]),
                    byok_inference_usage: HashMap::new(),
                    custom_endpoint_inference_usage: HashMap::new(),
                    platform_usage_in_cents: 6.0,
                    platform_usage_duration: Some(prost_types::Duration {
                        seconds: 7,
                        nanos: 600,
                    }),
                    platform_usage_in_credits: 6.5,
                },
            )]),
        }),
        incomplete: false,
        outcome: api::message::request_metadata::Outcome::Completed as i32,
        tool_call_summary: Some(api::message::request_metadata::ToolCallSummary {
            tool_calls: 8,
            commands_executed: 9,
            files_changed: 10,
            lines_added: 11,
            lines_removed: 12,
        }),
        context_window: Some(api::message::request_metadata::ContextWindow { usage: 0.75 }),
    };
    let mut task = task_with_user_query("task-metadata", "Inspect metadata", "Root title");
    task.messages.push(api::Message {
        id: "request-metadata".to_string(),
        task_id: task.id.clone(),
        request_id: "request-1".to_string(),
        message: Some(api::message::Message::RequestMetadata(request_metadata)),
        ..Default::default()
    });

    let decoded_task =
        api::Task::decode(task.encode_to_vec().as_slice()).expect("task should decode");
    let conversation =
        AIConversation::new_restored(AIConversationId::new(), vec![decoded_task], None)
            .expect("task should apply to a restored conversation");
    let applied_task = conversation
        .all_tasks()
        .next()
        .and_then(|task| task.source())
        .expect("restored task should retain its API source")
        .clone();
    assert_eq!(applied_task, task);

    let mut conn = test_connection();
    upsert_agent_conversation(
        &mut conn,
        "conv-metadata",
        [&applied_task],
        empty_conversation_data(),
    )
    .expect("upsert should succeed");
    let restored = read_agent_conversation_by_id(&mut conn, "conv-metadata")
        .expect("persistence read should succeed")
        .expect("conversation should exist");
    assert_eq!(restored.tasks, vec![task.clone()]);

    let echoed_task = api::Task::decode(restored.tasks[0].encode_to_vec().as_slice())
        .expect("persisted task should encode for the next request");
    assert_eq!(echoed_task, task);
}

#[test]
fn metadata_read_derives_and_backfill_persists_summary_for_legacy_rows() {
    use schema::agent_conversations::dsl::*;

    let mut conn = test_connection();
    let task = task_with_user_query("task-1", "Initial query", "Root title");
    upsert_agent_conversation(&mut conn, "conv-1", [&task], empty_conversation_data())
        .expect("upsert should succeed");

    // Simulate a row written before the summary column existed. Setting
    // `last_modified_at` explicitly keeps the update trigger from bumping it.
    let legacy_ts = ts(1_000);
    diesel::update(agent_conversations.filter(conversation_id.eq("conv-1")))
        .set((summary.eq(None::<String>), last_modified_at.eq(legacy_ts)))
        .execute(&mut conn)
        .expect("legacy row setup should succeed");

    let (conversations, backfills) =
        read_agent_conversation_metadata(&mut conn).expect("metadata read should succeed");

    // The read derives the summary from the row's own task snapshot.
    assert_eq!(conversations.len(), 1);
    let derived: AgentConversationSummary = serde_json::from_str(
        conversations[0]
            .conversation
            .summary
            .as_deref()
            .expect("legacy rows should get a read-time-derived summary"),
    )
    .expect("derived summary should be valid JSON");
    assert_eq!(derived.initial_query, "Initial query");

    // ... and queues a backfill preserving the original timestamp.
    assert_eq!(backfills.len(), 1);
    assert_eq!(backfills[0].conversation_id, "conv-1");
    assert_eq!(backfills[0].last_modified_at, legacy_ts);

    backfill_conversation_summaries(&mut conn, backfills).expect("backfill should succeed");

    assert!(
        summary_column(&mut conn, "conv-1").is_some(),
        "backfill must persist the derived summary"
    );
    assert_eq!(
        last_modified_column(&mut conn, "conv-1"),
        legacy_ts,
        "backfill must not reorder history by bumping last_modified_at"
    );

    // Subsequent startups stay metadata-only.
    let (_, backfills) =
        read_agent_conversation_metadata(&mut conn).expect("metadata read should succeed");
    assert!(backfills.is_empty());
}

#[test]
fn backfill_never_overwrites_a_newer_summary() {
    let mut conn = test_connection();
    let task = task_with_user_query("task-1", "Initial query", "Root title");
    upsert_agent_conversation(&mut conn, "conv-1", [&task], empty_conversation_data())
        .expect("upsert should succeed");
    let written_summary = summary_column(&mut conn, "conv-1");
    let written_ts = last_modified_column(&mut conn, "conv-1");

    // Stale backfills (computed before a newer write landed) must not
    // clobber the row's summary or timestamp, regardless of whether the
    // reader observed a NULL or a since-replaced invalid value.
    let stale_from_null = ConversationSummaryBackfill {
        conversation_id: "conv-1".to_string(),
        summary_json: "{\"stale\":true}".to_string(),
        previous_summary: None,
        last_modified_at: ts(1),
    };
    let stale_from_invalid = ConversationSummaryBackfill {
        conversation_id: "conv-1".to_string(),
        summary_json: "{\"stale\":true}".to_string(),
        previous_summary: Some("{not valid json".to_string()),
        last_modified_at: ts(1),
    };
    backfill_conversation_summaries(&mut conn, vec![stale_from_null, stale_from_invalid])
        .expect("backfill should succeed");

    assert_eq!(summary_column(&mut conn, "conv-1"), written_summary);
    assert_eq!(last_modified_column(&mut conn, "conv-1"), written_ts);
}

#[test]
fn metadata_read_heals_invalid_non_null_summaries() {
    use schema::agent_conversations::dsl::*;

    let mut conn = test_connection();
    let task = task_with_user_query("task-1", "Initial query", "Root title");
    upsert_agent_conversation(&mut conn, "conv-1", [&task], empty_conversation_data())
        .expect("upsert should succeed");

    // Corrupt the summary with unparseable JSON.
    let legacy_ts = ts(1_000);
    diesel::update(agent_conversations.filter(conversation_id.eq("conv-1")))
        .set((
            summary.eq(Some("{not valid json")),
            last_modified_at.eq(legacy_ts),
        ))
        .execute(&mut conn)
        .expect("corrupt summary setup should succeed");

    let (_, backfills) =
        read_agent_conversation_metadata(&mut conn).expect("metadata read should succeed");
    assert_eq!(backfills.len(), 1);
    assert_eq!(
        backfills[0].previous_summary.as_deref(),
        Some("{not valid json"),
        "the backfill must carry the observed invalid value for its compare-and-set"
    );

    backfill_conversation_summaries(&mut conn, backfills).expect("backfill should succeed");

    // The invalid summary healed in place and history order is preserved.
    let healed: AgentConversationSummary = serde_json::from_str(
        summary_column(&mut conn, "conv-1")
            .as_deref()
            .expect("summary should be present after healing"),
    )
    .expect("healed summary should be valid JSON");
    assert_eq!(healed.initial_query, "Initial query");
    assert_eq!(last_modified_column(&mut conn, "conv-1"), legacy_ts);

    // Subsequent startups stay metadata-only.
    let (_, backfills) =
        read_agent_conversation_metadata(&mut conn).expect("metadata read should succeed");
    assert!(backfills.is_empty());
}

fn ts(secs_from_epoch: i64) -> NaiveDateTime {
    // 2026-01-01 baseline keeps failure messages readable.
    NaiveDate::from_ymd_opt(2026, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        + chrono::Duration::seconds(secs_from_epoch)
}

fn make_row(id: i32, conversation_id: &str, secs: i64) -> AgentConversationRecord {
    AgentConversationRecord {
        id,
        conversation_id: conversation_id.to_string(),
        conversation_data: r#"{"server_conversation_token":null}"#.to_string(),
        last_modified_at: ts(secs),
        summary: None,
    }
}

/// Row count ≤ limit ⇒ no eviction.
#[test]
fn prune_is_no_op_when_under_limit() {
    let rows = vec![
        make_row(1, "a", 100),
        make_row(2, "b", 200),
        make_row(3, "c", 300),
    ];
    assert!(select_conversations_to_evict(&rows, 3).is_empty());
    assert!(select_conversations_to_evict(&rows, 100).is_empty());
}

#[test]
fn keeps_newest_conversations_and_evicts_oldest() {
    let mut rows = vec![
        make_row(1, "recent1", 100),
        make_row(2, "recent2", 500),
        make_row(3, "recent3", 1000),
        make_row(4, "recent4", 1500),
        make_row(5, "recent5", 2000),
    ];
    for i in 0_i32..9 {
        let id = format!("s{i}");
        rows.push(make_row(100 + i, &id, 10 + i64::from(i)));
    }
    let evicted = select_conversations_to_evict(&rows, 13);
    assert_eq!(evicted.len(), 1, "evicted={evicted:?}");
    assert_eq!(evicted[0], "s0", "must evict the oldest conversation");
}

/// Same input twice produces the same output. Tie-broken by conversation ID ASC.
#[test]
fn eviction_is_deterministic() {
    let rows = vec![
        make_row(1, "a", 100),
        make_row(2, "b", 100),
        make_row(3, "c", 100),
        make_row(4, "d", 100),
    ];
    let e1 = select_conversations_to_evict(&rows, 2);
    let e2 = select_conversations_to_evict(&rows, 2);
    assert_eq!(e1, e2);
    assert_eq!(e1, vec!["c".to_string(), "d".to_string()]);
}
