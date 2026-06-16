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

rm -rf third_party/rust/vendor
mkdir -p third_party/rust
mise exec -- cargo vendor --locked --versioned-dirs third_party/rust/vendor >/tmp/once-cargo-vendor-config

metadata_args=(metadata --locked --format-version 1)
if [[ -n "${target}" ]]; then
  metadata_args+=(--filter-platform "${target}")
fi
metadata="$(mise exec -- cargo "${metadata_args[@]}")"
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
