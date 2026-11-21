use std::path::Path;
use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use struo_cas::Digest;

use crate::InputFingerprintManifest;

mod blocks;
mod database;
mod decoded_blocks;
mod session;
mod tree;
use blocks::{Block, Branch, MAX_BLOCK_COMPONENTS};
pub use session::FingerprintStore;
pub use tree::fingerprint_components_digest;

const STORAGE: &str = "struo.input_fingerprint.blocks.v2";
const MAX_COMPONENTS: usize = 1_000_000;
const MAX_NODE_BYTES: usize = 1024 * 1024;
const MAX_TREE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFingerprint {
    storage: String,
    schema: String,
    input_digest: Digest,
    root: Option<Digest>,
    count: usize,
}

/// Store an ordered fingerprint as immutable, content-addressed components.
/// Nodes live outside target outcome storage so historical evidence retains them.
pub fn encode_fingerprint(workspace: &Path, manifest: &InputFingerprintManifest) -> Result<Value> {
    FingerprintStore::new(workspace).encode(manifest)
}

fn encode_stored(
    manifest: &InputFingerprintManifest,
    mut write_node: impl FnMut(Digest, &[u8]) -> Result<()>,
) -> Result<Value> {
    let count = manifest.components.len();
    ensure!(
        count <= MAX_COMPONENTS,
        "fingerprint component limit exceeded"
    );
    for component in &manifest.components {
        ensure!(
            component
                .category
                .len()
                .saturating_add(component.label.len())
                <= MAX_NODE_BYTES,
            "fingerprint component size limit exceeded"
        );
    }
    let mut remaining = MAX_TREE_BYTES;
    let root = blocks::store_tree(&manifest.components, |digest, bytes| -> Result<()> {
        ensure!(
            bytes.len() <= MAX_NODE_BYTES,
            "fingerprint node size limit exceeded"
        );
        remaining = remaining
            .checked_sub(bytes.len())
            .context("fingerprint byte limit exceeded")?;
        write_node(digest, bytes)
    })?;
    serde_json::to_value(StoredFingerprint {
        storage: STORAGE.to_string(),
        schema: manifest.schema.clone(),
        input_digest: manifest.input_digest,
        root,
        count,
    })
    .context("serializing compact fingerprint")
}

/// Expand a compact fingerprint, or read a legacy inline manifest unchanged.
/// Missing, malformed, or unauthenticated nodes are errors, not partial evidence.
pub fn decode_fingerprint(workspace: &Path, stored: Value) -> Result<InputFingerprintManifest> {
    FingerprintStore::new(workspace).decode(stored)
}

fn decode_stored(
    stored: Value,
    mut read_node: impl FnMut(Digest) -> Result<Vec<u8>>,
    mut parse_block: impl FnMut(Digest, &[u8]) -> Result<Arc<Block>>,
) -> Result<InputFingerprintManifest> {
    enum Visit {
        Read(Digest),
        Block(Digest),
    }
    if stored.get("storage").is_none() {
        return serde_json::from_value(stored).context("decoding inline fingerprint");
    }
    let stored: StoredFingerprint =
        serde_json::from_value(stored).context("decoding compact fingerprint")?;
    ensure!(
        stored.storage == STORAGE,
        "unsupported fingerprint storage format"
    );
    ensure!(
        stored.count <= MAX_COMPONENTS,
        "fingerprint component limit exceeded"
    );
    ensure!(
        stored.root.is_some() == (stored.count != 0),
        "invalid fingerprint root/count"
    );
    let mut pending = stored
        .root
        .map(|root| vec![Visit::Read(root)])
        .unwrap_or_default();
    let mut components = Vec::new();
    let mut reads = 0;
    let mut remaining = MAX_TREE_BYTES;
    while let Some(visit) = pending.pop() {
        match visit {
            Visit::Read(digest) => {
                ensure!(
                    reads < stored.count,
                    "fingerprint traversal exceeds declared count"
                );
                reads += 1;
                let bytes = read_node(digest)?;
                remaining = remaining
                    .checked_sub(bytes.len())
                    .context("fingerprint byte limit exceeded")?;
                let node: Branch = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decoding fingerprint node {digest}"))?;
                ensure!(node.version == 2, "unsupported fingerprint node version");
                if let Some(right) = node.right {
                    pending.push(Visit::Read(right));
                }
                pending.push(Visit::Block(node.block));
                if let Some(left) = node.left {
                    pending.push(Visit::Read(left));
                }
            }
            Visit::Block(digest) => {
                let bytes = read_node(digest)?;
                remaining = remaining
                    .checked_sub(bytes.len())
                    .context("fingerprint byte limit exceeded")?;
                let block = parse_block(digest, &bytes)?;
                ensure!(block.version == 2, "unsupported fingerprint block version");
                ensure!(
                    !block.components.is_empty() && block.components.len() <= MAX_BLOCK_COMPONENTS,
                    "invalid fingerprint block component count"
                );
                ensure!(
                    components.len() + block.components.len() <= stored.count,
                    "fingerprint traversal exceeds declared count"
                );
                components.extend(block.components.iter().cloned());
            }
        }
    }
    ensure!(
        components.len() == stored.count,
        "fingerprint component count mismatch"
    );
    Ok(InputFingerprintManifest {
        schema: stored.schema,
        input_digest: stored.input_digest,
        components,
    })
}

#[cfg(test)]
mod tests;
