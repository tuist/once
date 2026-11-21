use std::fs;

use rusqlite::Connection;
use tempfile::TempDir;

use super::*;
use crate::InputFingerprintComponent;

fn component(index: usize) -> InputFingerprintComponent {
    InputFingerprintComponent::new(
        "source",
        format!("src/{index:06}"),
        Digest::of_bytes(&index.to_le_bytes()),
    )
}

fn manifest(components: Vec<InputFingerprintComponent>) -> InputFingerprintManifest {
    InputFingerprintManifest::new(Digest::of_bytes(b"composed-input"), components)
}

fn node_count(workspace: &Path) -> usize {
    Connection::open(database::path(workspace))
        .unwrap()
        .query_row("SELECT count(*) FROM nodes", [], |row| row.get(0))
        .unwrap()
}

fn root_digest(stored: &Value) -> Digest {
    serde_json::from_value(stored["root"].clone()).unwrap()
}

fn read_blob(workspace: &Path, digest: Digest) -> Vec<u8> {
    Connection::open(database::path(workspace))
        .unwrap()
        .query_row(
            "SELECT data FROM nodes WHERE digest=?1",
            [digest.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap()
}

fn put_blob(workspace: &Path, digest: Digest, bytes: &[u8]) {
    Connection::open(database::path(workspace))
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO nodes (digest, data) VALUES (?1, ?2)",
            rusqlite::params![digest.as_bytes().as_slice(), bytes],
        )
        .unwrap();
}

fn delete_blob(workspace: &Path, digest: Digest) {
    Connection::open(database::path(workspace))
        .unwrap()
        .execute(
            "DELETE FROM nodes WHERE digest=?1",
            [digest.as_bytes().as_slice()],
        )
        .unwrap();
}

#[test]
fn roundtrip_preserves_order_duplicates_schema_and_digest() {
    let tmp = TempDir::new().unwrap();
    for components in [
        vec![],
        vec![component(1)],
        vec![
            component(3),
            component(1),
            component(3),
            component(0),
            component(1),
        ],
    ] {
        let mut original = manifest(components);
        original.schema = "custom-preserved-schema".into();
        let stored = encode_fingerprint(tmp.path(), &original).unwrap();
        assert!(stored.get("components").is_none());
        assert_eq!(
            decode_fingerprint(tmp.path(), stored.clone()).unwrap(),
            original
        );
        assert_eq!(encode_fingerprint(tmp.path(), &original).unwrap(), stored);
    }
}

#[test]
fn legacy_inline_manifest_needs_no_node_store() {
    let tmp = TempDir::new().unwrap();
    let original = manifest(vec![component(9), component(2), component(9)]);
    assert_eq!(
        decode_fingerprint(tmp.path(), serde_json::to_value(&original).unwrap()).unwrap(),
        original
    );
    assert!(!tmp.path().join(".struo").exists());
}

#[test]
fn missing_and_corrupt_nodes_fail_explicitly() {
    let tmp = TempDir::new().unwrap();
    let original = manifest((0..20).map(component).collect());
    let stored = encode_fingerprint(tmp.path(), &original).unwrap();
    let root = root_digest(&stored);
    let bytes = read_blob(tmp.path(), root);
    delete_blob(tmp.path(), root);
    let error = decode_fingerprint(tmp.path(), stored.clone()).unwrap_err();
    assert!(error.to_string().contains("missing fingerprint node"));
    put_blob(tmp.path(), root, b"corrupt");
    assert!(decode_fingerprint(tmp.path(), stored.clone())
        .unwrap_err()
        .to_string()
        .contains("digest mismatch"));
    put_blob(tmp.path(), root, &bytes);
    assert_eq!(
        decode_fingerprint(tmp.path(), stored.clone()).unwrap(),
        original
    );

    let another: Vec<u8> = Connection::open(database::path(tmp.path()))
        .unwrap()
        .query_row(
            "SELECT digest FROM nodes WHERE digest!=?1 LIMIT 1",
            [root.as_bytes().as_slice()],
            |row| row.get(0),
        )
        .unwrap();
    let another = Digest::from_bytes(another.try_into().unwrap());
    delete_blob(tmp.path(), another);
    assert!(decode_fingerprint(tmp.path(), stored).is_err());
}

#[test]
fn growing_asymmetric_sequences_share_stable_subtrees() {
    let tmp = TempDir::new().unwrap();
    let mut components: Vec<_> = (0..1024).map(|index| component(index * 8)).collect();
    let initial = manifest(components.clone());
    let initial_stored = encode_fingerprint(tmp.path(), &initial).unwrap();
    let mut previous = node_count(tmp.path());
    assert!(previous < components.len() / 8);
    for index in 0..32 {
        // Insertions throughout a large shared prefix plus asymmetric suffixes.
        components.push(component(index * 191 + 1));
        components.push(component(20_000 + index));
        components.sort_by(|left, right| left.label.cmp(&right.label));
        let grown = manifest(components.clone());
        let stored = encode_fingerprint(tmp.path(), &grown).unwrap();
        assert_eq!(decode_fingerprint(tmp.path(), stored).unwrap(), grown);
        let current = node_count(tmp.path());
        assert!(
            current - previous < 100,
            "insertion rewrote {} nodes",
            current - previous
        );
        previous = current;
    }
    assert!(previous < 1024 + 32 * 100);
    assert_eq!(
        decode_fingerprint(tmp.path(), initial_stored).unwrap(),
        initial
    );
}

#[test]
fn deep_duplicate_sequence_uses_no_recursive_traversal() {
    let tmp = TempDir::new().unwrap();
    let original = manifest(vec![component(1); 4096]);
    let stored = encode_fingerprint(tmp.path(), &original).unwrap();
    assert_eq!(decode_fingerprint(tmp.path(), stored).unwrap(), original);
}

#[test]
fn hostile_counts_versions_and_oversized_nodes_fail() {
    let tmp = TempDir::new().unwrap();
    let original = manifest(vec![component(1), component(2)]);
    let stored = encode_fingerprint(tmp.path(), &original).unwrap();
    for count in [0, 1, 3, MAX_COMPONENTS + 1] {
        let mut invalid = stored.clone();
        invalid["count"] = count.into();
        assert!(decode_fingerprint(tmp.path(), invalid).is_err());
    }
    let mut invalid = stored.clone();
    invalid["storage"] = "unknown".into();
    assert!(decode_fingerprint(tmp.path(), invalid).is_err());
    let root = root_digest(&stored);
    let mut node: Value = serde_json::from_slice(&read_blob(tmp.path(), root)).unwrap();
    node["version"] = 3.into();
    let bytes = serde_json::to_vec(&node).unwrap();
    let digest = Digest::of_bytes(&bytes);
    put_blob(tmp.path(), digest, &bytes);
    let mut invalid = stored.clone();
    invalid["root"] = serde_json::to_value(digest).unwrap();
    assert!(decode_fingerprint(tmp.path(), invalid)
        .unwrap_err()
        .to_string()
        .contains("node version"));
    put_blob(tmp.path(), root, &vec![0; MAX_NODE_BYTES + 1]);
    assert!(decode_fingerprint(tmp.path(), stored)
        .unwrap_err()
        .to_string()
        .contains("size limit"));
}

#[test]
fn concurrent_writers_publish_complete_nodes() {
    let original = manifest((0..128).map(component).collect());
    let baseline = TempDir::new().unwrap();
    encode_fingerprint(baseline.path(), &original).unwrap();
    let expected_nodes = node_count(baseline.path());
    for _ in 0..8 {
        let tmp = TempDir::new().unwrap();
        let start = std::sync::Barrier::new(4);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        encode_fingerprint(tmp.path(), &original).unwrap()
                    })
                })
                .collect();
            for handle in handles {
                assert_eq!(
                    decode_fingerprint(tmp.path(), handle.join().unwrap()).unwrap(),
                    original
                );
            }
        });
        assert_eq!(node_count(tmp.path()), expected_nodes);
    }
}

#[test]
fn pure_digest_is_storage_independent_and_binds_every_component_field() {
    let tmp = TempDir::new().unwrap();
    let original = manifest(vec![component(1), component(2), component(1)]);
    let digest = fingerprint_components_digest(&original.components);
    let stored = encode_fingerprint(tmp.path(), &original).unwrap();
    assert_eq!(decode_fingerprint(tmp.path(), stored).unwrap(), original);
    let mut variants = Vec::new();
    for field in 0..3 {
        let mut components = original.components.clone();
        match field {
            0 => components[0].category.push('x'),
            1 => components[0].label.push('x'),
            _ => components[0].digest = Digest::of_bytes(b"changed"),
        }
        variants.push(components);
    }
    variants.push(vec![component(2), component(1), component(1)]);
    variants.push(vec![component(1), component(2)]);
    variants.push(vec![component(1), component(2), component(1), component(1)]);
    variants.push(vec![]);
    for components in variants {
        assert_ne!(fingerprint_components_digest(&components), digest);
    }
    assert_eq!(
        fingerprint_components_digest(&[]),
        fingerprint_components_digest(&[])
    );
    assert_ne!(
        fingerprint_components_digest(&[]),
        fingerprint_components_digest(&[component(1)])
    );
    let deep = vec![component(1); 4096];
    let stored = encode_fingerprint(tmp.path(), &manifest(deep.clone())).unwrap();
    assert_eq!(
        fingerprint_components_digest(&decode_fingerprint(tmp.path(), stored).unwrap().components),
        fingerprint_components_digest(&deep)
    );
}

#[test]
fn session_reuses_verified_nodes_but_decode_always_checks_disk() {
    let tmp = TempDir::new().unwrap();
    let store = FingerprintStore::new(tmp.path());
    let original = manifest((0..128).map(component).collect());
    let stored = store.encode(&original).unwrap();
    let writes = store.disk_checks();
    assert!(writes < 128);
    assert_eq!(store.encode(&original).unwrap(), stored);
    assert_eq!(store.disk_checks(), writes);
    assert_eq!(store.decode(stored.clone()).unwrap(), original);
    let root = root_digest(&stored);
    put_blob(tmp.path(), root, b"corrupted after verification");
    assert!(store
        .decode(stored.clone())
        .unwrap_err()
        .to_string()
        .contains("digest mismatch"));
    delete_blob(tmp.path(), root);
    assert!(store.decode(stored).is_err());
}

#[test]
fn unused_inline_and_empty_stores_do_not_create_database() {
    let tmp = TempDir::new().unwrap();
    let store = FingerprintStore::new(tmp.path());
    let inline = manifest(vec![component(1)]);
    assert_eq!(
        store
            .decode(serde_json::to_value(&inline).unwrap())
            .unwrap(),
        inline
    );
    let empty = manifest(vec![]);
    let stored = store.encode(&empty).unwrap();
    assert_eq!(store.decode(stored).unwrap(), empty);
    assert!(!tmp.path().join(".struo").exists());
}

#[test]
fn failed_encode_rolls_back_nodes_and_can_be_retried() {
    let tmp = TempDir::new().unwrap();
    let store = FingerprintStore::new(tmp.path());
    let mut oversized = component(2);
    oversized.label = "\0".repeat(MAX_NODE_BYTES / 2);
    assert!(store
        .encode(&manifest(vec![component(1), oversized]))
        .is_err());
    assert_eq!(node_count(tmp.path()), 0);
    let original = manifest(vec![component(1), component(2)]);
    let stored = store.encode(&original).unwrap();
    assert_eq!(store.decode(stored).unwrap(), original);
}

fn storage_size(workspace: &Path) -> (u64, u64, usize) {
    let logical = Connection::open(database::path(workspace))
        .unwrap()
        .query_row(
            "SELECT sum(length(digest)+length(data)) FROM nodes",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut physical = 0;
    let mut files = 0;
    for entry in fs::read_dir(workspace.join(".struo/fingerprints")).unwrap() {
        let metadata = entry.unwrap().metadata().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            physical += metadata.blocks() * 512;
        }
        #[cfg(not(unix))]
        {
            physical += metadata.len();
        }
        files += 1;
    }
    (logical, physical, files)
}

#[test]
fn packed_storage_avoids_per_component_files_and_block_amplification() {
    let tmp = TempDir::new().unwrap();
    let store = FingerprintStore::new(tmp.path());
    let original = manifest((0..2048).map(component).collect());
    let stored = store.encode(&original).unwrap();
    drop(store);
    let (logical, physical, files) = storage_size(tmp.path());
    assert!(files <= 3);
    assert!(
        physical < logical * 4 + 65536,
        "logical={logical}, physical={physical}"
    );
    assert_eq!(decode_fingerprint(tmp.path(), stored).unwrap(), original);
}

#[test]
#[ignore = "800-target disk-work comparison"]
fn growing_chain_encode_disk_work() {
    use std::time::Instant;

    let tmp = TempDir::new().unwrap();
    let run = |shared: bool| {
        let session = FingerprintStore::new(tmp.path());
        let mut components: Vec<_> = (0..1024).map(|index| component(index * 8)).collect();
        let mut checks = 0;
        let mut references = Vec::new();
        let start = Instant::now();
        for index in 0..800 {
            components.push(component(index * 7 + 1));
            components.push(component(20_000 + index));
            components.sort_by(|left, right| left.label.cmp(&right.label));
            let original = manifest(components.clone());
            if shared {
                references.push(session.encode(&original).unwrap());
            } else {
                let isolated = FingerprintStore::new(tmp.path());
                isolated.encode(&original).unwrap();
                checks += isolated.disk_checks();
            }
        }
        (
            if shared {
                session.disk_checks()
            } else {
                checks
            },
            start.elapsed(),
            references,
        )
    };
    let cold = run(true);
    let isolated = run(false);
    let shared = run(true);
    let reader = FingerprintStore::new(tmp.path());
    let start = Instant::now();
    for (index, stored) in cold.2.into_iter().enumerate() {
        let decoded = reader.decode(stored).unwrap();
        assert_eq!(decoded.components.len(), 1024 + (index + 1) * 2);
    }
    let decode_time = start.elapsed();
    drop(reader);
    let sizes = storage_size(tmp.path());
    eprintln!("800 growing fingerprints: cold={:?}, isolated={:?}, shared={:?}, decode={decode_time:?}, insert_attempts={}/{}, logical/physical/files={sizes:?}",
        cold.1, isolated.1, shared.1, isolated.0, shared.0);
    assert!(isolated.0 < 1_460_000 / 16);
    assert!(shared.0 < isolated.0 / 2);
    assert_eq!(shared.0, node_count(tmp.path()));
}

#[test]
fn mixed_order_and_duplicates_roundtrip_across_many_blocks() {
    let tmp = TempDir::new().unwrap();
    let original = manifest(
        (0..4097)
            .map(|index| component((index * 37) % 257))
            .collect(),
    );
    let stored = encode_fingerprint(tmp.path(), &original).unwrap();
    assert_eq!(decode_fingerprint(tmp.path(), stored).unwrap(), original);
}

#[test]
fn block_payload_is_reauthenticated_after_prior_decode() {
    let tmp = TempDir::new().unwrap();
    let store = FingerprintStore::new(tmp.path());
    let original = manifest((0..1024).map(component).collect());
    let stored = store.encode(&original).unwrap();
    assert_eq!(store.decode(stored.clone()).unwrap(), original);
    let branch: Value =
        serde_json::from_slice(&read_blob(tmp.path(), root_digest(&stored))).unwrap();
    let block: Digest = serde_json::from_value(branch["block"].clone()).unwrap();
    let bytes = read_blob(tmp.path(), block);
    put_blob(tmp.path(), block, b"corrupted block");
    assert!(store
        .decode(stored.clone())
        .unwrap_err()
        .to_string()
        .contains("digest mismatch"));
    put_blob(tmp.path(), block, &bytes);
    assert_eq!(store.decode(stored.clone()).unwrap(), original);
    delete_blob(tmp.path(), block);
    assert!(store
        .decode(stored)
        .unwrap_err()
        .to_string()
        .contains("missing fingerprint node"));
}

#[test]
fn authenticated_malformed_blocks_fail_semantic_validation() {
    let tmp = TempDir::new().unwrap();
    let stored = encode_fingerprint(tmp.path(), &manifest(vec![component(1)])).unwrap();
    let branch: Value =
        serde_json::from_slice(&read_blob(tmp.path(), root_digest(&stored))).unwrap();
    for (version, count, expected) in [
        (2, 0, "component count"),
        (2, 257, "component count"),
        (3, 1, "block version"),
    ] {
        let block =
            serde_json::json!({"version": version, "components": vec![component(1); count]});
        let bytes = serde_json::to_vec(&block).unwrap();
        let digest = Digest::of_bytes(&bytes);
        put_blob(tmp.path(), digest, &bytes);
        let mut malformed = branch.clone();
        malformed["block"] = serde_json::to_value(digest).unwrap();
        let bytes = serde_json::to_vec(&malformed).unwrap();
        let digest = Digest::of_bytes(&bytes);
        put_blob(tmp.path(), digest, &bytes);
        let mut invalid = stored.clone();
        invalid["root"] = serde_json::to_value(digest).unwrap();
        invalid["count"] = count.max(1).into();
        assert!(decode_fingerprint(tmp.path(), invalid)
            .unwrap_err()
            .to_string()
            .contains(expected));
    }
}
