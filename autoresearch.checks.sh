#!/usr/bin/env bash
set -euo pipefail

mise exec -- cargo test --quiet -p struo-frontend modules --lib
mise exec -- cargo test --quiet -p struo-cli commands::graph::analysis::tests --bin struo
mise exec -- cargo fmt --all -- --check
