#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
repository="$(CDPATH= cd -- "$root/../.." && pwd)"

cd "$root/struo"
env \
  TUIST_TOKEN=benchmark \
  XDG_CACHE_HOME="$root/.state/struo-client/cache" \
  XDG_CONFIG_HOME="$root/.state/struo-client/config" \
  "$repository/target/release/struo" \
  build distribution \
  --format json \
  --quiet
