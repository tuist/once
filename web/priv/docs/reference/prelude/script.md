# `script`

An adapter target for existing executable automation. Use annotated scripts
when a command already works and you want to declare its inputs, outputs,
and environment without rewriting it as a custom target kind.

## Contract

The graph schema requires `script_path` and `script_runtime`. The script's
`# struo` headers describe the action contract; `struo query script` validates
and inspects those headers. The adapter exposes `run` with the `default`
output group.

For most script workflows, start with `struo exec` rather than writing an
adapter manifest by hand:

```sh
struo query script scripts/build.sh
struo exec -- bash scripts/build.sh
```

See [Scripted Automation](/guide/scripted) for a complete example and
[Caching](/guide/scripted/caching) for environment and cache-policy declarations.
Use `struo query schema script` when inspecting a script target in the graph.
