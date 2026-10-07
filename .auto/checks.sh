#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
./autoresearch.checks.sh
mise exec -- cargo test --quiet -p once-cli bus_events::tests --bin once
mise exec -- cargo clippy --quiet -p once-cli --all-targets -- -D warnings
