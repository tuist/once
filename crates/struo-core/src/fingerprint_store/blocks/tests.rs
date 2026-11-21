use std::collections::{HashMap, HashSet};

use super::*;

fn component(index: usize) -> InputFingerprintComponent {
    InputFingerprintComponent::new(
        "source",
        index.to_string(),
        Digest::of_bytes(&index.to_le_bytes()),
    )
}

fn encoded(components: &[InputFingerprintComponent]) -> HashMap<Digest, Vec<u8>> {
    let mut blobs = HashMap::new();
    store_tree(components, |digest, bytes| {
        assert!(bytes.len() <= MAX_NODE_BYTES);
        blobs.insert(digest, bytes.to_vec());
        Ok(())
    })
    .unwrap();
    blobs
}

fn block_ids(blobs: &HashMap<Digest, Vec<u8>>) -> HashSet<Digest> {
    blobs
        .iter()
        .filter_map(|(&digest, bytes)| serde_json::from_slice::<Block>(bytes).ok().map(|_| digest))
        .collect()
}

#[test]
fn component_and_byte_caps_force_bounded_blocks() {
    let repeated = (0..100)
        .map(component)
        .find(|component| {
            Digest::of_bytes(&serde_json::to_vec(component).unwrap()).as_bytes()[0].trailing_zeros()
                < 6
        })
        .unwrap();
    let blobs = encoded(&vec![repeated; MAX_BLOCK_COMPONENTS * 3 + 17]);
    let lengths: Vec<_> = blobs
        .values()
        .filter_map(|bytes| {
            serde_json::from_slice::<Block>(bytes)
                .ok()
                .map(|block| block.components.len())
        })
        .collect();
    assert!(lengths.contains(&MAX_BLOCK_COMPONENTS));
    assert!(lengths.contains(&17));
    assert!(lengths.iter().all(|&length| length <= MAX_BLOCK_COMPONENTS));

    let large: Vec<_> = (0..100)
        .map(|index| {
            let mut component = component(index);
            component.label.push_str(&"x".repeat(32_768));
            component
        })
        .collect();
    for bytes in encoded(&large).values() {
        if let Ok(block) = serde_json::from_slice::<Block>(bytes) {
            assert!(block.components.len() < 32);
        }
    }
}

#[test]
fn insertion_and_removal_preserve_distant_content_boundaries() {
    let original: Vec<_> = (0..4096).map(component).collect();
    let initial = block_ids(&encoded(&original));
    assert!(initial.len() > 32);
    for index in [0, 31, 100, 2000, 4095] {
        let mut changed = original.clone();
        changed.insert(index, component(9000 + index));
        let inserted = block_ids(&encoded(&changed));
        assert!(initial.intersection(&inserted).count() >= initial.len() - 6);
        changed.remove(index);
        changed.remove(index);
        let removed = block_ids(&encoded(&changed));
        assert!(initial.intersection(&removed).count() >= initial.len() - 6);
    }
}
