# Once Rust Review

Review Once changes with attention to behavior that can invalidate cache
correctness or the command-line contract.

Focus on:

- digest stability and cache key partitioning
- script annotation parsing and path resolution
- action output restoration and artifact paths
- provider behavior across local and remote cache storage
- runtime session query behavior
- CLI compatibility covered by shellspec
- missing tests for user-visible behavior

Skip style-only findings that rustfmt, clippy, or existing shellspec
coverage already handle.
