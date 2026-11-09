#!/usr/bin/env bash
#MISE description="Build one Apple target's static library for the Once XCFramework"
#USAGE flag "--target <target>" help="Apple Rust target triple to compile"
#USAGE flag "--version <version>" help="Version number to embed in the library"
#USAGE flag "--output <dir>" help="Directory that receives <target>/libonce.a"
set -euo pipefail

release_tasks_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

target=""
version=""
output="dist/xcframework-slices"
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
    --output)
      output="${2}"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

case "${target}" in
  aarch64-apple-darwin | x86_64-apple-darwin | aarch64-apple-ios | aarch64-apple-ios-sim | x86_64-apple-ios) ;;
  *)
    echo "--target must be an XCFramework slice target, got: ${target}" >&2
    exit 1
    ;;
esac

if [[ ! "${version}" =~ ^[0-9]+[.][0-9]+[.][0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "--version must be a semantic version" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "XCFramework slices require macOS" >&2
  exit 1
fi

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-13.0}"

cleanup() {
  "${release_tasks_dir}/write-rust-release-manifests.sh" --clear >/dev/null 2>&1 || true
}
trap cleanup EXIT

rustup target add "${target}"
once_bin="$("${release_tasks_dir}/bootstrap-once.sh")"
"${release_tasks_dir}/prepare-rust-graph-deps.sh" --target "${target}"
"${release_tasks_dir}/write-rust-release-manifests.sh" --target "${target}" --version "${version}"

suffix="$(printf '%s' "${target}" | tr '.-' '__')"
target_id="crates/once/once_staticlib_${suffix}"
if ! "${once_bin}" build "${target_id}" --format json --quiet; then
  echo "once build failed for ${target}" >&2
  exit 1
fi

source=".once/out/${target_id}/libonce.a"
if [[ ! -f "${source}" ]]; then
  echo "expected static library does not exist: ${source}" >&2
  exit 1
fi

mkdir -p "${output}/${target}"
cp "${source}" "${output}/${target}/libonce.a"
