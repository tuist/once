#!/bin/sh

set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
repository="$(CDPATH= cd -- "$root/../.." && pwd)"

cd "$root/once"
env \
  TUIST_TOKEN=benchmark \
  XDG_CACHE_HOME="$root/.state/once-client/cache" \
  XDG_CONFIG_HOME="$root/.state/once-client/config" \
  "$repository/target/release/once" \
  build distribution \
  --format json \
  --quiet
