<p align="center">
  <a href="https://github.com/tuist/once/actions/workflows/once.yml"><img src="https://github.com/tuist/once/actions/workflows/once.yml/badge.svg" alt="Continuous integration" /></a>
  <a href="https://github.com/tuist/once/releases/latest"><img src="https://img.shields.io/github/v/release/tuist/once?display_name=tag&sort=semver" alt="Latest release" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/tuist/once" alt="License" /></a>
</p>

# Once

Once turns repository automation into typed, cacheable actions that humans and
coding agents can discover, run, and reuse.

## Install

Install the current release with [mise](https://mise.jdx.dev/):

```sh
mise use -g "github:tuist/once@$(mise latest github:tuist/once)"
mise exec -- once --version
```

Use `mise use github:tuist/once@...` inside a repository when the project
should pin Once in its own `mise.toml`.

## Connect A Coding Harness

Once includes a [Model Context Protocol (MCP)](https://modelcontextprotocol.io/)
server. Add it to a coding harness that supports servers over standard input
and output, and start it through mise so global installs and project pins both
work:

```
{
  "mcpServers": {
    "once": {
      "command": "mise",
      "args": [
        "-C",
        "/absolute/path/to/your/project",
        "exec",
        "--",
        "once",
        "mcp",
        "--workspace",
        "/absolute/path/to/your/project",
        "--allow-run"
      ]
    }
  }
}
```

Remove `--allow-run` if the harness should not edit manifests, build, test,
run, or start runtime sessions.

For a brand-new Rust project, also pin Rust in the project:

```sh
mise use rust@1.96.0
```

Then ask the harness:

```text
Use Once to initialize this directory as a Rust binary package. Discover the
available target kinds, fetch the Rust binary starter, create the files, and
build the target.
```

The harness can discover `rust_binary`, fetch the `rust-binary-with-crate`
starter metadata, materialize it with `once_materialize_example`, validate the
complete graph with `once_validate_workspace`, and verify the result with
`once_build_target`. The materialization call keeps vendored dependencies and
other large starter files out of the model context.

The same live discovery loop supports a request such as “build an Android app
with Once.” See the [coding harness guide](https://once.tuist.dev/guide/harness)
for typed graphs, annotated scripts, result checks, and project memory.

## Run A Script

Add a small contract to an existing script so Once knows the inputs, outputs,
environment, and working directory that shape the action:

```sh
#!/usr/bin/env bash
# once input "../assets/**/*"
# once output "../dist/"
# once cwd ".."

npm run build-assets
```

Run it as a cached action:

```sh
once exec -- bash scripts/build-assets.sh
once exec --remote --compute microsandbox -- bash scripts/build-assets.sh
```

Scripts can also run directly with a Once shebang:

```sh
#!/usr/bin/env -S once exec -- bash
```

## Documentation

Read the documentation at [once.tuist.dev](https://once.tuist.dev).

## License

[MIT License](LICENSE).
