#!/usr/bin/env bash
set -euo pipefail

mise exec -- cargo test --quiet -p once-frontend modules --lib
mise exec -- cargo test --quiet -p once-cli commands::graph::analysis::tests --bin once
mise exec -- cargo fmt --all -- --check
