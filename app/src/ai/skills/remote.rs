use remote_server::proto::{BundledSkillMetadata, RemoteSkillProto, remote_skill_proto};

use super::bundled::{BundledSkill, BundledSkillActivation};

/// Serializes a daemon-side bundled catalog for the aggregate remote Agent Mode snapshot.
///
/// `RequiresFile` activations are evaluated here — the daemon owns the
/// files — so only active skills reach the client. The result is sorted by
/// skill path so pushes are deterministic across daemon restarts.
pub(crate) fn bundled_skill_snapshot_protos(catalog: &BundledSkill) -> Vec<RemoteSkillProto> {
    let mut protos: Vec<RemoteSkillProto> = catalog
        .iter_definitions()
        .filter_map(|(id, skill, activation)| {
            match activation {
                BundledSkillActivation::Always => {}
                BundledSkillActivation::RequiresFile(path) => {
                    if !path.exists() {
                        return None;
                    }
                }
            }
            Some(RemoteSkillProto {
                path: skill.path.display_path(),
                content: skill.content.clone(),
                source: Some(remote_skill_proto::Source::Bundled(BundledSkillMetadata {
                    id: id.to_owned(),
                })),
            })
        })
        .collect();
    protos.sort_by(|a, b| a.path.cmp(&b.path));
    protos
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
