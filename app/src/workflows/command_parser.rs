//! This module contains utilities for computing helper data structures used to render and
//! implement the Workflows UI in the info box and the terminal input.

use std::collections::HashMap;
use std::ops::Range;

use itertools::Itertools;
use lazy_static::lazy_static;
use regex::Regex;
use string_offset::{ByteOffset, CharCounter, CharOffset};

use super::workflow::Workflow;

lazy_static! {
    /// Regex for escaped arguments in workflow command.
    static ref ESCAPED_ARGUMENTS_PATTERN: Regex = Regex::new(r"\{\{\{([^{}]+)\}\}\}").expect("Escaped argument regex should be valid.");
}

#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct WorkflowArgumentIndex(usize);

impl From<usize> for WorkflowArgumentIndex {
    fn from(num: usize) -> Self {
        Self(num)
    }
}

impl std::ops::Deref for WorkflowArgumentIndex {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Helper struct for inputting Workflow arguments within the editor.
#[derive(Debug)]
struct WorkflowArgument<'a> {
    /// The index of the argument in the list of arguments.
    argument_index: WorkflowArgumentIndex,
    argument_name: &'a str,
    /// The text the workflow should replace the argument identifier with.
    replacement_text: &'a str,
    /// The byte indices of the argument in the workflow.
    byte_range: Range<ByteOffset>,
    /// The character indices of the argument in the workflow.
    char_range: Range<CharOffset>,
}

/// Helper struct containing computed metadata about the workflow and its arguments, used to render
/// the workflows UI in both the input editor and workflows "info box".
#[derive(Debug)]
pub struct WorkflowDisplayData {
    /// The command with replaced arguments. Arguments are replaced with their default values (if
    /// they exist) or with their display "placeholders" (e.g. "{{argument_name}}"), or in the case
    /// of executed workflows from history, the arguments used when executing the workflow.
    pub command_with_replaced_arguments: String,

    /// A vector of `ByteOffset` ranges representing the ranges in the original command string
    /// replaced with argument values.
    pub replaced_ranges: Vec<Range<ByteOffset>>,

    /// Index of workflow argument index (in the workflow.arguments() list) mapped to a vector of
    /// indices of the workflow argument instances in the actual replaced command. For instance,
    /// if workflow.arguments = ["foo", "bar"] and the workflow is "echo {{foo}} {{bar}} {{foo}}",
    /// the entry for "foo" would be (0, [0, 2]).
    pub argument_index_to_highlight_index_map: HashMap<WorkflowArgumentIndex, Vec<usize>>,

    /// Index of workflow argument index (in the workflow.arguments() list) mapped to a vector of
    /// [`CharOffset`] ranges in the replaced command indicating the ranges in the
    /// replaced command where text was replaced for the corresponding argument. For example,
    /// if workflow.arguments = ["foo", "bar"] and the workflow is "echo {{foo}} {{bar}} {{foo}}",
    /// the entry for "foo" would be [5-8, 13-16].
    pub argument_index_to_char_range_map: HashMap<WorkflowArgumentIndex, Vec<Range<CharOffset>>>,
}

/// Computes workflow display data for displaying the workflow in the input editor and info box
/// when the workflow is selected for execution.
pub fn compute_workflow_display_data(workflow: &Workflow) -> WorkflowDisplayData {
    compute_workflow_display_data_internal(workflow, None)
}

/// Computes workflow display data for displaying the workflow in the input editor and info box
/// allowing argument override.
pub fn compute_workflow_display_data_with_overrides(
    workflow: &Workflow,
    override_argument_values: HashMap<String, String>,
) -> WorkflowDisplayData {
    let values = override_argument_values
        .into_iter()
        .map(|(k, v)| (k, ArgumentValue(v)))
        .collect();
    compute_workflow_display_data_internal(workflow, Some(values))
}

fn compute_workflow_display_data_internal(
    workflow: &Workflow,
    override_argument_values: Option<HashMap<String, ArgumentValue>>,
) -> WorkflowDisplayData {
    let (command, workflow_arguments) = parse_and_escape_workflow(workflow);
    let mut command_with_replaced_arguments = command.to_owned();

    let mut delta_bytes = 0_isize;
    let mut delta_chars = 0_isize;
    let mut replaced_ranges = vec![];
    let mut argument_index_to_highlight_index_map = HashMap::new();
    let mut argument_index_to_char_range_map = HashMap::new();

    // Compute the final command (with the argument identifiers replaced with the argument name)
    // and its corresponding text style ranges.
    for (highlight_index, workflow_argument) in workflow_arguments.into_iter().enumerate() {
        let original_char_range = workflow_argument.char_range;
        let original_byte_range = workflow_argument.byte_range;
        let replacement_text = override_argument_values
            .as_ref()
            .and_then(|values| values.get(workflow_argument.argument_name))
            .map(|value| value.0.as_str())
            .unwrap_or(workflow_argument.replacement_text);

        // Compute the range of the argument within the workflow and replace the range in the
        // original workflow command with the argument name.
        let text_byte_range = original_byte_range.start.add_signed(delta_bytes)
            ..original_byte_range.end.add_signed(delta_bytes);
        let text_char_range = original_char_range.start.add_signed(delta_chars)
            ..original_char_range.end.add_signed(delta_chars);

        command_with_replaced_arguments.replace_range(
            text_byte_range.start.as_usize()..text_byte_range.end.as_usize(),
            replacement_text,
        );

        // Compute the delta between the replacement text and the original length of the
        // argument within the command. This is inclusive of the curly braces around the
        // argument name, which is why we add 4 here (since there are two curly braces on each
        // side).
        let original_argument_byte_length = workflow_argument.argument_name.len() + 4;
        delta_bytes += replacement_text.len() as isize - original_argument_byte_length as isize;
        let original_argument_char_length = workflow_argument.argument_name.chars().count() + 4;
        delta_chars +=
            replacement_text.chars().count() as isize - original_argument_char_length as isize;

        argument_index_to_highlight_index_map
            .entry(workflow_argument.argument_index)
            .or_insert_with(Vec::new)
            .push(highlight_index);

        replaced_ranges
            .push(text_byte_range.start..(text_byte_range.start + replacement_text.len()));

        argument_index_to_char_range_map
            .entry(workflow_argument.argument_index)
            .or_insert_with(Vec::new)
            .push(
                text_char_range.start..(text_char_range.start + replacement_text.chars().count()),
            );
    }

    WorkflowDisplayData {
        command_with_replaced_arguments,
        replaced_ranges,
        argument_index_to_highlight_index_map,
        argument_index_to_char_range_map,
    }
}

/// Remove extra brackets from escaped arguments in workflow command and compute a list of workflow arguments and their positions in the escaped command.
fn parse_and_escape_workflow(workflow: &Workflow) -> (String, Vec<WorkflowArgument<'_>>) {
    let (escaped_content, shift_indices) = replace_escaped_brackets_in_command(workflow.command());
    let workflow_arguments = parse_workflow_arguments(workflow, shift_indices);

    (escaped_content, workflow_arguments)
}

/// Replaces the triple brackets used for indicating escaped arguments in the command with double brackets.
/// Returns both the modified command and a vector representing the start indices of the escaped arguments, based on the original command.
fn replace_escaped_brackets_in_command(command: &str) -> (String, Vec<usize>) {
    let mut escaped_indices = Vec::new();
    let mut new_command = command.to_owned();
    let mut num_removed_brackets = 0;

    // Iterate through escaped args and remove one bracket from either end
    for cap in ESCAPED_ARGUMENTS_PATTERN.captures_iter(command) {
        let escaped_arg = cap.get(0).expect("First regex group always exists");

        new_command.replace_range(
            escaped_arg.start() - num_removed_brackets..escaped_arg.end() - num_removed_brackets,
            &escaped_arg.as_str()[1..escaped_arg.len() - 1], // remove one bracket from either end
        );

        // Store the location of the escaped argument
        escaped_indices.push(escaped_arg.start());
        num_removed_brackets += 2;
    }

    // Return the new command and the start index of escaped args that had brackets removed
    (new_command, escaped_indices)
}

/// Given a `workflow` and a vector `escaped_indices`, which is presumed to be a vector representing the start
/// indices of escaped arguments in the `workflow` command as generated by `replace_escaped_brackets_in_command`,
/// return a vector of parsed WorkflowArgument objects.
fn parse_workflow_arguments(
    workflow: &Workflow,
    escaped_indices: Vec<usize>,
) -> Vec<WorkflowArgument<'_>> {
    workflow
        .arguments()
        .iter()
        .map(|argument| (argument, format!("{{{{{}}}}}", argument.name())))
        .enumerate()
        .flat_map(|(argument_index, (argument, argument_placeholder))| {
            let mut char_counter = CharCounter::new(workflow.command());
            workflow
                .command()
                .match_indices(argument_placeholder.as_str())
                .filter(|(argument_start_index, _)| {
                    // Skip any escaped arguments (this case only occurs when an escaped argument has the same inner text as a real argument)
                    *argument_start_index == 0
                        || !escaped_indices.contains(&(*argument_start_index - 1))
                })
                .map(|(argument_start_index, argument_placeholder)| {
                    // Based on how many escape argument brackets we have already removed, shift over the stored argument range.
                    let range_offset = escaped_indices
                        .iter()
                        .filter(|x| **x < argument_start_index)
                        .count()
                        * 2;
                    let byte_start = ByteOffset::from(argument_start_index - range_offset);
                    let byte_end = ByteOffset::from(
                        argument_start_index + argument_placeholder.len() - range_offset,
                    );

                    let char_start = char_counter
                        .char_offset(byte_start)
                        .unwrap_or(CharOffset::from(byte_start.as_usize()));
                    let char_end = char_counter
                        .char_offset(byte_end)
                        .unwrap_or(CharOffset::from(byte_end.as_usize()));

                    WorkflowArgument {
                        argument_index: argument_index.into(),
                        argument_name: argument.name(),
                        replacement_text: argument
                            .default_value()
                            .as_deref()
                            .unwrap_or_else(|| argument.name()),
                        byte_range: byte_start..byte_end,
                        char_range: char_start..char_end,
                    }
                })
                .collect::<Vec<_>>()
        })
        .sorted_by_key(|workflow_argument| workflow_argument.byte_range.start)
        .collect_vec()
}

#[derive(Debug)]
pub struct ArgumentValue(String);

#[cfg(test)]
#[path = "command_parser_tests.rs"]
mod tests;
