//! Bundled Starlark prelude.
//!
//! This Starlark source is evaluated implicitly in every user
//! `fabrik.star` file's namespace before user code runs. It defines
//! the built-in target types (`rust_binary`, `rust_library`,
//! `rust_test`) in terms of the lower-level `target(...)` primitive
//! exposed by the Rust globals, so third-party plugins will use the
//! same contract.

pub(crate) const PRELUDE_NAME: &str = "@@fabrik//rust.star";
pub(crate) const PRELUDE_SOURCE: &str = include_str!("../prelude/rust.star");
