//! Go rule planning.
//!
//! Go module resolution stays native: Fabrik models the dependency edge
//! for graph visibility and cache-key metadata, then invokes `go build`
//! from the target package so `go.mod`, `go.sum`, and `replace` entries
//! keep their normal meaning.

mod compile;
mod plan;

pub use compile::CompileError;
pub use plan::{build_plan, supports_kind, PlanBuildError};
