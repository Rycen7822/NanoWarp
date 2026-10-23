use std::collections::HashMap;

use crate::schema;

/// Merges warp and byok per-model token usage rows into the persistence-layer
/// `ModelTokenUsage` shape, preserving per-category breakdowns for the
/// conversation restore path (`crate::ai`).
pub(crate) fn convert_token_usage(
    warp_token_usage: &[TokenUsage],
    byok_token_usage: &[TokenUsage],
) -> Vec<persistence::model::ModelTokenUsage> {
    let mut usage_by_model: HashMap<String, persistence::model::ModelTokenUsage> = HashMap::new();

    for usage in warp_token_usage {
        let entry = usage_by_model
            .entry(usage.model_id.clone())
            .or_insert_with(|| persistence::model::ModelTokenUsage {
                model_id: usage.model_id.clone(),
                ..Default::default()
            });
        entry.warp_tokens += u32::try_from(usage.total_tokens).unwrap_or_default();
        for category_breakdown in &usage.token_usage_by_category {
            *entry
                .warp_token_usage_by_category
                .entry(category_breakdown.category.clone())
                .or_default() += u32::try_from(category_breakdown.tokens).unwrap_or_default();
        }
    }

    for usage in byok_token_usage {
        let entry = usage_by_model
            .entry(usage.model_id.clone())
            .or_insert_with(|| persistence::model::ModelTokenUsage {
                model_id: usage.model_id.clone(),
                ..Default::default()
            });
        entry.byok_tokens += u32::try_from(usage.total_tokens).unwrap_or_default();
        for category_breakdown in &usage.token_usage_by_category {
            *entry
                .byok_token_usage_by_category
                .entry(category_breakdown.category.clone())
                .or_default() += u32::try_from(category_breakdown.tokens).unwrap_or_default();
        }
    }

    let mut result: Vec<_> = usage_by_model.into_values().collect();
    result.sort_by(|a, b| a.model_id.cmp(&b.model_id));
    result
}

/// Combines the server's cumulative billed inference cents with its platform cents into the
/// single total the client displays, matching what `ChargedUsageTotals::total_cost_in_cents`
/// sums from the per-turn wire charges. `None` when the billed inference total is unknown,
/// since a platform-only figure would understate the conversation's cost.
pub(crate) fn total_billed_cost_in_cents(
    billed_inference_cents: Option<f64>,
    platform_cents: Option<f64>,
) -> Option<f32> {
    billed_inference_cents.map(|billed| (billed + platform_cents.unwrap_or_default()) as f32)
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct TokenUsage {
    pub model_id: String,
    pub total_tokens: i32,
    pub token_usage_by_category: Vec<CategoryTokenBreakdown>,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct CategoryTokenBreakdown {
    pub category: String,
    pub tokens: i32,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct ToolCallStats {
    pub count: i32,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct ApplyFileDiffStats {
    pub count: i32,
    pub lines_added: i32,
    pub lines_removed: i32,
    pub files_changed: i32,
}

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct ToolUsageMetadata {
    pub run_command_stats: ToolCallStats,
    pub run_commands_executed: i32,
    pub read_files_stats: ToolCallStats,
    pub search_codebase_stats: ToolCallStats,
    pub grep_stats: ToolCallStats,
    pub file_glob_stats: ToolCallStats,
    pub suggest_plan_stats: ToolCallStats,
    pub suggest_create_plan_stats: ToolCallStats,
    pub write_to_long_running_shell_command_stats: ToolCallStats,
    pub apply_file_diff_stats: ApplyFileDiffStats,
    pub read_shell_command_output_stats: ToolCallStats,
}

impl From<&ToolUsageMetadata> for persistence::model::ToolUsageMetadata {
    fn from(gql: &ToolUsageMetadata) -> Self {
        Self {
            run_command_stats: persistence::model::RunCommandStats {
                count: gql.run_command_stats.count,
                commands_executed: gql.run_commands_executed,
            },
            read_files_stats: persistence::model::ToolCallStats {
                count: gql.read_files_stats.count,
            },
            search_codebase_stats: persistence::model::ToolCallStats {
                count: gql.search_codebase_stats.count,
            },
            grep_stats: persistence::model::ToolCallStats {
                count: gql.grep_stats.count,
            },
            file_glob_stats: persistence::model::ToolCallStats {
                count: gql.file_glob_stats.count,
            },
            apply_file_diff_stats: persistence::model::ApplyFileDiffStats {
                count: gql.apply_file_diff_stats.count,
                lines_added: gql.apply_file_diff_stats.lines_added,
                lines_removed: gql.apply_file_diff_stats.lines_removed,
                files_changed: gql.apply_file_diff_stats.files_changed,
            },
            write_to_long_running_shell_command_stats: persistence::model::ToolCallStats {
                count: gql.write_to_long_running_shell_command_stats.count,
            },
            suggest_plan_stats: persistence::model::ToolCallStats {
                count: gql.suggest_plan_stats.count,
            },
            suggest_create_plan_stats: persistence::model::ToolCallStats {
                count: gql.suggest_create_plan_stats.count,
            },
            read_shell_command_output_stats: persistence::model::ToolCallStats {
                count: gql.read_shell_command_output_stats.count,
            },
        }
    }
}
