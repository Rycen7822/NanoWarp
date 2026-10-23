use std::collections::HashMap;

/// User defaults for predictable AI input behavior needed in evals.
///
/// This allows tests to more reliably enter and exit AI input mode.
///
/// * UDI is enabled
/// * Natural language detection is disabled
pub fn user_defaults_map_for_ai_input() -> HashMap<String, String> {
    HashMap::from_iter([
        (
            "AIAutoDetectionEnabled".to_owned(),
            serde_json::to_string(&false).unwrap(),
        ),
        (
            "InputBoxTypeSetting".to_owned(),
            serde_json::to_string("Universal").unwrap(),
        ),
    ])
}
