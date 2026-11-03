#!/usr/bin/env bash
#MISE description="Build and package a release archive for a target triple"
#USAGE flag "--target <target>" help="Rust target triple to compile"
#USAGE flag "--version <version>" help="Version number to embed in the binary and asset name"
set -euo pipefail

release_tasks_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

target=""
version=""
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

if [[ -z "${target}" || -z "${version}" ]]; then
  echo "--target and --version are required" >&2
  exit 1
fi

if [[ ! "${version}" =~ ^[0-9]+[.][0-9]+[.][0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "--version must be a semantic version" >&2
  exit 1
fi

target_suffix() {
  printf '%s' "$1" | tr '.-' '__'
}

stage_dir=""
cleanup() {
  "${release_tasks_dir}/write-rust-release-manifests.sh" --clear >/dev/null 2>&1 || true
  if [[ -n "${stage_dir}" ]]; then
    rm -rf "${stage_dir}"
  fi
}
trap cleanup EXIT

mkdir -p dist
once_bin="$("${release_tasks_dir}/bootstrap-once.sh")"
"${release_tasks_dir}/prepare-rust-graph-deps.sh" --target "${target}"
"${release_tasks_dir}/write-rust-release-manifests.sh" --target "${target}" --version "${version}"

# Windows uses the .exe suffix; everything else ships a bare binary.
case "${target}" in
  *-windows-*)
    bin_name="once.exe"
    ;;
  *)
    bin_name="once"
    ;;
esac

suffix="$(target_suffix "${target}")"
target_id="crates/once-cli/once_cli_${suffix}"
"${once_bin}" build "${target_id}" --format json --quiet
source=".once/out/${target_id}/${bin_name}"
if [[ ! -f "${source}" ]]; then
  echo "expected binary does not exist: ${source}" >&2
  exit 1
fi

# Linux binaries take their glibc symbol versions from the build host's
# headers. The release runners use Ubuntu 22.04 so the binary loads on
# glibc 2.35 userlands; fail instead of silently raising that floor.
linux_glibc_ceiling="2.35"
case "${target}" in
  *-linux-gnu)
    required_glibc="$(objdump -T "${source}" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)"
    highest="$(printf '%s\n%s\n' "${required_glibc:-0}" "${linux_glibc_ceiling}" | sort -V | tail -n 1)"
    if [[ "${highest}" != "${linux_glibc_ceiling}" ]]; then
      echo "${source} requires glibc ${required_glibc}, above the ${linux_glibc_ceiling} release baseline; build it on an older userland" >&2
      exit 1
    fi
    ;;
esac

stage_dir="$(mktemp -d)"
cp "${source}" "${stage_dir}/${bin_name}"

# tar.gz everywhere, modern Windows ships tar(1) and `mise install`'s
# ubi backend handles either format. Keeping one extension across
# platforms makes the asset-naming pattern downstream tools key off
# (mise/aqua/ubi) trivially predictable.
asset="once-${version}-${target}.tar.gz"
tar -C "${stage_dir}" -czf "dist/${asset}" "${bin_name}"

# sha256: sha256sum on Linux and Git-Bash-on-Windows; shasum -a 256 on macOS.
if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "${asset}" > "${asset}.sha256")
else
  (cd dist && shasum -a 256 "${asset}" > "${asset}.sha256")
fi
