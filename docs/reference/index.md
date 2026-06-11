# Reference

Looked-up material that mirrors the code: synopsis, flags, exit
codes, and other surface that callers reach for at the keyboard.

The [CLI Reference](/reference/cli/) is generated from the `clap`
definitions in `crates/once-cli/src/cli.rs`. Run `npm run
build:reference` from `docs/` after touching that file to refresh
the committed markdown; the docs build itself just renders what's
on disk so deploys don't need a Rust toolchain.
