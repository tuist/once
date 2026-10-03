//! Portable host-directory digests and their metadata cache.
//!
//! Analysis and execution share the same hashing implementation so
//! materialized trees can be verified against their declared identity.

mod digest;
mod digest_cache;

pub use digest::host_tree_sha256_hex;
pub use digest_cache::{tree_stat_fingerprint, TreeDigestCache};
