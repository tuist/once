#!/usr/bin/env bash
#MISE description="Inject or clear temporary Struo release graph targets"
#USAGE flag "--target <target>" help="Rust target triple to compile"
#USAGE flag "--version <version>" help="Version number to embed in final artifacts"
#USAGE flag "--bootstrap" help="Generate bootstrap targets that optimize dependencies but not Struo's own crates"
#USAGE flag "--clear" help="Remove generated release targets"
set -euo pipefail

target=""
version="0.0.0"
clear=false
bootstrap=false
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
    --bootstrap)
      bootstrap=true
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

manifest_files=(
  struo.toml
  crates/struo-cas/struo.toml
  crates/struo-host-tree/struo.toml
  crates/struo-core/struo.toml
  crates/struo-presentation/struo.toml
  crates/struo-events-client/struo.toml
  crates/struo-frontend/struo.toml
  crates/struo/struo.toml
  crates/struo-cli/struo.toml
)

remove_generated_blocks() {
  local file tmp
  for file in "${manifest_files[@]}"; do
    tmp="$(mktemp)"
    awk '
      /^# struo release generated targets start$/ { skip = 1; next }
      /^# struo release generated targets end$/ { skip = 0; next }
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

case "${target}" in
  *-apple-ios*)
    release_flags='["-C", "opt-level=3", "-C", "codegen-units=1"]'
    ;;
  *)
    release_flags='["-C", "opt-level=3", "-C", "codegen-units=1", "-C", "panic=abort"]'
    ;;
esac
dependency_flags="${release_flags}"
crate_flags="${release_flags}"

# The bootstrap Struo only drives graph builds, so its own crates skip
# optimization like the Cargo bootstrap profile, while dependencies such
# as the Starlark interpreter and hashers stay optimized.
if [[ "${bootstrap}" == true ]]; then
  suffix="bootstrap_${suffix}"
  dependency_flags='["-C", "opt-level=3"]'
  crate_flags='[]'
fi
dependency_target="cargo_dependencies_${suffix}"

remove_generated_blocks

cat >>struo.toml <<EOF
# struo release generated targets start
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
dep_rustc_flags = ${dependency_flags}
# struo release generated targets end
EOF

cat >>crates/struo-cas/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_cas_${suffix}"
kind = "rust_library"
deps = ["${dependency_target}"]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "struo_cas"
crate_root = "src/lib.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo-cas"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo-cas"
CARGO_PKG_NAME = "struo-cas"
CARGO_PKG_VERSION = "${version}"
# struo release generated targets end
EOF

cat >>crates/struo-host-tree/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_host_tree_${suffix}"
kind = "rust_library"
deps = ["${dependency_target}"]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "struo_host_tree"
crate_root = "src/lib.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo-host-tree"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo-host-tree"
CARGO_PKG_NAME = "struo-host-tree"
CARGO_PKG_VERSION = "${version}"
# struo release generated targets end
EOF

cat >>crates/struo-presentation/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_presentation_${suffix}"
kind = "rust_library"
deps = ["${dependency_target}"]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "struo_presentation"
crate_root = "src/lib.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo-presentation"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo-presentation"
CARGO_PKG_NAME = "struo-presentation"
CARGO_PKG_VERSION = "${version}"
# struo release generated targets end
EOF

cat >>crates/struo-core/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_core_${suffix}"
kind = "rust_library"
deps = [
  "crates/struo-presentation/struo_presentation_${suffix}",
  "crates/struo-cas/struo_cas_${suffix}",
  "crates/struo-host-tree/struo_host_tree_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "struo_core"
crate_root = "src/lib.rs"
edition = "2021"
features = ["default", "remote-microsandbox"]
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo-core"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo-core"
CARGO_PKG_NAME = "struo-core"
CARGO_PKG_VERSION = "${version}"
# struo release generated targets end
EOF

cat >>crates/struo-events-client/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_events_client_${suffix}"
kind = "rust_library"
deps = [
  "crates/struo-core/struo_core_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "proto/**/*.proto", "build.rs"]

[target.attrs]
crate_name = "struo_events_client"
crate_root = "src/lib.rs"
edition = "2021"
build_script = "build.rs"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo-events-client"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo-events-client"
CARGO_PKG_NAME = "struo-events-client"
CARGO_PKG_VERSION = "${version}"
# struo release generated targets end
EOF

cat >>crates/struo-frontend/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_frontend_${suffix}"
kind = "rust_library"
deps = [
  "crates/struo-presentation/struo_presentation_${suffix}",
  "crates/struo-host-tree/struo_host_tree_${suffix}",
  "crates/struo-core/struo_core_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "prelude/**/*.star", "prelude/examples/**"]

[target.attrs]
crate_name = "struo_frontend"
crate_root = "src/lib.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo-frontend"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo-frontend"
CARGO_PKG_NAME = "struo-frontend"
CARGO_PKG_VERSION = "${version}"
# struo release generated targets end
EOF

cat >>crates/struo/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_cdylib_${suffix}"
kind = "rust_library"
deps = [
  "crates/struo-cas/struo_cas_${suffix}",
  "crates/struo-host-tree/struo_host_tree_${suffix}",
  "crates/struo-core/struo_core_${suffix}",
  "crates/struo-frontend/struo_frontend_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "include/**/*", "swift/**/*.swift"]

[target.attrs]
crate_name = "struo"
crate_root = "src/lib.rs"
edition = "2021"
crate_type = "cdylib"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo"
CARGO_PKG_NAME = "struo"
CARGO_PKG_VERSION = "${version}"

[[target]]
name = "struo_staticlib_${suffix}"
kind = "rust_library"
deps = [
  "crates/struo-cas/struo_cas_${suffix}",
  "crates/struo-host-tree/struo_host_tree_${suffix}",
  "crates/struo-core/struo_core_${suffix}",
  "crates/struo-frontend/struo_frontend_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs", "include/**/*", "swift/**/*.swift"]

[target.attrs]
crate_name = "struo"
crate_root = "src/lib.rs"
edition = "2021"
crate_type = "staticlib"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo"
CARGO_PKG_NAME = "struo"
CARGO_PKG_VERSION = "${version}"
# struo release generated targets end
EOF

cat >>crates/struo-cli/struo.toml <<EOF
# struo release generated targets start
[[target]]
name = "struo_cli_${suffix}"
kind = "rust_binary"
deps = [
  "crates/struo-cas/struo_cas_${suffix}",
  "crates/struo-host-tree/struo_host_tree_${suffix}",
  "crates/struo-core/struo_core_${suffix}",
  "crates/struo-events-client/struo_events_client_${suffix}",
  "crates/struo-frontend/struo_frontend_${suffix}",
  "${dependency_target}",
]
srcs = ["src/**/*.rs"]

[target.attrs]
crate_name = "struo"
crate_root = "src/main.rs"
edition = "2021"
target = "${target}"
rustc_flags = ${crate_flags}
cargo_package = "struo-cli"

[target.attrs.rustc_env]
CARGO_MANIFEST_DIR = "crates/struo-cli"
CARGO_PKG_NAME = "struo-cli"
CARGO_PKG_VERSION = "0.0.0"
STRUO_VERSION = "${version}"
# struo release generated targets end
EOF
