//! Embed Once in Rust, Swift, Objective-C, or C.
//!
//! The Rust API gives embedders a small, stable entry point for cache
//! access.
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

mod ffi;
mod sdk;

pub use once_cas::{ActionResult, CacheProvider, Digest, Stats};
pub use sdk::{digest_from_hex, Cache, Error, OnceCache, Result};
