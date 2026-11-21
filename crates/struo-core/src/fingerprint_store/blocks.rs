use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use struo_cas::Digest;

use crate::InputFingerprintComponent;

use super::MAX_NODE_BYTES;

pub(super) const MAX_BLOCK_COMPONENTS: usize = 256;
const HEADER: &[u8] = b"{\"version\":2,\"components\":[";
const FOOTER: &[u8] = b"]}";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Branch {
    pub(super) version: u8,
    pub(super) block: Digest,
    pub(super) left: Option<Digest>,
    pub(super) right: Option<Digest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Block {
    pub(super) version: u8,
    pub(super) components: Vec<InputFingerprintComponent>,
}

struct Pending {
    block: Digest,
    left: Option<Digest>,
}

pub(super) fn store_tree(
    components: &[InputFingerprintComponent],
    mut write: impl FnMut(Digest, &[u8]) -> Result<()>,
) -> Result<Option<Digest>> {
    let mut spine: Vec<Pending> = Vec::new();
    let mut buffer = HEADER.to_vec();
    let mut bytes = Vec::new();
    let mut count = 0;
    let mut finish = |buffer: &mut Vec<u8>| -> Result<()> {
        buffer.extend_from_slice(FOOTER);
        let block = Digest::of_bytes(buffer);
        write(block, buffer)?;
        let mut left = None;
        while spine
            .last()
            .is_some_and(|parent| parent.block.as_bytes() > block.as_bytes())
        {
            let child = spine.pop().expect("nonempty block tree spine");
            left = Some(write_branch(&child, left, &mut write)?);
        }
        spine.push(Pending { block, left });
        buffer.clear();
        buffer.extend_from_slice(HEADER);
        Ok(())
    };
    for component in components {
        bytes.clear();
        serde_json::to_writer(&mut bytes, component)?;
        ensure!(
            bytes.len() + HEADER.len() + FOOTER.len() <= MAX_NODE_BYTES,
            "fingerprint component size limit exceeded"
        );
        if count != 0 && buffer.len() + 1 + bytes.len() + FOOTER.len() > MAX_NODE_BYTES {
            finish(&mut buffer)?;
            count = 0;
        }
        if count != 0 {
            buffer.push(b',');
        }
        buffer.extend_from_slice(&bytes);
        count += 1;
        // Anchors depend only on component content, not its index or prior boundaries.
        if Digest::of_bytes(&bytes).as_bytes()[0].trailing_zeros() >= 6
            || count == MAX_BLOCK_COMPONENTS
        {
            finish(&mut buffer)?;
            count = 0;
        }
    }
    if count != 0 {
        finish(&mut buffer)?;
    }
    let mut root = None;
    while let Some(child) = spine.pop() {
        root = Some(write_branch(&child, root, &mut write)?);
    }
    Ok(root)
}

fn write_branch(
    child: &Pending,
    right: Option<Digest>,
    write: &mut impl FnMut(Digest, &[u8]) -> Result<()>,
) -> Result<Digest> {
    let bytes = serde_json::to_vec(&Branch {
        version: 2,
        block: child.block,
        left: child.left,
        right,
    })?;
    let digest = Digest::of_bytes(&bytes);
    write(digest, &bytes)?;
    Ok(digest)
}

#[cfg(test)]
mod tests;
