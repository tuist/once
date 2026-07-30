//! Embed Once in Rust, Swift, Objective-C, or C.
//!
//! The Rust programming interface gives embedders a small, stable entry point
//! for cache access.
//!
//! ```no_run
//! # async fn example() -> once::Result<()> {
//! let cache = once::Cache::new();
//! let digest = cache.put_blob(b"hello").await?;
//!
//! assert_eq!(cache.get_blob(digest).await?, b"hello");
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub use action_key::ActionKeyBuilder;
pub use cache::{digest_from_hex, Cache, Error, Result};
pub use once_cas::{ActionResult, CacheProvider, Digest, Stats, TuistCacheConfig};

mod action_key;
mod cache;

// The C ABI entry points are the workspace's only sanctioned use of
// `unsafe`: they dereference caller-supplied pointers and hand back
// owned `CString`s, which cannot be expressed in safe Rust. Scoped to
// this module so the rest of the crate stays under the workspace-wide
// `unsafe_code = "deny"`.
#[allow(unsafe_code)]
mod ffi;

#[cfg(test)]
static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
