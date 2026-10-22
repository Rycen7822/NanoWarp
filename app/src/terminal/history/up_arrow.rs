use std::collections::HashSet;

use warpui::{AppContext, SingletonEntity};

use super::{History, HistoryEntry};
use crate::suggestions::ignored_suggestions_model::{IgnoredSuggestionsModel, SuggestionType};
use crate::terminal::model::session::SessionId;

impl History {
    pub(crate) fn up_arrow_suggestions(
        &self,
        session_id: Option<SessionId>,
        app: &AppContext,
    ) -> Vec<&HistoryEntry> {
        let ignored = app
            .has_singleton_model::<IgnoredSuggestionsModel>()
            .then(|| IgnoredSuggestionsModel::as_ref(app));
        let mut commands: Vec<_> = session_id
            .and_then(|id| self.commands(id))
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| {
                ignored.is_none_or(|ignored| {
                    !ignored.is_ignored(&entry.command, SuggestionType::ShellCommand)
                })
            })
            .collect();
        commands.sort_by(|a, b| a.cmp_for_history(b, session_id));
        let mut seen = HashSet::new();
        let mut keep = vec![false; commands.len()];
        for (index, entry) in commands.iter().enumerate().rev() {
            let text = entry.command.trim();
            keep[index] = !text.is_empty() && seen.insert(text);
        }
        commands
            .into_iter()
            .zip(keep)
            .filter_map(|(entry, keep)| keep.then_some(entry))
            .collect()
    }
}

#[cfg(test)]
#[path = "up_arrow_tests.rs"]
mod tests;
