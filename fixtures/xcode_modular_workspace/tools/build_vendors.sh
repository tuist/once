#!/bin/sh
# Builds the prebuilt dynamic XCFrameworks and the binary macro plugin this
# fixture's project references. Binary Swift modules are toolchain-locked, so
# they are produced at spec time with the host toolchain instead of being
# checked in.
#
# Usage: build_vendors.sh <vendor-dir>
set -eu

VENDOR_DIR="$1"
TOOLS_DIR="$(cd "$(dirname "$0")" && pwd)"
SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"
SWIFTC="$(xcrun --sdk iphonesimulator --find swiftc)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
mkdir -p "$VENDOR_DIR"

# build_dynamic_framework <name> <swift-source>
build_dynamic_framework() {
    name="$1"
    fw="$SCRATCH/$name/$name.framework"
    modules="$fw/Modules/$name.swiftmodule"
    mkdir -p "$modules"
    printf '%s\n' "$2" > "$SCRATCH/$name/$name.swift"
    "$SWIFTC" -sdk "$SDK" -target arm64-apple-ios16.0-simulator \
        -module-name "$name" -parse-as-library -emit-library -enable-library-evolution -swift-version 5 \
        -emit-module-interface-path "$modules/arm64-apple-ios-simulator.swiftinterface" \
        -emit-module-path "$modules/arm64-apple-ios-simulator.swiftmodule" \
        -Xlinker -install_name -Xlinker "@rpath/$name.framework/$name" \
        -o "$fw/$name" "$SCRATCH/$name/$name.swift"
    rm -f "$modules"/*.swiftsourceinfo "$modules"/*.private.swiftinterface "$modules"/*.package.swiftinterface
    cat > "$fw/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>$name</string>
  <key>CFBundleIdentifier</key><string>dev.once.fixture.$name</string>
  <key>CFBundlePackageType</key><string>FMWK</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>MinimumOSVersion</key><string>16.0</string>
</dict>
</plist>
PLIST
    rm -rf "$VENDOR_DIR/$name.xcframework"
    xcodebuild -create-xcframework -framework "$fw" -output "$VENDOR_DIR/$name.xcframework" >/dev/null
}

build_dynamic_framework Networking 'public enum NetworkingMock { public static let name = "Networking" }'
build_dynamic_framework Analytics 'public enum AnalyticsMock { public static let name = "Analytics" }'
build_dynamic_framework MacroSupport '@freestanding(expression)
public macro mockIdentifier(_ value: String) -> String = #externalMacro(module: "MacroSupportPlugin", type: "MockIdentifierMacro")'

xcrun --sdk macosx swiftc -O -o "$VENDOR_DIR/MacroSupportPlugin" "$TOOLS_DIR/MacroSupportPlugin.swift"
