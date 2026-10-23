use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use repo_metadata::{RepositoryUpdate, TargetFile};

use super::{filter_repository_update_by_prefix, warp_home_skills_dir, warp_managed_skill_dirs};

#[test]
fn warp_managed_skill_dirs_contains_only_warp_home_path() {
    let dirs = warp_managed_skill_dirs();
    match warp_home_skills_dir() {
        Some(warp_home_skills_dir) => assert_eq!(dirs, vec![warp_home_skills_dir]),
        None => assert!(dirs.is_empty()),
    }
}

#[test]
fn filter_repository_update_by_prefix_keeps_only_matching_paths() {
    let skills_dir = PathBuf::from("/tmp/.warp-local/skills");
    let other_dir = PathBuf::from("/tmp/.warp-local/worktrees/repo");
    let skill_file = skills_dir.join("deploy").join("SKILL.md");
    let other_file = other_dir.join("README.md");

    let update = RepositoryUpdate {
        added: HashSet::from([
            TargetFile::new(skill_file.clone(), false),
            TargetFile::new(other_file.clone(), false),
        ]),
        modified: HashSet::new(),
        deleted: HashSet::new(),
        moved: HashMap::new(),
        commit_updated: false,
        index_lock_detected: false,
        remote_ref_updated: false,
    };

    let filtered =
        filter_repository_update_by_prefix(&update, &skills_dir).expect("expected update");

    assert!(filtered.contains_added_or_modified(&TargetFile::new(skill_file, false)));
    assert!(!filtered.contains_added_or_modified(&TargetFile::new(other_file, false)));
}

#[test]
fn filter_repository_update_by_prefix_converts_cross_boundary_moves() {
    let skills_dir = PathBuf::from("/tmp/.warp-local/skills");
    let skill_file = skills_dir.join("deploy").join("SKILL.md");
    let ignored_file = PathBuf::from("/tmp/.warp-local/worktrees/repo/SKILL.md");

    let update = RepositoryUpdate {
        added: HashSet::new(),
        modified: HashSet::new(),
        deleted: HashSet::new(),
        moved: HashMap::from([(
            TargetFile::new(skill_file.clone(), false),
            TargetFile::new(ignored_file, false),
        )]),
        commit_updated: false,
        index_lock_detected: false,
        remote_ref_updated: false,
    };

    let filtered =
        filter_repository_update_by_prefix(&update, &skills_dir).expect("expected update");

    assert!(filtered.contains_added_or_modified(&TargetFile::new(skill_file, false)));
    assert!(filtered.moved.is_empty());
    assert!(filtered.deleted.is_empty());
}
