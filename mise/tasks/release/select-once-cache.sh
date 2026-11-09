#!/usr/bin/env bash
#MISE description="Use the Tuist remote cache for release graph builds when it is reachable"
set -uo pipefail

# Release builds share action results through the Tuist remote cache, but a
# Tuist outage must not block a release. Signing in and one cache lookup
# prove the remote cache answers; otherwise the build uses the local cache.

release_tasks_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The latest released Once signs in before the bootstrap exists, so the
# bootstrap can already build through the remote cache.
once_bin="${ONCE_BIN:-$(ONCE_BOOTSTRAP_SOURCE=release "${release_tasks_dir}/bootstrap-once.sh")}"
probe_digest="0000000000000000000000000000000000000000000000000000000000000000"

provider=local
if "${once_bin}" auth login --provider tuist --no-browser &&
  ONCE_CACHE_PROVIDER=tuist "${once_bin}" cache action get "${probe_digest}" >/dev/null; then
  provider=tuist
else
  echo "warning: the Tuist remote cache is unavailable, building with the local cache" >&2
fi

echo "select-once-cache: ONCE_CACHE_PROVIDER=${provider}" >&2
if [[ -n "${GITHUB_ENV:-}" ]]; then
  echo "ONCE_CACHE_PROVIDER=${provider}" >> "${GITHUB_ENV}"
fi
