use serde_json::json;

use super::{settings_schema_json, strip_empty_enum_entries, strip_numeric_metadata};

#[test]
fn strips_numeric_metadata_recursively() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "count": {
                "type": "integer",
                "minimum": 0,
                "maximum": 255,
                "format": "uint8"
            }
        }
    });

    strip_numeric_metadata(&mut schema);

    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer"
                }
            }
        })
    );
}

#[test]
fn strips_empty_enum_entries() {
    let mut schema = json!({
        "oneOf": [
            {
                "enum": [],
                "type": "string"
            },
            {
                "const": "kept"
            }
        ]
    });

    strip_empty_enum_entries(&mut schema);

    assert_eq!(
        schema,
        json!({
            "oneOf": [
                {
                    "const": "kept"
                }
            ]
        })
    );
}

#[test]
fn generates_a_settings_schema() {
    let schema = settings_schema_json(|_| false).unwrap();
    let schema: serde_json::Value = serde_json::from_str(&schema).unwrap();

    assert_eq!(schema["title"], "Warp Settings");
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"].is_object());
}
