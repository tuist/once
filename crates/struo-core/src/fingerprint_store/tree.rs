use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use struo_cas::Digest;

use crate::digest_writer::DigestWriter;
use crate::InputFingerprintComponent;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Node<T> {
    pub(super) version: u8,
    pub(super) component: T,
    pub(super) left: Option<Digest>,
    pub(super) right: Option<Digest>,
}

/// Hash category, label, content, order, and multiplicity without filesystem access.
/// This identity is independent of storage blocking. Manifest schema and input
/// digest are not included; empty sequences use a versioned empty-tree digest.
pub fn fingerprint_components_digest(components: &[InputFingerprintComponent]) -> Digest {
    let root = component_tree(components, |node| {
        Ok::<_, Infallible>(serialized_digest(&node))
    })
    .unwrap_or_else(|never| match never {});
    root.unwrap_or_else(|| Digest::of_bytes(b"struo.input_fingerprint.tree.v1\0empty"))
}

fn serialized_digest(value: &impl Serialize) -> Digest {
    let mut writer = DigestWriter::default();
    serde_json::to_writer(&mut writer, value)
        .expect("fingerprint JSON serialization is infallible");
    writer.finish()
}

pub(super) fn component_tree<E>(
    components: &[InputFingerprintComponent],
    mut node_digest: impl FnMut(Node<&InputFingerprintComponent>) -> Result<Digest, E>,
) -> Result<Option<Digest>, E> {
    struct Pending<'a> {
        component: &'a InputFingerprintComponent,
        priority: Digest,
        left: Option<Digest>,
    }
    // Equal priorities retain their original order, including duplicate components.
    // Finalize subtrees as they leave the spine instead of retaining the whole tree.
    let mut spine: Vec<Pending<'_>> = Vec::new();
    for component in components {
        let priority = serialized_digest(component);
        let mut left = None;
        while spine
            .last()
            .is_some_and(|parent| parent.priority.as_bytes() > priority.as_bytes())
        {
            let child = spine.pop().expect("nonempty tree spine");
            left = Some(node_digest(Node {
                version: 1,
                component: child.component,
                left: child.left,
                right: left,
            })?);
        }
        spine.push(Pending {
            component,
            priority,
            left,
        });
    }
    let mut root = None;
    while let Some(child) = spine.pop() {
        root = Some(node_digest(Node {
            version: 1,
            component: child.component,
            left: child.left,
            right: root,
        })?);
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(components: &[InputFingerprintComponent]) -> Option<Digest> {
        let index = components
            .iter()
            .enumerate()
            .min_by_key(|(_, component)| serialized_digest(*component).to_hex())?
            .0;
        Some(serialized_digest(&Node {
            version: 1,
            component: &components[index],
            left: reference(&components[..index]),
            right: reference(&components[index + 1..]),
        }))
    }

    #[test]
    fn streaming_spine_matches_canonical_cartesian_tree() {
        for seed in 0_u64..64 {
            let components: Vec<_> = (0_u64..64)
                .map(|index| {
                    let value = (index * 17 + seed) % 11;
                    InputFingerprintComponent::new(
                        "source",
                        value.to_string(),
                        Digest::of_bytes(&value.to_le_bytes()),
                    )
                })
                .collect();
            assert_eq!(
                Some(fingerprint_components_digest(&components)),
                reference(&components)
            );
        }
    }
}
