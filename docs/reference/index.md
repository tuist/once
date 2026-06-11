# Reference

Looked-up material that mirrors the code: synopsis, flags, exit
codes, and other surface that callers reach for at the keyboard.

The [CLI Reference](/reference/cli/) is generated from the `clap`
definitions in `crates/once-cli/src/cli.rs`. The docs build runs
`npm run build:reference` before `vitepress build`, so the rendered
pages never drift from the binary that ships.
