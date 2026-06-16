#!/usr/bin/env bash
#MISE description="Build and package the Once XCFramework"
#USAGE flag "--version <version>" help="Version number to validate the release input"
set -Eeuo pipefail

version=""
while (($# > 0)); do
  case "$1" in
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

version="${version//[[:space:]]/}"
if [[ -z "${version}" ]]; then
  echo "--version is required" >&2
  exit 1
fi

if [[ ! "${version}" =~ ^[0-9]+[.][0-9]+[.][0-9]+([-+][0-9A-Za-z.-]+)?$ ]]; then
  echo "--version must be a semantic version" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "XCFramework packaging requires macOS" >&2
  exit 1
fi

export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-13.0}"

require_tool() {
  local tool="$1"
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} is required" >&2
    exit 1
  fi
}

require_file() {
  local path="$1"
  if [[ ! -f "${path}" ]]; then
    echo "expected file does not exist: ${path}" >&2
    exit 1
  fi
}

require_lipo_archs() {
  local path="$1"
  shift
  local archs
  archs="$(lipo -archs "${path}")"
  echo "${path}: ${archs}"
  for arch in "$@"; do
    if [[ " ${archs} " != *" ${arch} "* ]]; then
      echo "${path} is missing ${arch}" >&2
      exit 1
    fi
  done
}

target_suffix() {
  printf '%s' "$1" | tr '.-' '__'
}

staticlib_path() {
  local target="$1"
  local suffix
  suffix="$(target_suffix "${target}")"
  printf '.once/out/crates/once/once_staticlib_%s/libonce.a\n' "${suffix}"
}

build_target() {
  local target="$1"
  local suffix target_id
  suffix="$(target_suffix "${target}")"
  target_id="crates/once/once_staticlib_${suffix}"
  ./mise/tasks/release/write-rust-release-manifests.sh --target "${target}" --version "${version}"
  if ! "${once_bin}" build "${target_id}" --format json --quiet; then
    echo "once build failed for ${target}" >&2
    exit 1
  fi
}

for tool in cargo ditto lipo rustup swiftc xcodebuild; do
  require_tool "${tool}"
done

targets=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  aarch64-apple-ios
  aarch64-apple-ios-sim
  x86_64-apple-ios
)

stage_dir=""
keychain_path=""
certificate_path=""
cleanup() {
  ./mise/tasks/release/write-rust-release-manifests.sh --clear >/dev/null 2>&1 || true
  if [[ -n "${certificate_path}" ]]; then
    rm -f "${certificate_path}"
  fi
  if [[ -n "${keychain_path}" ]]; then
    security delete-keychain "${keychain_path}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${stage_dir}" ]]; then
    rm -rf "${stage_dir}"
  fi
}
trap cleanup EXIT

rustup target add "${targets[@]}"
once_bin="$(./mise/tasks/release/bootstrap-once.sh)"
./mise/tasks/release/prepare-rust-graph-deps.sh

for target in "${targets[@]}"; do
  build_target "${target}"
done

for target in "${targets[@]}"; do
  require_file "$(staticlib_path "${target}")"
done

stage_dir="$(mktemp -d)"

mkdir -p "${stage_dir}/macos" "${stage_dir}/ios-simulator" dist

if [[ -n "${APPLE_DEVELOPER_ID_CERTIFICATE_ENCRYPTION_PASSWORD:-}" ]]; then
  for tool in openssl security xcrun; do
    require_tool "${tool}"
  done

  required_env=(
    APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD
    APPLE_DEVELOPER_ID_CERTIFICATE_NAME
    APPLE_ID
    APPLE_TEAM_ID
    APPLE_APP_SPECIFIC_PASSWORD
  )
  for name in "${required_env[@]}"; do
    if [[ -z "${!name:-}" ]]; then
      echo "${name} is required when signing is enabled" >&2
      exit 1
    fi
  done

  certificate_path="${stage_dir}/developer-id-application.p12"
  openssl enc -d -aes-256-cbc -pbkdf2 -iter 100000 \
    -in certificates/developer-id-application.p12.enc \
    -out "${certificate_path}" \
    -pass env:APPLE_DEVELOPER_ID_CERTIFICATE_ENCRYPTION_PASSWORD
  chmod 600 "${certificate_path}"
  openssl pkcs12 \
    -in "${certificate_path}" \
    -passin env:APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD \
    -noout

  keychain_path="${stage_dir}/signing.keychain"
  keychain_password="$(uuidgen)"
  security create-keychain -p "${keychain_password}" "${keychain_path}"
  security set-keychain-settings -lut 21600 "${keychain_path}"
  security default-keychain -s "${keychain_path}"
  security unlock-keychain -p "${keychain_password}" "${keychain_path}"
  security import "${certificate_path}" \
    -P "${APPLE_DEVELOPER_ID_CERTIFICATE_PASSWORD}" \
    -A \
    -t cert \
    -f pkcs12 \
    -k "${keychain_path}"
  if ! security find-identity -v -p codesigning "${keychain_path}" | grep -F "${APPLE_DEVELOPER_ID_CERTIFICATE_NAME}" >/dev/null; then
    echo "imported certificate does not match APPLE_DEVELOPER_ID_CERTIFICATE_NAME" >&2
    exit 1
  fi
  rm -f "${certificate_path}"
  security set-key-partition-list \
    -S apple-tool:,apple:,codesign: \
    -s \
    -k "${keychain_password}" \
    "${keychain_path}"
fi

lipo -create \
  "$(staticlib_path aarch64-apple-darwin)" \
  "$(staticlib_path x86_64-apple-darwin)" \
  -output "${stage_dir}/macos/libonce.a"
require_lipo_archs "${stage_dir}/macos/libonce.a" arm64 x86_64

lipo -create \
  "$(staticlib_path aarch64-apple-ios-sim)" \
  "$(staticlib_path x86_64-apple-ios)" \
  -output "${stage_dir}/ios-simulator/libonce.a"
require_lipo_archs "$(staticlib_path aarch64-apple-ios)" arm64
require_lipo_archs "${stage_dir}/ios-simulator/libonce.a" arm64 x86_64

release_dir="${stage_dir}/release"
mkdir -p "${release_dir}"

rm -rf "${release_dir}/Once.xcframework"
xcodebuild -create-xcframework \
  -library "${stage_dir}/macos/libonce.a" -headers "crates/once/include/Once" \
  -library "$(staticlib_path aarch64-apple-ios)" -headers "crates/once/include/Once" \
  -library "${stage_dir}/ios-simulator/libonce.a" -headers "crates/once/include/Once" \
  -output "${release_dir}/Once.xcframework"

if [[ ! -d "${release_dir}/Once.xcframework" ]]; then
  echo "xcodebuild did not create Once.xcframework" >&2
  exit 1
fi

swiftc -typecheck \
  -module-name OnceBridge \
  -I crates/once/include/Once \
  crates/once/swift/Once.swift

if [[ -n "${APPLE_DEVELOPER_ID_CERTIFICATE_ENCRYPTION_PASSWORD:-}" ]]; then
  codesign --force \
    --timestamp \
    --options runtime \
    --sign "${APPLE_DEVELOPER_ID_CERTIFICATE_NAME}" \
    "${release_dir}/Once.xcframework"
  codesign --verify --strict --verbose=2 "${release_dir}/Once.xcframework"
fi

asset="Once.xcframework.zip"
(
  cd "${release_dir}"
  ditto -c -k --keepParent Once.xcframework "${OLDPWD}/dist/${asset}"
)

if [[ -n "${APPLE_DEVELOPER_ID_CERTIFICATE_ENCRYPTION_PASSWORD:-}" ]]; then
  xcrun notarytool submit "dist/${asset}" \
    --wait \
    --apple-id "${APPLE_ID}" \
    --team-id "${APPLE_TEAM_ID}" \
    --password "${APPLE_APP_SPECIFIC_PASSWORD}" \
    --output-format json
fi

if command -v sha256sum >/dev/null 2>&1; then
  (cd dist && sha256sum "${asset}" > "${asset}.sha256")
else
  (cd dist && shasum -a 256 "${asset}" > "${asset}.sha256")
fi
