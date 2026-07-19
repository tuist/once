#!/bin/sh
set -eu

mise exec -- cargo run --quiet -p once-core --example sandbox_contract_benchmark
