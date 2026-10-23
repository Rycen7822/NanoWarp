use std::collections::HashMap;

use lazy_static::lazy_static;

use super::compute_workflow_display_data;
use crate::workflows::workflow::{Argument, Workflow};

lazy_static! {
    static ref WORKFLOW_WITH_ESCAPES: Workflow = Workflow::Command {
        name: "Workflow with escaped arguments".to_owned(),
        command:
            r#"docker history --no-trunc --format {{arg1}} {{{.ID}}}: {{{.CreatedBy}}} {{{arg2}}} {{arg2}}"#
                .to_owned(),
        arguments: vec![
            Argument {
                name: "arg1".to_owned(),
                default_value: Some("default1".to_owned()),
                description: None,
                arg_type: Default::default()
            },
            Argument {
                name: "arg2".to_owned(),
                default_value: None,
                description: None,
                arg_type: Default::default()
            }
        ],
        description: None,
        tags: vec![],
        source_url: None,
        author: None,
        author_url: None,
        shells: vec![
            warp_workflows::Shell::Zsh,
            warp_workflows::Shell::Bash,
            warp_workflows::Shell::Fish,
        ],
        environment_variables: None,
    };
    static ref WORKFLOW_WITH_DUPLICATES_AND_ESCAPES: Workflow = Workflow::Command {
        name: "Workflow with escaped arguments".to_owned(),
        command:
            r#"{{{hi}}} {{hi}} {{{{{{hi}} {{{{hi}}}} {{{{{hi}}} {{hi}}"#
                .to_owned(),
        arguments: vec![
            Argument {
                name: "hi".to_owned(),
                default_value: None,
                description: None,
                arg_type: Default::default()
            },
        ],
        description: None,
        tags: vec![],
        source_url: None,
        author: None,
        author_url: None,
        shells: vec![
            warp_workflows::Shell::Zsh,
            warp_workflows::Shell::Bash,
            warp_workflows::Shell::Fish,
        ],
        environment_variables: None,
    };

    static ref WORKFLOW_WITH_MULTIBYTE_CHARS: Workflow = Workflow::Command {
        name: "Workflow with multiyte chars".to_owned(),
        command:
            r#"echo "hello 😎{{name}}🤠{{name}}"#
                .to_owned(),
        arguments: vec![
            Argument {
                name: "name".to_owned(),
                default_value: None,
                description: None,
                arg_type: Default::default()
            },
        ],
        description: None,
        tags: vec![],
        source_url: None,
        author: None,
        author_url: None,
        shells: vec![
            warp_workflows::Shell::Zsh,
            warp_workflows::Shell::Bash,
            warp_workflows::Shell::Fish,
        ],
        environment_variables: None,
    };
}

#[test]
fn test_compute_workflow_display_data_with_escaped_arguments() {
    let display_data = compute_workflow_display_data(&WORKFLOW_WITH_ESCAPES);
    let correct_command =
        "docker history --no-trunc --format default1 {{.ID}}: {{.CreatedBy}} {{arg2}} arg2";

    assert_eq!(
        display_data.command_with_replaced_arguments.as_str(),
        correct_command
    );
    assert_eq!(
        display_data.replaced_ranges,
        vec![35.into()..43.into(), 77.into()..81.into()]
    );
    assert_eq!(
        display_data.argument_index_to_char_range_map,
        HashMap::from([
            (0.into(), vec![35.into()..43.into()]),
            (1.into(), vec![77.into()..81.into()])
        ])
    );
}

#[test]
fn test_compute_workflow_display_data_with_duplicates_and_escaped_arguments() {
    let display_data = compute_workflow_display_data(&WORKFLOW_WITH_DUPLICATES_AND_ESCAPES);
    let correct_command = "{{hi}} hi {{{{hi {{{hi}}} {{{{hi}} hi";

    assert_eq!(
        display_data.command_with_replaced_arguments.as_str(),
        correct_command
    );
    assert_eq!(
        display_data.replaced_ranges,
        vec![
            7.into()..9.into(),
            14.into()..16.into(),
            35.into()..37.into()
        ]
    );
    assert_eq!(
        display_data.argument_index_to_char_range_map,
        HashMap::from([(
            0.into(),
            vec![
                7.into()..9.into(),
                14.into()..16.into(),
                35.into()..37.into()
            ]
        ),])
    );
}

#[test]
fn test_compute_workflow_display_data_with_multibyte_chars() {
    let display_data = compute_workflow_display_data(&WORKFLOW_WITH_MULTIBYTE_CHARS);
    assert_eq!(
        display_data.command_with_replaced_arguments.as_str(),
        r#"echo "hello 😎name🤠name"#
    );
    assert_eq!(
        display_data.replaced_ranges,
        vec![16.into()..20.into(), 24.into()..28.into()]
    );
    assert_eq!(
        display_data.argument_index_to_char_range_map,
        HashMap::from([(0.into(), vec![13.into()..17.into(), 18.into()..22.into()])])
    );
}
