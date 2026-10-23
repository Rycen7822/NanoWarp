use std::collections::{HashMap, HashSet};

use handlebars::parser::{ParsedArgumentResult, ParsedArgumentsIterator};

use crate::workflows::workflow::Argument;

/// Tracks notebook command arguments across edits, retaining descriptions and defaults.
#[derive(Debug, Default)]
pub struct ArgumentsState {
    pub arguments: Vec<Argument>,
    /// Hashmap mapping the word index in the input command string to (index) in the
    /// arguments vector. Each index of the arguments vector should be a hashmap value;
    /// only word indexes with arguments (from input string) should be a hashmap key.
    /// Enables `query_argument_by_name`.
    word_index_to_arg_index_map: HashMap<usize, usize>,
    /// Hashmap mapping the argument name in the input command string to (index) in the
    /// arguments vector. Enables `query_argument_by_word_index`.
    arg_name_to_arg_index_map: HashMap<String, usize>,
    number_of_words: usize,
}

impl ArgumentsState {
    /// Parses distinct valid arguments in first-occurrence order.
    /// Retains prior metadata by word position when the word count is unchanged, or by name otherwise.
    pub fn for_command_workflow(prev_state: &ArgumentsState, input_string: String) -> Self {
        let mut arg_name_word_index_pairs: Vec<(String, usize)> = Vec::new();
        let mut arg_names = HashSet::new();

        let mut arguments_iterator = ParsedArgumentsIterator::new(input_string.chars());

        for argument_result in arguments_iterator.by_ref() {
            if let ParsedArgumentResult::Valid { current_word_index } = argument_result.result() {
                let start_char_index = argument_result.chars_range().start;
                let argument_name_length = argument_result.chars_range().end - start_char_index;
                let argument_name: String = input_string
                    .chars()
                    .skip(start_char_index)
                    .take(argument_name_length)
                    .collect();

                if !arg_names.contains(&argument_name) {
                    arg_name_word_index_pairs.push((argument_name.clone(), *current_word_index));
                    arg_names.insert(argument_name);
                }
            }
        }

        let number_of_words = arguments_iterator.word_count();

        let (arguments, word_index_to_arg_index_map, arg_name_to_arg_index_map) =
            ArgumentsState::build_arguments_and_query_maps(
                prev_state,
                number_of_words != prev_state.number_of_words,
                arg_name_word_index_pairs,
            );

        Self {
            arguments,
            word_index_to_arg_index_map,
            arg_name_to_arg_index_map,
            number_of_words,
        }
    }

    fn build_arguments_and_query_maps(
        prev_state: &ArgumentsState,
        is_insertion_or_deletion: bool,
        arg_name_word_index_pairs: Vec<(String, usize)>,
    ) -> (Vec<Argument>, HashMap<usize, usize>, HashMap<String, usize>) {
        let mut word_index_to_arg_index_map = HashMap::new();
        let mut arg_name_to_arg_index_map = HashMap::new();

        let arguments: Vec<Argument> = arg_name_word_index_pairs
            .iter()
            .enumerate()
            .map(|(arg_index, (name, word_index))| {
                let prev_argument = if is_insertion_or_deletion {
                    prev_state.query_argument_by_name(name)
                } else {
                    prev_state.query_argument_by_word_index(*word_index)
                };

                let argument: Argument = match prev_argument {
                    Some(prev_argument) => {
                        ArgumentsState::new_argument_with_previous_data(name, prev_argument)
                    }
                    None => Argument::new(name, Default::default()),
                };

                word_index_to_arg_index_map.insert(*word_index, arg_index);
                arg_name_to_arg_index_map.insert(name.to_string(), arg_index);

                argument
            })
            .collect();

        (
            arguments,
            word_index_to_arg_index_map,
            arg_name_to_arg_index_map,
        )
    }

    fn query_argument_by_name(&self, name: &str) -> Option<&Argument> {
        match self.arg_name_to_arg_index_map.get(name) {
            Some(argument_index) => Some(&self.arguments[*argument_index]),
            None => None,
        }
    }

    fn query_argument_by_word_index(&self, word_index: usize) -> Option<&Argument> {
        match self.word_index_to_arg_index_map.get(&word_index) {
            Some(argument_index) => Some(&self.arguments[*argument_index]),
            None => None,
        }
    }

    fn new_argument_with_previous_data(
        new_argument_name: &str,
        prev_argument: &Argument,
    ) -> Argument {
        Argument {
            name: new_argument_name.to_string(),
            description: prev_argument.description.clone(),
            default_value: prev_argument.default_value.clone(),
            arg_type: Default::default(),
        }
    }
}

#[cfg(test)]
#[path = "arguments_tests.rs"]
mod tests;
