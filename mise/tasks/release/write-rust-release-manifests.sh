#!/usr/bin/env bash
#MISE description="Inject or clear temporary Once release graph targets"
#USAGE flag "--target <target>" help="Rust target triple to compile"
#USAGE flag "--version <version>" help="Version number to embed in final artifacts"
#USAGE flag "--clear" help="Remove generated release targets"
set -euo pipefail

target=""
version="0.0.0"
clear=false
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
    --clear)
      clear=true
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

manifest_files=(
  once.toml
  crates/once-cas/once.toml
  crates/once-host-tree/once.toml
  crates/once-core/once.toml
  crates/once-events-client/once.toml
  crates/once-frontend/once.toml
  crates/once/once.toml
  crates/once-cli/once.toml
)

remove_generated_blocks() {
  local file tmp
  for file in "${manifest_files[@]}"; do
    tmp="$(mktemp)"
    awk '
      /^# once release generated targets start$/ { skip = 1; next }
      /^# once release generated targets end$/ { skip = 0; next }
      skip != 1 { print }
    ' "${file}" >"${tmp}"
    mv "${tmp}" "${file}"
  done
}

if [[ "${clear}" == true ]]; then
  remove_generated_blocks
  exit 0
fi

if [[ -z "${target}" ]]; then
  echo "--target is required" >&2
  exit 1
fi
if [[ ! "${target}" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "--target must be a Rust target triple" >&2
  exit 1
fi
if [[ ! "${version}" =~ ^[0-9]+[.][0-9]+[.][0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "--version must be a semantic version" >&2
  exit 1
fi

suffix="$(printf '%s' "${target}" | tr '.-' '__')"
dependency_target="cargo_dependencies_${suffix}"

# `once-events-client` is the one first-party crate whose build script is
# load-bearing: it runs `tonic-prost-build` to generate the wire types that
# `src/lib.rs` pulls in with `tonic::include_proto!`. The Cargo resolver
# synthesises a host target per build dependency and names it
# `<crate>-<version>`, and the graph has no way to expand a dependency set
# into a `build_deps` role, so the script has to name those targets. Reading
# the versions out of `Cargo.lock` keeps a dependency bump from silently
# breaking the release build.
locked_version() {
  local crate="$1" version
  version="$(awk -v crate="${crate}" '
    $1 == "name" && $3 == "\"" crate "\"" { found = 1; next }
    found && $1 == "version" { gsub(/"/, "", $3); print $3; exit }
  ' Cargo.lock)"
  if [[ -z "${version}" ]]; then
    echo "could not find ${crate} in Cargo.lock" >&2
    exit 1
  fi
  printf '%s' "${version}"
}

protoc_bin_vendored_version="$(locked_version protoc-bin-vendored)"
tonic_build_version="$(locked_version tonic-build)"
tonic_prost_build_version="$(locked_version tonic-prost-build)"
case "${target}" in
  *-apple-ios*)
    release_flags='["-C", "opt-level=3", "-C", "codegen-units=1"]'
    ;;
  *)
    release_flags='["-C", "opt-level=3", "-C", "codegen-units=1", "-C", "panic=abort"]'
    ;;
esac

remove_generated_blocks

cat >>once.toml <<EOF
# once release generated targets start
[[target]]
name = "${dependency_target}"
kind = "cargo_dependencies"
srcs = [
  "Cargo.toml",
  "Cargo.lock",
  "crates/*/Cargo.toml",
  "third_party/rust/src/formulas/**",
]

[target.attrs]
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
vendor_dir = "third_party/rust/vendor"
target = "${target}"
dep_rustc_flags = ${release_flags}
# once release generated targets end
EOF

cat >>crates/once-cas/once.toml <<EOF
# once release generated targets start
[[target]]
name = "once_cas_${suffix}"
kind = "rust_library"
deps = ["${dependency_target}"]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "once_cas"
crate_root = "src/lib.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once-cas"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once-cas"
CARGO_PKG_NAME = "once-cas"
CARGO_PKG_VERSION = "${version}"
# once release generated targets end
EOF

cat >>crates/once-host-tree/once.toml <<EOF
# once release generated targets start
[[target]]
name = "once_host_tree_${suffix}"
kind = "rust_library"
deps = ["${dependency_target}"]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "once_host_tree"
crate_root = "src/lib.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once-host-tree"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once-host-tree"
CARGO_PKG_NAME = "once-host-tree"
CARGO_PKG_VERSION = "${version}"
# once release generated targets end
EOF

cat >>crates/once-core/once.toml <<EOF
# once release generated targets start
[[target]]
name = "once_core_${suffix}"
kind = "rust_library"
deps = [
  "crates/once-cas/once_cas_${suffix}",
  "crates/once-host-tree/once_host_tree_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "once_core"
crate_root = "src/lib.rs"
edition = "2021"
features = ["default", "remote-microsandbox"]
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once-core"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once-core"
CARGO_PKG_NAME = "once-core"
CARGO_PKG_VERSION = "${version}"
# once release generated targets end
EOF

cat >>crates/once-events-client/once.toml <<EOF
# once release generated targets start
[[target]]
name = "once_events_client_${suffix}"
kind = "rust_library"
deps = [
  "crates/once-core/once_core_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "proto/**/*.proto", "build.rs"]

[target.dependencies]
build_deps = [
  "protoc-bin-vendored-${protoc_bin_vendored_version}",
  "tonic-build-${tonic_build_version}",
  "tonic-prost-build-${tonic_prost_build_version}",
]

[target.attrs]
crate_name = "once_events_client"
crate_root = "src/lib.rs"
edition = "2021"
build_script = "build.rs"
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once-events-client"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once-events-client"
CARGO_PKG_NAME = "once-events-client"
CARGO_PKG_VERSION = "${version}"
# once release generated targets end
EOF

cat >>crates/once-frontend/once.toml <<EOF
# once release generated targets start
[[target]]
name = "once_frontend_${suffix}"
kind = "rust_library"
deps = [
  "crates/once-host-tree/once_host_tree_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "prelude/**/*.star", "prelude/examples/**"]

[target.attrs]
crate_name = "once_frontend"
crate_root = "src/lib.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once-frontend"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once-frontend"
CARGO_PKG_NAME = "once-frontend"
CARGO_PKG_VERSION = "${version}"
# once release generated targets end
EOF

cat >>crates/once/once.toml <<EOF
# once release generated targets start
[[target]]
name = "once_cdylib_${suffix}"
kind = "rust_library"
deps = [
  "crates/once-cas/once_cas_${suffix}",
  "crates/once-host-tree/once_host_tree_${suffix}",
  "crates/once-core/once_core_${suffix}",
  "crates/once-frontend/once_frontend_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "include/**/*", "swift/**/*.swift"]

[target.attrs]
crate_name = "once"
crate_root = "src/lib.rs"
edition = "2021"
crate_type = "cdylib"
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once"
CARGO_PKG_NAME = "once"
CARGO_PKG_VERSION = "${version}"

[[target]]
name = "once_staticlib_${suffix}"
kind = "rust_library"
deps = [
  "crates/once-cas/once_cas_${suffix}",
  "crates/once-host-tree/once_host_tree_${suffix}",
  "crates/once-core/once_core_${suffix}",
  "crates/once-frontend/once_frontend_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "include/**/*", "swift/**/*.swift"]

[target.attrs]
crate_name = "once"
crate_root = "src/lib.rs"
edition = "2021"
crate_type = "staticlib"
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once"
CARGO_PKG_NAME = "once"
CARGO_PKG_VERSION = "${version}"
# once release generated targets end
EOF

cat >>crates/once-cli/once.toml <<EOF
# once release generated targets start
[[target]]
name = "once_cli_${suffix}"
kind = "rust_binary"
deps = [
  "crates/once-cas/once_cas_${suffix}",
  "crates/once-host-tree/once_host_tree_${suffix}",
  "crates/once-core/once_core_${suffix}",
  "crates/once-events-client/once_events_client_${suffix}",
  "crates/once-frontend/once_frontend_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "once"
crate_root = "src/main.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${release_flags}
cargo_package = "once-cli"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/once-cli"
CARGO_PKG_NAME = "once-cli"
CARGO_PKG_VERSION = "0.0.0"
ONCE_VERSION = "${version}"
# once release generated targets end
EOF
