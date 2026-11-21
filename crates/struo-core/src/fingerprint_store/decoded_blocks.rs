use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use anyhow::{Context, Result};
use struo_cas::Digest;

use super::blocks::Block;
use crate::InputFingerprintComponent;

const MAX_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRIES: usize = 256;

#[derive(Debug, Default)]
pub(super) struct DecodedBlocks {
    entries: HashMap<Digest, (Arc<Block>, usize)>,
    order: VecDeque<Digest>,
    bytes: usize,
}

impl DecodedBlocks {
    /// The caller must read and authenticate the blob before every lookup.
    pub(super) fn parse(&mut self, digest: Digest, bytes: &[u8]) -> Result<Arc<Block>> {
        if let Some((block, _)) = self.entries.get(&digest) {
            return Ok(Arc::clone(block));
        }
        let block: Block = serde_json::from_slice(bytes)
            .with_context(|| format!("decoding fingerprint block {digest}"))?;
        let size = std::mem::size_of::<Block>()
            + 2 * std::mem::size_of::<usize>()
            + block.components.capacity() * std::mem::size_of::<InputFingerprintComponent>()
            + block
                .components
                .iter()
                .map(|component| component.category.capacity() + component.label.capacity())
                .sum::<usize>();
        let block = Arc::new(block);
        if size <= MAX_PAYLOAD_BYTES {
            while self.bytes + size > MAX_PAYLOAD_BYTES || self.entries.len() >= MAX_ENTRIES {
                let oldest = self
                    .order
                    .pop_front()
                    .expect("nonempty decoded block cache");
                let (_, removed) = self.entries.remove(&oldest).expect("indexed decoded block");
                self.bytes -= removed;
            }
            self.bytes += size;
            self.order.push_back(digest);
            self.entries.insert(digest, (Arc::clone(&block), size));
        }
        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_payloads_and_index_have_separate_bounds() {
        let mut cache = DecodedBlocks::default();
        for label_len in [8, 100_000] {
            for index in 0..=MAX_ENTRIES {
                let bytes = serde_json::to_vec(&serde_json::json!({
                    "version": 2,
                    "components": [{
                        "category": "source",
                        "label": format!("{index}{}", "x".repeat(label_len)),
                        "digest": Digest::of_bytes(&index.to_le_bytes()),
                    }],
                }))
                .unwrap();
                cache.parse(Digest::of_bytes(&bytes), &bytes).unwrap();
                assert!(cache.bytes <= MAX_PAYLOAD_BYTES);
                assert!(cache.entries.len() <= MAX_ENTRIES);
                assert_eq!(cache.entries.len(), cache.order.len());
            }
        }
    }
}
