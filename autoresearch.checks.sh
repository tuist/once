#!/usr/bin/env bash
set -euo pipefail

mise exec -- cargo test --quiet -p once-frontend analysis --lib
mise exec -- cargo test --quiet -p once-cli commands::graph::analysis --bin once
mise exec -- cargo test --quiet -p once-cli commands::graph::build_receipt --bin once
mise exec -- cargo test --quiet -p once-cli commands::change_tracker --bin once
mise exec -- cargo fmt --all -- --check
python3 -m py_compile \
  benchmarks/codex-build-comparison/benchmark.py \
  benchmarks/codex-build-comparison/generate-manifest.py
