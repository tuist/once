#!/usr/bin/env bash
#MISE description="Use the Tuist remote cache for release graph builds when it is reachable"
set -uo pipefail

# Release builds share action results through the Tuist remote cache, but a
# Tuist outage must not block a release. Signing in and one cache lookup
# prove the remote cache answers; otherwise the build uses the local cache.

release_tasks_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The latest released Struo signs in before the bootstrap exists, so the
# bootstrap can already build through the remote cache.
struo_bin="${STRUO_BIN:-$(STRUO_BOOTSTRAP_SOURCE=release "${release_tasks_dir}/bootstrap-struo.sh")}"
probe_digest="0000000000000000000000000000000000000000000000000000000000000000"

provider=local
if "${struo_bin}" auth login --provider tuist --no-browser &&
  STRUO_CACHE_PROVIDER=tuist "${struo_bin}" cache action get "${probe_digest}" >/dev/null; then
  provider=tuist
else
  echo "warning: the Tuist remote cache is unavailable, building with the local cache" >&2
fi

echo "select-struo-cache: STRUO_CACHE_PROVIDER=${provider}" >&2
if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "STRUO_CACHE_PROVIDER=${provider}" >> "${GITHUB_ENV}"
fi
