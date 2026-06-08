#!/usr/bin/env bash
#MISE description="Build shared Once SDK libraries for Ruby and JavaScript packages"
#USAGE flag "--target <target>" help="Rust target triple to compile"
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

cargo build --locked --release --package once --target "${target}"

source="target/${target}/release/${library}"
if [[ ! -f "${source}" ]]; then
  echo "expected shared library does not exist: ${source}" >&2
  exit 1
fi

for package in packages/js packages/ruby; do
  mkdir -p "${package}/prebuilds/${platform}"
  cp "${source}" "${package}/prebuilds/${platform}/${library}"
done
