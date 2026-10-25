mod binary_detection {
    use std::io::Write as _;

    use async_io::block_on;
    use tempfile::TempDir;

    use super::super::{is_file_content_binary_async, should_read_as_binary};

    fn write_file(dir: &TempDir, name: &str, contents: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).expect("create temp file");
        file.write_all(contents).expect("write temp file");
        file.flush().expect("flush temp file");
        path
    }

    #[test]
    fn text_file_with_known_extension_is_not_binary() {
        let dir = TempDir::new().expect("create tempdir");
        let path = write_file(&dir, "script.sh", b"#!/usr/bin/env bash\necho hi\n");
        assert!(!block_on(should_read_as_binary(&path)));
    }

    #[test]
    fn binary_file_with_known_extension_is_binary() {
        let dir = TempDir::new().expect("create tempdir");
        // Known binary extension — should be classified as binary without
        // needing content inspection.
        let path = write_file(&dir, "image.png", b"not really a png but extension wins\n");
        assert!(block_on(should_read_as_binary(&path)));
    }

    #[test]
    fn extensionless_shell_script_is_not_binary() {
        // Regression test for QUALITY-507: an extensionless shell script (e.g.
        // `script/linux/bundle`) was being classified as binary solely because
        // its basename isn't in the known extensionless-text allow-list.
        let dir = TempDir::new().expect("create tempdir");
        let path = write_file(
            &dir,
            "bundle",
            b"#!/usr/bin/env bash\n#\n# Builds a Warp binary and bundles it up for distribution.\n\nset -e\n",
        );
        assert!(!block_on(should_read_as_binary(&path)));
    }

    #[test]
    fn extensionless_binary_content_is_binary() {
        // An extensionless file whose contents are actually binary should fall
        // through the content-based check and be classified as binary.
        let dir = TempDir::new().expect("create tempdir");
        let path = write_file(
            &dir,
            "payload",
            // NUL byte is a strong binary signal for content_inspector.
            &[0u8, 1, 2, 3, b'A', 0, 0, 0, 0xFF, 0xFE, 0xFD],
        );
        assert!(block_on(should_read_as_binary(&path)));
    }

    #[test]
    fn extensionless_text_allowlisted_is_not_binary() {
        // Files whose basenames are in the known text allow-list (e.g. README)
        // should take the fast path and skip content inspection.
        let dir = TempDir::new().expect("create tempdir");
        let path = write_file(&dir, "README", b"Hello, world!\n");
        assert!(!block_on(should_read_as_binary(&path)));
    }

    #[test]
    fn empty_extensionless_file_is_not_binary() {
        // `content_inspector` treats an empty buffer as text, which is the
        // desired behavior for `read_files`: an empty file should be
        // surfaced to the agent as an empty string, not as zero binary bytes.
        let dir = TempDir::new().expect("create tempdir");
        let path = write_file(&dir, "empty", b"");
        assert!(!block_on(should_read_as_binary(&path)));
    }

    #[test]
    fn missing_extensionless_file_is_classified_as_binary() {
        // When an extensionless file cannot be opened during content
        // inspection, `should_read_as_binary` must route to the binary path
        // so the binary reader can produce a consistent `Missing` result.
        let dir = TempDir::new().expect("create tempdir");
        let missing = dir.path().join("does-not-exist");
        assert!(block_on(should_read_as_binary(&missing)));
    }

    #[test]
    fn missing_file_helper_is_classified_as_binary() {
        // Direct coverage of the low-level helper: opening a non-existent
        // path must return `true` so the caller doesn't accidentally try the
        // text path on an unreadable file.
        let dir = TempDir::new().expect("create tempdir");
        let missing = dir.path().join("does-not-exist");
        assert!(block_on(is_file_content_binary_async(&missing)));
    }
}

mod read_file_failures {
    use std::io::Write as _;

    use async_io::block_on;
    use tempfile::TempDir;

    use super::super::read_local_file_context;
    use crate::util::file::FileLocations;

    fn write_file(dir: &TempDir, name: &str, contents: &[u8]) -> String {
        let path = dir.path().join(name);
        let mut file = std::fs::File::create(&path).expect("create temp file");
        file.write_all(contents).expect("write temp file");
        file.flush().expect("flush temp file");
        path.to_string_lossy().to_string()
    }

    fn location(path: &str) -> FileLocations {
        FileLocations {
            name: path.to_string(),
            lines: vec![],
        }
    }

    #[test]
    fn oversized_binary_file_reports_too_large_not_missing() {
        // Regression for APP-4882: an existing binary file over the per-file
        // limit was reported as "do not exist" instead of "too large".
        let dir = TempDir::new().expect("create tempdir");
        let path = write_file(&dir, "big.png", &vec![0u8; 1_500_000]);

        let result = block_on(read_local_file_context(
            &[location(&path)],
            None,
            None,
            None,
            None,
        ))
        .expect("read_local_file_context");

        assert!(result.file_contexts.is_empty());
        assert_eq!(result.failed_files.len(), 1);
        let message = &result.failed_files[0].message;
        assert!(message.contains("too large to read"), "got: {message}");
        assert!(message.contains("1.5 MB"), "got: {message}");
        assert!(message.contains("1.0 MB limit"), "got: {message}");
        assert!(!message.contains("does not exist"), "got: {message}");
    }

    #[test]
    fn missing_file_reports_does_not_exist() {
        let dir = TempDir::new().expect("create tempdir");
        let missing = dir.path().join("nope.txt").to_string_lossy().to_string();

        let result = block_on(read_local_file_context(
            &[location(&missing)],
            None,
            None,
            None,
            None,
        ))
        .expect("read_local_file_context");

        assert!(result.file_contexts.is_empty());
        assert_eq!(result.failed_files.len(), 1);
        assert_eq!(result.failed_files[0].message, "File does not exist");
    }

    #[test]
    fn unprocessable_image_reports_processing_failure() {
        // An image-typed file with invalid bytes (under the size limit) fails
        // image processing and must be distinct from "too large"/"do not exist".
        let dir = TempDir::new().expect("create tempdir");
        let path = write_file(&dir, "broken.png", b"definitely not a valid PNG image");

        let result = block_on(read_local_file_context(
            &[location(&path)],
            None,
            None,
            None,
            None,
        ))
        .expect("read_local_file_context");

        assert!(result.file_contexts.is_empty());
        assert_eq!(result.failed_files.len(), 1);
        let message = &result.failed_files[0].message;
        assert!(
            message.contains("could not be processed as an image"),
            "got: {message}"
        );
        assert!(!message.contains("too large"), "got: {message}");
        assert!(!message.contains("does not exist"), "got: {message}");
    }
}
