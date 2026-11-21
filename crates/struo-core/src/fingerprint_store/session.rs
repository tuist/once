use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::Value;
use struo_cas::Digest;

use crate::InputFingerprintManifest;

const VERIFIED_NODE_CAPACITY: usize = 65_536;

#[derive(Debug, Default)]
struct VerifiedNodes {
    digests: HashSet<Digest>,
    order: VecDeque<Digest>,
}

impl VerifiedNodes {
    fn insert(&mut self, digest: Digest) {
        if self.digests.insert(digest) {
            self.order.push_back(digest);
            if self.order.len() > VERIFIED_NODE_CAPACITY {
                if let Some(oldest) = self.order.pop_front() {
                    self.digests.remove(&oldest);
                }
            }
        }
    }
}

/// Session-scoped fingerprint storage with bounded node and parsed-block reuse.
/// Encode assumes immutable nodes. Decode reads and authenticates every blob
/// before reusing its parsed block, including after a successful earlier read.
#[derive(Debug)]
pub struct FingerprintStore {
    workspace: PathBuf,
    state: Mutex<State>,
    #[cfg(test)]
    disk_checks: std::sync::atomic::AtomicUsize,
}

#[derive(Debug, Default)]
struct State {
    connection: Option<Connection>,
    verified: VerifiedNodes,
    decoded: super::decoded_blocks::DecodedBlocks,
}

impl FingerprintStore {
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        Self {
            workspace: workspace.as_ref().to_path_buf(),
            state: Mutex::default(),
            #[cfg(test)]
            disk_checks: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub fn encode(&self, manifest: &InputFingerprintManifest) -> Result<Value> {
        if manifest.components.is_empty() {
            return super::encode_stored(manifest, |_, _| Ok(()));
        }
        let mut state = self.state.lock().expect("fingerprint store poisoned");
        let State {
            connection,
            verified,
            ..
        } = &mut *state;
        if connection.is_none() {
            *connection = Some(super::database::open(&self.workspace, true)?);
        }
        let connection = connection
            .as_mut()
            .expect("initialized fingerprint database");
        let result = (|| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let stored = {
                let mut insert = transaction
                    .prepare_cached("INSERT OR IGNORE INTO nodes (digest, data) VALUES (?1, ?2)")?;
                super::encode_stored(manifest, |digest, bytes| {
                    if !verified.digests.contains(&digest) {
                        insert.execute(rusqlite::params![digest.as_bytes().as_slice(), bytes])?;
                        verified.insert(digest);
                        #[cfg(test)]
                        self.disk_checks
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(())
                })?
            };
            transaction.commit()?;
            Ok(stored)
        })();
        if result.is_err() {
            *verified = VerifiedNodes::default();
        }
        result
    }

    pub fn decode(&self, stored: Value) -> Result<InputFingerprintManifest> {
        let mut state = self.state.lock().expect("fingerprint store poisoned");
        let State {
            connection,
            decoded,
            ..
        } = &mut *state;
        super::decode_stored(
            stored,
            |digest| {
                if connection.is_none() {
                    *connection = Some(super::database::open(&self.workspace, false)?);
                }
                super::database::read_node(
                    connection
                        .as_ref()
                        .expect("initialized fingerprint database"),
                    digest,
                )
            },
            |digest, bytes| decoded.parse(digest, bytes),
        )
    }

    #[cfg(test)]
    pub(super) fn disk_checks(&self) -> usize {
        self.disk_checks.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_node_cache_is_bounded() {
        let mut cache = VerifiedNodes::default();
        for index in 0..=VERIFIED_NODE_CAPACITY {
            let digest = Digest::of_bytes(&index.to_le_bytes());
            cache.insert(digest);
        }
        assert_eq!(cache.digests.len(), VERIFIED_NODE_CAPACITY);
        assert_eq!(cache.order.len(), VERIFIED_NODE_CAPACITY);
        assert!(!cache
            .digests
            .contains(&Digest::of_bytes(&0_usize.to_le_bytes())));
    }
}
