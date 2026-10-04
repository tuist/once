#!/bin/sh
set -eu
cd "$(dirname "$0")"
vendor_arch="$(uname -m)"
for vendor_slice in macos ios-simulator; do
  case "$vendor_slice" in
    macos) vendor_triple="$vendor_arch-apple-macos13.0"; vendor_index=0; vendor_sdk=macosx ;;
    ios-simulator) vendor_triple="$vendor_arch-apple-ios17.0-simulator"; vendor_index=1; vendor_sdk=iphonesimulator ;;
  esac
  vendor_bundle="GraphVendor.xcframework/$vendor_slice/GraphVendor.framework"
  mkdir -p "$vendor_bundle/Headers" "$vendor_bundle/Modules"
  cp GraphVendor.h "$vendor_bundle/Headers/"
  cp module.modulemap "$vendor_bundle/Modules/"
  vendor_sdkroot="$(xcrun --sdk "$vendor_sdk" --show-sdk-path)"
  xcrun clang -isysroot "$vendor_sdkroot" -target "$vendor_triple" -c GraphVendor.c -o "GraphVendor.xcframework/$vendor_slice/vendor.o"
  xcrun ar rcs "$vendor_bundle/GraphVendor" "GraphVendor.xcframework/$vendor_slice/vendor.o"
  plutil -replace "AvailableLibraries.$vendor_index.SupportedArchitectures.0" -string "$vendor_arch" GraphVendor.xcframework/Info.plist
done
