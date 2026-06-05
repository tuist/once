//! Embed Once in Rust, Swift, Objective-C, or C.
//!
//! The Rust API gives embedders a small, stable entry point for
//! cache-aware command execution while still exposing the lower-level
//! building blocks for advanced integrations.
//!
//! ```no_run
//! # async fn example() -> once::Result<()> {
//! let once = once::Once::new(".", ".once");
//! let outcome = once
//!     .run_command(
//!         once::Command::new("sh")
//!             .arg("-c")
//!             .arg("printf hello")
//!     )
//!     .await?;
//!
//! assert_eq!(outcome.exit_code, 0);
//! # Ok(())
//! # }
//! ```

mod ffi;
mod sdk;

pub use sdk::{Command, CommandOutcome, Error, Once, Result};

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
