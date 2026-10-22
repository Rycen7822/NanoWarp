use crate::search::mixer::SearchMixer;

pub type CommandSearchMixer = SearchMixer<CommandSearchItemAction>;

/// Actions produced by selecting or explicitly executing a command history result.
#[derive(Clone, Debug)]
pub enum CommandSearchItemAction {
    AcceptHistory(String),
    ExecuteHistory(String),
}

#[cfg(test)]
#[path = "searcher_tests.rs"]
mod tests;
