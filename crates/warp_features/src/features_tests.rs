use super::*;

#[test]
fn factory_deferred_repositories_are_dogfood_only_by_default() {
    assert!(DOGFOOD_FLAGS.contains(&FeatureFlag::FactoryDeferredRepositories));
    for flags in [DEBUG_FLAGS, LOCAL_FLAGS, PREVIEW_FLAGS, RELEASE_FLAGS] {
        assert!(!flags.contains(&FeatureFlag::FactoryDeferredRepositories));
    }
}

#[test]
fn local_child_harnesses_are_local_only_by_default() {
    assert!(LOCAL_FLAGS.contains(&FeatureFlag::LocalClaudeCodexChildHarnesses));
    assert!(!DEBUG_FLAGS.contains(&FeatureFlag::LocalClaudeCodexChildHarnesses));
    assert!(!DOGFOOD_FLAGS.contains(&FeatureFlag::LocalClaudeCodexChildHarnesses));
}
