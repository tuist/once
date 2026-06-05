//! Embed Once in Rust, Swift, Objective-C, or C.
//!
//! The Rust API gives embedders a small, stable entry point for cache
//! access while still exposing the lower-level building blocks for
//! advanced integrations.
//!
//! ```no_run
//! # async fn example() -> once::Result<()> {
//! let cache = once::OnceCache::new();
//! let digest = cache.put_blob(b"hello").await?;
//!
//! assert_eq!(cache.get_blob(digest).await?, b"hello");
//! # Ok(())
//! # }
//! ```

mod ffi;
mod sdk;

pub use sdk::{digest_from_hex, Error, OnceCache, Result};

/// Content-addressed storage and cache-provider primitives.
pub mod cas {
    pub use once_cas::*;
}

/// Lower-level action execution primitives.
pub mod core {
    pub use once_core::*;
}

/// Manifest and script-annotation parsing primitives.
pub mod frontend {
    pub use once_frontend::*;
}
