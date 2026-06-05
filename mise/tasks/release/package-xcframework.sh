#!/usr/bin/env bash
#MISE description="Build and package the Once XCFramework"
#USAGE flag "--version <version>" help="Version number used in the asset name"
set -euo pipefail

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

if [[ -z "${version}" ]]; then
  echo "--version is required" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "XCFramework packaging requires macOS" >&2
  exit 1
fi

targets=(
  aarch64-apple-darwin
  x86_64-apple-darwin
  aarch64-apple-ios
  aarch64-apple-ios-sim
  x86_64-apple-ios
)

rustup target add "${targets[@]}"

for target in "${targets[@]}"; do
  cargo build --locked --release --package once --target "${target}"
done

stage_dir="$(mktemp -d)"
keychain_path=""
cleanup() {
  if [[ -n "${keychain_path}" ]]; then
    security delete-keychain "${keychain_path}" >/dev/null 2>&1 || true
  fi
  rm -rf "${stage_dir}"
}
trap cleanup EXIT

mkdir -p "${stage_dir}/macos" "${stage_dir}/ios-simulator" dist

if [[ -n "${APPLE_DEVELOPER_ID_CERTIFICATE_ENCRYPTION_PASSWORD:-}" ]]; then
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
    -pass "pass:${APPLE_DEVELOPER_ID_CERTIFICATE_ENCRYPTION_PASSWORD}"

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
  security set-key-partition-list \
    -S apple-tool:,apple:,codesign: \
    -s \
    -k "${keychain_password}" \
    "${keychain_path}"
fi

lipo -create \
  "target/aarch64-apple-darwin/release/libonce.a" \
  "target/x86_64-apple-darwin/release/libonce.a" \
  -output "${stage_dir}/macos/libonce.a"

lipo -create \
  "target/aarch64-apple-ios-sim/release/libonce.a" \
  "target/x86_64-apple-ios/release/libonce.a" \
  -output "${stage_dir}/ios-simulator/libonce.a"

rm -rf "${stage_dir}/Once.xcframework"
xcodebuild -create-xcframework \
  -library "${stage_dir}/macos/libonce.a" -headers "crates/once/include/Once" \
  -library "target/aarch64-apple-ios/release/libonce.a" -headers "crates/once/include/Once" \
  -library "${stage_dir}/ios-simulator/libonce.a" -headers "crates/once/include/Once" \
  -output "${stage_dir}/Once.xcframework"

if [[ -n "${APPLE_DEVELOPER_ID_CERTIFICATE_ENCRYPTION_PASSWORD:-}" ]]; then
  codesign --force \
    --timestamp \
    --options runtime \
    --sign "${APPLE_DEVELOPER_ID_CERTIFICATE_NAME}" \
    "${stage_dir}/Once.xcframework"
fi

asset="Once-${version}.xcframework.zip"
ditto -c -k --keepParent "${stage_dir}/Once.xcframework" "dist/${asset}"

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
