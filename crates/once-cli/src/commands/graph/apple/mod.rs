//! Apple-specific glue between the once-cli graph commands and the
//! starlark analysis layer in once-frontend.
//!
//! Source globbing lives here because it depends on workspace layout,
//! and the impl callbacks in `apple.star` consume the resolved paths
//! through `ctx.srcs`.

pub(super) mod srcs;
