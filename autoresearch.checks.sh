#!/bin/sh
set -eu

mise exec -- cargo test -p once-core
mise exec -- cargo fmt --all -- --check
