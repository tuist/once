#!/usr/bin/env bash
#MISE description="Build shared Once SDK libraries for Ruby and JavaScript packages"
#USAGE flag "--target <target>" help="Rust target triple to compile"
#USAGE flag "--version <version>" help="Version number to embed in the shared library"
set -euo pipefail

release_tasks_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

target=""
version="0.0.0"
while (($# > 0)); do
  case "$1" in
    --target)
      target="${2}"
      shift 2
      ;;
    --version)
      version="${2}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

case "${target}" in
  aarch64-apple-darwin)
    platform="darwin-arm64"
    library="libonce.dylib"
    ;;
  x86_64-apple-darwin)
    platform="darwin-x64"
    library="libonce.dylib"
    ;;
  aarch64-unknown-linux-gnu)
    platform="linux-arm64"
    library="libonce.so"
    ;;
  x86_64-unknown-linux-gnu)
    platform="linux-x64"
    library="libonce.so"
    ;;
  x86_64-pc-windows-msvc)
    platform="win32-x64"
    library="once.dll"
    ;;
  *)
    if [[ -z "${target}" ]]; then
      echo "--target is required" >&2
      exit 1
    fi
    echo "unsupported SDK package target: ${target}" >&2
    exit 1
    ;;
esac

if [[ ! "${version}" =~ ^[0-9]+[.][0-9]+[.][0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "--version must be a semantic version" >&2
  exit 1
fi

target_suffix() {
  printf '%s' "$1" | tr '.-' '__'
}

cleanup() {
  "${release_tasks_dir}/write-rust-release-manifests.sh" --clear >/dev/null 2>&1 || true
}
trap cleanup EXIT

once_bin="$("${release_tasks_dir}/bootstrap-once.sh")"
"${release_tasks_dir}/prepare-rust-graph-deps.sh" --target "${target}"
"${release_tasks_dir}/write-rust-release-manifests.sh" --target "${target}" --version "${version}"

suffix="$(target_suffix "${target}")"
target_id="crates/once/once_cdylib_${suffix}"
"${once_bin}" build "${target_id}" --format json --quiet

source=".once/out/${target_id}/${library}"
if [[ ! -f "${source}" ]]; then
  echo "expected shared library does not exist: ${source}" >&2
  exit 1
fi

for package in packages/js packages/ruby; do
  mkdir -p "${package}/prebuilds/${platform}"
  cp "${source}" "${package}/prebuilds/${platform}/${library}"
done
