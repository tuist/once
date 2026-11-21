use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest as _, Sha256};
use struo_cas::Digest;
use struo_frontend::analysis::DeclaredAction;

use super::super::lowering::compose_input_fingerprint_with_available;
use crate::commands::graph::analysis::AvailableInput;

fn store_node(workspace: &Path, direct: &[&str], children: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"struo.transitive_set.v1\0");
    hash.update((direct.len() as u64).to_le_bytes());
    for path in direct {
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
    }
    hash.update((children.len() as u64).to_le_bytes());
    for child in children {
        hash.update(hex::decode(child).unwrap());
    }
    let hex = hex::encode(hash.finalize());
    let directory = workspace.join(".struo/transitive-sets").join(&hex[..2]);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{hex}.json")),
        serde_json::to_vec(&serde_json::json!({"direct": direct, "children": children})).unwrap(),
    )
    .unwrap();
    hex
}

#[test]
fn composed_inputs_track_shared_sources_and_contextual_generated_contents() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("shared.h"), b"initial source").unwrap();
    let shared = store_node(workspace.path(), &["shared.h"], &[]);
    let left = store_node(
        workspace.path(),
        &["generated-"],
        std::slice::from_ref(&shared),
    );
    let right = store_node(workspace.path(), &["generated/right"], &[shared]);
    let root = store_node(workspace.path(), &[], &[left, right]);
    let mut declared: DeclaredAction = serde_json::from_value(serde_json::json!({
        "argv": ["tool", "--compile"],
        "inputs": [{"$struo_transitive_set": root}],
        "outputs": []
    }))
    .unwrap();
    declared.resolve_inputs(workspace.path()).unwrap();
    let available = |contents: &[u8], same_target| AvailableInput {
        blob_digest: Digest::of_bytes(contents),
        same_target,
        materialized: false,
    };
    let mut generated = BTreeMap::from([
        ("generated-".to_string(), available(b"left", false)),
        ("generated/right".to_string(), available(b"right", false)),
    ]);
    let fingerprint = |generated: &BTreeMap<_, _>| {
        compose_input_fingerprint_with_available(
            workspace.path(),
            &declared,
            Digest::of_bytes(b"module"),
            &[],
            generated,
            None,
        )
        .unwrap()
    };
    let initial = fingerprint(&generated);
    let paths = initial
        .components
        .iter()
        .filter(|component| {
            ["source", "dependency-output", "generated-input"]
                .contains(&component.category.as_str())
        })
        .map(|component| component.label.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["generated-", "generated/right", "shared.h"]);

    generated.get_mut("generated/right").unwrap().blob_digest = Digest::of_bytes(b"changed right");
    assert_ne!(initial.input_digest, fingerprint(&generated).input_digest);
    generated.get_mut("generated/right").unwrap().blob_digest = Digest::of_bytes(b"right");
    generated.get_mut("generated-").unwrap().same_target = true;
    assert_ne!(initial.input_digest, fingerprint(&generated).input_digest);
    generated.get_mut("generated-").unwrap().same_target = false;
    std::fs::write(workspace.path().join("shared.h"), b"edited source").unwrap();
    assert_ne!(initial.input_digest, fingerprint(&generated).input_digest);
}
