use super::*;

#[test]
fn test_empty_diffs() {
    let hidden_lines = calculate_hidden_lines(&[], 20);

    let expected_hidden = [LineCount::from(0)..LineCount::from(20)]
        .into_iter()
        .collect::<RangeSet<LineCount>>();

    assert_eq!(hidden_lines, expected_hidden);
}

#[test]
fn test_single_diff_middle_of_file() {
    // File with 20 lines, change at lines 10-12 (1-indexed)
    let diffs = vec![DiffDelta {
        replacement_line_range: 10..13, // 1-indexed, replacing lines 10, 11, 12
        insertion: "new line 1\nnew line 2\nnew line 3".to_string(), // 3 lines
    }];

    let hidden_lines = calculate_hidden_lines(&diffs, 20);

    let mut expected_hidden = RangeSet::new();
    expected_hidden.insert(0.into()..5.into()); // lines 1-5 (1-indexed)
    expected_hidden.insert(16.into()..20.into()); // lines 17-20 (1-indexed)

    assert_eq!(hidden_lines, expected_hidden);
}

#[test]
fn test_insertion_at_beginning() {
    // Insert at the very beginning of file
    let diffs = vec![DiffDelta {
        replacement_line_range: 1..1,                    // Insert at beginning
        insertion: "new line 1\nnew line 2".to_string(), // 2 lines
    }];

    let hidden_lines = calculate_hidden_lines(&diffs, 20);

    let mut expected_hidden = RangeSet::new();
    expected_hidden.insert(4.into()..20.into());

    assert_eq!(hidden_lines, expected_hidden);
}

#[test]
fn test_multiple_diffs_overlapping_context() {
    // Two changes close enough that their context overlaps
    let diffs = vec![
        DiffDelta {
            replacement_line_range: 5..6,      // 1-indexed, replace line 5
            insertion: "change 1".to_string(), // 1 line
        },
        DiffDelta {
            replacement_line_range: 8..9,      // 1-indexed, replace line 8
            insertion: "change 2".to_string(), // 1 line
        },
    ];

    let hidden_lines = calculate_hidden_lines(&diffs, 20);
    let mut expected_hidden = RangeSet::new();
    expected_hidden.insert(12.into()..20.into());

    assert_eq!(hidden_lines, expected_hidden);
}

#[test]
fn test_multiple_diffs_separate_context() {
    // Two changes far apart
    let diffs = vec![
        DiffDelta {
            replacement_line_range: 3..4,      // 1-indexed, replace line 3
            insertion: "change 1".to_string(), // 1 line
        },
        DiffDelta {
            replacement_line_range: 15..16,    // 1-indexed, replace line 15
            insertion: "change 2".to_string(), // 1 line
        },
    ];

    let hidden_lines = calculate_hidden_lines(&diffs, 20);

    let mut expected_hidden = RangeSet::new();
    expected_hidden.insert(7.into()..10.into());
    expected_hidden.insert(19.into()..20.into());

    assert_eq!(hidden_lines, expected_hidden);
}
