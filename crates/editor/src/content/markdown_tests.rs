use std::sync::Arc;

use markdown_parser::parse_markdown;
use serde_yaml::Value;
use warpui_core::{App, ReadModel};

use super::MarkdownStyle;
use crate::content::buffer::Buffer;
use crate::content::buffer::tests::TestEmbeddedItem;
use crate::content::text::{IndentBehavior, TABLE_BLOCK_MARKDOWN_LANG};

#[test]
fn test_export_normalizes_code_languages() {
    let formatted = parse_markdown(
        r#"
```JavaScript
console.log("Hello, World");
```
```Rust
println!("Hello, World");
```
```ocaml
print_endline "Hello, World!"
```
"#,
    )
    .unwrap();
    let exported = Buffer::export_to_markdown(
        formatted,
        None,
        MarkdownStyle::Export {
            app_context: None,
            should_not_escape_markdown_punctuation: false,
        },
    );

    // Exporting should use external code languages.
    assert_eq!(
        exported,
        r#"
```js
console.log("Hello, World");
```
```rust
println!("Hello, World");
```
```ocaml
print_endline "Hello, World!"
```
"#
    );
}

#[test]
fn test_mermaid_markdown_round_trip() {
    App::test((), |mut app| async move {
        let markdown = "```mermaid\ngraph TD\nA --> B\n```\n";
        let (buffer, _selection) = Buffer::mock_from_markdown(
            markdown,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );

        let internal_markdown = app.read_model(&buffer, |buffer, _| buffer.markdown());
        assert_eq!(internal_markdown, markdown);

        let exported_markdown = app.read_model(&buffer, |buffer, _| buffer.markdown_unescaped());
        assert_eq!(exported_markdown, markdown);
    });
}

#[test]
fn test_export_expands_embeds() {
    // This tests styled block for the edge case of querying just the
    // leading block item (0..1).
    App::test((), |mut app| async move {
        let (buffer, _selection) = Buffer::mock_from_markdown(
            r#"
```warp-embedded-object
id: embed-123
```
```warp-embedded-object
id: embed-456
ignored: value
```"#,
            Some(|mut mapping| match mapping.remove(&"id".into()) {
                Some(Value::String(id)) => Some(Arc::new(TestEmbeddedItem { id })),
                _ => None,
            }),
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );

        let exported = app.read_model(&buffer, |buffer, _| {
            buffer.to_markdown(MarkdownStyle::Export {
                app_context: None,
                should_not_escape_markdown_punctuation: false,
            })
        });

        // Exporting should expand the embedded objects.
        assert_eq!(
            exported,
            r#"
```warp-embedded-object
---
id: embed-123
export: true

```
```warp-embedded-object
---
id: embed-456
export: true

```
"#
        );
    });
}

#[test]
fn test_table_markdown_round_trip() {
    App::test((), |mut app| async move {
        let markdown = format!(
            "```{}\nheader 1\theader 2\nvalue 1\tvalue 2\n```\n",
            TABLE_BLOCK_MARKDOWN_LANG
        );
        let (buffer, _selection) = Buffer::mock_from_markdown(
            &markdown,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        let internal_markdown = app.read_model(&buffer, |buffer, _| buffer.markdown());
        assert_eq!(internal_markdown, markdown);

        let exported_markdown = app.read_model(&buffer, |buffer, _| buffer.markdown_unescaped());
        assert_eq!(
            exported_markdown,
            "| header 1 | header 2 |\n| --- | --- |\n| value 1 | value 2 |\n"
        );
    });
}

#[test]
fn test_table_markdown_export_escapes_pipe_characters() {
    App::test((), |mut app| async move {
        let markdown = format!(
            "```{}\nhead|er 1\theader 2\nvalue | 1\tvalue 2\n```\n",
            TABLE_BLOCK_MARKDOWN_LANG
        );
        let (buffer, _selection) = Buffer::mock_from_markdown(
            &markdown,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );

        let exported_markdown = app.read_model(&buffer, |buffer, _| buffer.markdown_unescaped());
        assert_eq!(
            exported_markdown,
            "| head\\|er 1 | header 2 |\n| --- | --- |\n| value \\| 1 | value 2 |\n"
        );
    });
}

#[test]
fn test_url_link_display_text_round_trip_is_stable() {
    App::test((), |mut app| async move {
        let original =
            "[https://example.com/index.html#section](https://example.com/index.html#section)";
        // After the first save, `.` and `#` in the display text are escaped.
        // The URL in `(...)` is written verbatim — no escaping.
        let expected_escaped = "[https://example\\.com/index\\.html\\#section](https://example.com/index.html#section)";

        let (buffer, _) = Buffer::mock_from_markdown(
            original,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        let after_first = app.read_model(&buffer, |buffer, _| buffer.markdown());
        assert_eq!(
            after_first, expected_escaped,
            "first save should escape special chars in display text"
        );

        let (buffer2, _) = Buffer::mock_from_markdown(
            &after_first,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        let after_second = app.read_model(&buffer2, |buffer, _| buffer.markdown());
        assert_eq!(
            after_second, expected_escaped,
            "second round-trip should be stable"
        );

        let (buffer3, _) = Buffer::mock_from_markdown(
            &after_second,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        let after_third = app.read_model(&buffer3, |buffer, _| buffer.markdown());
        assert_eq!(
            after_third, expected_escaped,
            "third round-trip should be stable"
        );

        // Plain text should be the clean, unescaped URL — no backslashes.
        let plain_text = app.read_model(&buffer3, |buffer, _| buffer.text().as_str().to_string());
        assert_eq!(plain_text, "https://example.com/index.html#section");
    });
}

#[test]
fn test_markdown_escapes_punctuation() {
    App::test((), |mut app| async move {
        // markdown() escapes special chars.
        let markdown = "Here's a markdown comment.\n";
        let (buffer, _) = Buffer::mock_from_markdown(
            markdown,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        let escaped = app.read_model(&buffer, |buffer, _| buffer.markdown());
        assert!(
            escaped.contains("\\."),
            "expected escaped periods, got: {escaped}"
        );
    });
}

#[test]
fn test_markdown_unescaped_does_not_escape_punctuation() {
    App::test((), |mut app| async move {
        // markdown_unescaped() should not add backslashes before periods.
        let markdown = "Here's a markdown comment.\n";
        let (buffer, _) = Buffer::mock_from_markdown(
            markdown,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        let unescaped = app.read_model(&buffer, |buffer, _| buffer.markdown_unescaped());
        assert!(
            !unescaped.contains("\\."),
            "expected no escaped periods, got: {unescaped}"
        );
        assert!(
            unescaped.contains("comment."),
            "expected unescaped period, got: {unescaped}"
        );
    });
}

#[test]
fn test_markdown_unescaped_preserves_urls() {
    App::test((), |mut app| async move {
        // markdown_unescaped() should not escape characters inside URLs.
        let markdown = "Check out https://www.example.com/path\n";
        let (buffer, _) = Buffer::mock_from_markdown(
            markdown,
            None,
            Box::new(|_, _| IndentBehavior::Ignore),
            &mut app,
        );
        let unescaped = app.read_model(&buffer, |buffer, _| buffer.markdown_unescaped());
        assert!(
            !unescaped.contains("\\/"),
            "expected no escaped slashes, got: {unescaped}"
        );
        assert!(
            unescaped.contains("https://www.example.com/path"),
            "expected URL preserved, got: {unescaped}"
        );
    });
}
