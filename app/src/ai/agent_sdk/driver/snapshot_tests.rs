use super::*;
use command::blocking::Command as BlockingCommand;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;
use tempfile::{Builder as TempDirBuilder, TempDir};
use tokio::runtime::Runtime;

fn init_git_repo(dir: &Path, dirty: bool) {
    let run = |args: &[&str]| {
        let output = BlockingCommand::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("failed to spawn git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.com"]);
    run(&["config", "user.name", "Test"]);
    fs::write(dir.join("README.md"), "initial\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
    if dirty {
        fs::write(dir.join("README.md"), "modified\n").unwrap();
    }
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let output = BlockingCommand::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn snaptest_tempdir() -> TempDir {
    TempDirBuilder::new().prefix("snaptest").tempdir().unwrap()
}

fn assert_server_accepts_logical_name(name: &str) {
    assert!(!name.is_empty(), "name is empty");
    assert!(name.len() <= 255, "name exceeds 255 bytes: {name}");
    assert!(name != "." && name != "..", "name is a directory alias");
    assert!(!name.starts_with('-'), "name parses as a flag: {name}");
    assert!(
        !name.starts_with("checkpoint_") && name != "latest-checkpoint.json",
        "name collides with the reserved checkpoint namespace: {name}"
    );
    assert!(
        name.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
        "name leaves the server charset: {name}"
    );
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn build_repo_patch_preserves_non_utf8_untracked_paths() {
    let tempdir = snaptest_tempdir();
    init_git_repo(tempdir.path(), false);
    let filename = std::ffi::OsString::from_vec(b"non-utf8-\xFF.txt".to_vec());
    let file_path = tempdir.path().join(filename);
    fs::write(&file_path, b"content from non-utf8 path\n").unwrap();

    let patch = Runtime::new()
        .unwrap()
        .block_on(build_repo_patch(tempdir.path()))
        .unwrap();
    let patch = String::from_utf8_lossy(&patch);

    assert!(
        patch.contains("content from non-utf8 path"),
        "patch should include non-UTF-8 untracked file contents: {patch}"
    );
}

#[test]
fn sanitize_name_component_never_yields_the_reserved_double_underscore() {
    for raw in [
        "a__b.txt",
        "a  b.txt",
        "checkpoint_1700000000000-0__evil.txt",
        "weird name?!.txt",
        "___",
        "ünïcödé.txt",
    ] {
        let sanitized = sanitize_name_component(raw, FALLBACK_SNAPSHOT_FILENAME);
        assert!(
            !sanitized.contains("__"),
            "sanitized {raw:?} still contains `__`: {sanitized}"
        );
        assert!(
            sanitized
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')),
            "sanitized {raw:?} left characters outside the server charset: {sanitized}"
        );
        assert!(!sanitized.is_empty(), "sanitized {raw:?} became empty");
    }
}

#[test]
fn sanitize_name_component_satisfies_the_server_name_contract() {
    let long_name = format!("{}.txt", "a".repeat(400));
    for raw in [
        "-rf.txt",
        "--force",
        "-",
        ".",
        "..",
        "checkpoint_1700000000000-0__evil.txt",
        "latest-checkpoint.json",
        "a__b.txt",
        "weird name?!.txt",
        "___",
        "ünïcödé.txt",
        &long_name,
    ] {
        let sanitized = sanitize_name_component(raw, FALLBACK_SNAPSHOT_FILENAME);
        assert_server_accepts_logical_name(&sanitized);
        assert!(
            !sanitized.contains("__"),
            "sanitized {raw:?} contains the reserved separator: {sanitized}"
        );
    }
}

#[test]
fn unique_filename_keeps_deduplicated_names_within_the_contract() {
    // `a_.txt` used to de-duplicate to `a__2.txt`, reintroducing the reserved separator that
    // sanitization had just squashed out.
    let mut used = HashSet::new();
    let names: Vec<String> = (0..3)
        .map(|_| unique_filename(&sanitize_name_component("a_.txt", "repo"), &mut used))
        .collect();
    for name in &names {
        assert_server_accepts_logical_name(name);
        assert!(
            !name.contains("__"),
            "de-duplicated name is ambiguous: {name}"
        );
    }
    assert_eq!(names.len(), used.len(), "names must stay unique: {names:?}");
}
