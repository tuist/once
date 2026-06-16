#!/usr/bin/env bash
#MISE description="Resolve a released Once binary for bootstrapping graph builds"
set -euo pipefail

host_triple() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}:${arch}" in
    Darwin:arm64 | Darwin:aarch64)
      printf '%s\n' "aarch64-apple-darwin"
      ;;
    Darwin:x86_64)
      printf '%s\n' "x86_64-apple-darwin"
      ;;
    Linux:x86_64)
      printf '%s\n' "x86_64-unknown-linux-gnu"
      ;;
    Linux:arm64 | Linux:aarch64)
      printf '%s\n' "aarch64-unknown-linux-gnu"
      ;;
    MINGW*:x86_64 | MSYS*:x86_64 | CYGWIN*:x86_64)
      printf '%s\n' "x86_64-pc-windows-msvc"
      ;;
    *)
      echo "unsupported bootstrap host: ${os} ${arch}" >&2
      exit 1
      ;;
  esac
}

latest_release() {
  if [[ -n "${ONCE_BOOTSTRAP_VERSION:-}" ]]; then
    printf '%s\n' "${ONCE_BOOTSTRAP_VERSION}"
    return
  fi
  if command -v gh >/dev/null 2>&1; then
    gh release view --repo tuist/once --json tagName --jq .tagName
    return
  fi
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL https://api.github.com/repos/tuist/once/releases/latest |
      sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
      head -n 1
    return
  fi
  echo "gh or curl is required to resolve the latest Once release" >&2
  exit 1
}

fallback_cargo_build() {
  echo "falling back to a Cargo-built bootstrap Once for this host" >&2
  cargo build --locked --release --package once-cli >&2
  case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*)
      printf '%s\n' "target/release/once.exe"
      ;;
    *)
      printf '%s\n' "target/release/once"
      ;;
  esac
}

if [[ -n "${ONCE_BIN:-}" ]]; then
  if [[ ! -x "${ONCE_BIN}" ]]; then
    echo "ONCE_BIN is not executable: ${ONCE_BIN}" >&2
    exit 1
  fi
  printf '%s\n' "${ONCE_BIN}"
  exit 0
fi

target="$(host_triple)"
version="$(latest_release)"
version="${version#v}"
if [[ -z "${version}" ]]; then
  echo "could not determine latest Once release" >&2
  exit 1
fi

exe="once"
case "${target}" in
  *-windows-*)
    exe="once.exe"
    ;;
esac

cache_dir="target/once-bootstrap/${version}-${target}"
binary="${cache_dir}/${exe}"
if [[ -x "${binary}" ]]; then
  printf '%s\n' "${binary}"
  exit 0
fi

mkdir -p "${cache_dir}"
asset="once-${version}-${target}.tar.gz"
archive="${cache_dir}/${asset}"
url="https://github.com/tuist/once/releases/download/${version}/${asset}"
if command -v curl >/dev/null 2>&1 && curl -fL "${url}" -o "${archive}" >&2; then
  tar -C "${cache_dir}" -xzf "${archive}"
  if [[ -x "${binary}" ]]; then
    printf '%s\n' "${binary}"
    exit 0
  fi
fi

fallback_cargo_build
