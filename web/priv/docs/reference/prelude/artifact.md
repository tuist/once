# `artifact`

Packages explicitly selected files as an immutable artifact.

`srcs` selects package-relative source globs. Output is a deterministic ZIP,
exposed through both `build` and `artifact` capabilities with `default` and
`artifact` output groups. The target emits `struo_artifact_info` with one member
named `product`, independently of the native target name. Selected file and
directory symlinks retain their link targets rather than being dereferenced.
Select the linked content explicitly when the artifact needs to contain it.

| Attribute | Type | Default | Purpose |
| --- | --- | --- | --- |
| `interfaces` | `map<string, any>` | `{}` | Versioned consumption contracts. |
| `selectors` | `map<string, string>` | `{}` | Explicit variant facts. |
| `file_mode` | `int` | `420` | UNIX permissions; `493` is executable. |

Each interface binds `payload = "content"` to a contract-owned `spec` object.

Discover complete starters with `struo query example artifact artifact-static-site`
and `struo query example artifact artifact-download`. See
[Artifacts](/guide/infrastructure/artifacts) for configuration,
publication and consumer responsibilities.
