#!/usr/bin/env bash
#MISE description="Materialize Cargo-resolved Rust dependencies for Once graph builds"
#USAGE flag "--target <target>" help="Optional Rust target triple for metadata resolution"
set -euo pipefail

target=""
while (($# > 0)); do
  case "$1" in
    --target)
      target="${2}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -n "${target}" && ! "${target}" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "--target must be a Rust target triple" >&2
  exit 1
fi

for tool in jq mise; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} is required" >&2
    exit 1
  fi
done

fingerprint="$(
  {
    printf '%s\n' "${target}"
    shasum -a 256 Cargo.lock Cargo.toml crates/*/Cargo.toml "${BASH_SOURCE[0]}"
  } | shasum -a 256 | awk '{print $1}'
)"
prepared_file="third_party/rust/.prepared"
if [[ -d third_party/rust/vendor && -f "${prepared_file}" && "$(cat "${prepared_file}")" == "${fingerprint}" ]]; then
  echo "prepare-rust-graph-deps: reusing prepared dependencies" >&2
  exit 0
fi
rm -f "${prepared_file}"

export CARGO_NET_RETRY="${CARGO_NET_RETRY:-10}"

retry_command() {
  local description="$1"
  shift

  local attempt=1
  local max_attempts=3
  while true; do
    if "$@"; then
      return 0
    fi
    if ((attempt >= max_attempts)); then
      return 1
    fi
    echo "${description} failed; retrying (${attempt}/${max_attempts})" >&2
    sleep $((attempt * 5))
    attempt=$((attempt + 1))
  done
}

cargo_vendor() {
  mise exec -- cargo vendor --locked --versioned-dirs third_party/rust/vendor >/dev/null
}

rm -rf third_party/rust/vendor
mkdir -p third_party/rust
retry_command "cargo vendor" cargo_vendor

metadata_args=(metadata --locked --format-version 1)
if [[ -n "${target}" ]]; then
  metadata_args+=(--filter-platform "${target}")
fi

cargo_metadata() {
  mise exec -- cargo "${metadata_args[@]}"
}

metadata="$(retry_command "cargo metadata" cargo_metadata)"
schlussel_manifest="$(
  jq -r '
    .packages[]
    | select(.name == "schlussel")
    | select(.source | startswith("git+"))
    | .manifest_path
  ' <<<"${metadata}" | head -n 1
)"

rm -rf third_party/rust/src/formulas
if [[ -n "${schlussel_manifest}" && "${schlussel_manifest}" != "null" ]]; then
  schlussel_root="$(cd "$(dirname "${schlussel_manifest}")/../.." && pwd)"
  if [[ -d "${schlussel_root}/src/formulas" ]]; then
    mkdir -p third_party/rust/src
    cp -R "${schlussel_root}/src/formulas" third_party/rust/src/formulas
  fi
fi

# microsandbox-filesystem's build script downloads the arch-specific
# `agentd` binary from GitHub at compile time. Under Once's hermetic
# execution the child process cannot reach the network, so pre-stage the
# binary where the crate's documented CI escape hatch looks for it:
# `<workspace_root>/build/agentd`, where `workspace_root` is the vendored
# crate's `CARGO_MANIFEST_DIR/../..`. The crate then copies the file into
# its `OUT_DIR` instead of downloading. The `CI` env var is forwarded
# through the Rust build-script action so the escape hatch fires.
microsandbox_version="$(
  jq -r '
    .packages[]
    | select(.name == "microsandbox-filesystem")
    | .version
  ' <<<"${metadata}" | head -n 1
)"
if [[ -n "${microsandbox_version}" && "${microsandbox_version}" != "null" ]]; then
  case "${target}" in
    aarch64-apple-darwin|arm64-apple-darwin) agentd_arch="aarch64" ;;
    x86_64-*) agentd_arch="x86_64" ;;
    aarch64-*) agentd_arch="aarch64" ;;
    "")
      host_arch="$(uname -m)"
      case "${host_arch}" in
        arm64|aarch64) agentd_arch="aarch64" ;;
        x86_64|amd64) agentd_arch="x86_64" ;;
        *) agentd_arch="" ;;
      esac
      ;;
    *) agentd_arch="" ;;
  esac
  if [[ -n "${agentd_arch}" ]]; then
    agentd_url="https://github.com/superradcompany/microsandbox/releases/download/v${microsandbox_version}/agentd-${agentd_arch}"
    mkdir -p third_party/rust/vendor/build
    echo "prepare-rust-graph-deps: fetching ${agentd_url}" >&2
    fetch_agentd() {
      curl --fail --location --silent --show-error \
        --retry 5 --retry-max-time 300 --connect-timeout 30 --max-time 120 \
        --output third_party/rust/vendor/build/agentd.tmp \
        "${agentd_url}"
    }
    retry_command "download agentd" fetch_agentd
    mv third_party/rust/vendor/build/agentd.tmp third_party/rust/vendor/build/agentd
    chmod +x third_party/rust/vendor/build/agentd
    # The upstream build script only consults the pre-staged binary when it
    # detects a CI environment; under Once's hermetic execution neither the
    # `CI` nor the `GITHUB_ACTIONS` markers necessarily reach the child
    # process, so patch the build script to consult the local file
    # unconditionally. The download branch stays as a fallback.
    build_rs="third_party/rust/vendor/microsandbox-filesystem-${microsandbox_version}/build.rs"
    if [[ -f "${build_rs}" ]]; then
      python3 - "${build_rs}" <<'PY'
import re, sys
path = sys.argv[1]
source = open(path).read()
pattern = re.compile(
    r'if std::env::var_os\("CI"\)\.is_some\(\) \|\| std::env::var_os\("GITHUB_ACTIONS"\)\.is_some\(\) \{',
)
replacement = 'if true {'
patched, count = pattern.subn(replacement, source, count=1)
if count == 0:
    raise SystemExit(
        f"expected CI-branch guard to patch in {path}, but did not find it"
    )
open(path, 'w').write(patched)
print(f"prepare-rust-graph-deps: patched {path} to always consult local agentd")
PY
      # Cargo verifies vendored source integrity against a per-crate
      # `.cargo-checksum.json` file; overwrite the checksum for `build.rs`
      # with the new hash so `cargo build --frozen` and Once's own hash
      # verification see the file as authentic.
      checksum_file="third_party/rust/vendor/microsandbox-filesystem-${microsandbox_version}/.cargo-checksum.json"
      if [[ -f "${checksum_file}" ]]; then
        new_hash="$(shasum -a 256 "${build_rs}" | awk '{print $1}')"
        python3 - "${checksum_file}" "${new_hash}" <<'PY'
import json, sys
path, new_hash = sys.argv[1], sys.argv[2]
data = json.load(open(path))
data["files"]["build.rs"] = new_hash
json.dump(data, open(path, 'w'), separators=(",", ":"))
print(f"prepare-rust-graph-deps: refreshed {path} for build.rs")
PY
      fi
    fi
  fi
fi

printf '%s\n' "${fingerprint}" > "${prepared_file}"
