#!/bin/sh
# Builds the prebuilt vendor XCFrameworks this fixture's generated project
# references. Binary Swift modules are toolchain-locked, so the bundles are
# produced at spec time with the host toolchain instead of being checked in.
#
# Usage: build_vendors.sh <vendor-dir> <remote-vendor-dir>
#
# <vendor-dir>        receives the workspace-relative bundles (Vendor/...)
# <remote-vendor-dir> receives VendorRemote.xcframework, referenced by the
#                     project through an absolute out-of-workspace path
set -eu

VENDOR_DIR="$1"
REMOTE_DIR="$2"

SDK="$(xcrun --sdk iphonesimulator --show-sdk-path)"
SWIFTC="$(xcrun --sdk iphonesimulator --find swiftc)"
CLANG="$(xcrun --sdk iphonesimulator --find clang)"
MIN_OS="15.0"
ARCHS="arm64 x86_64"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

xcframework_plist() {
    # $1 = destination Info.plist, $2 = LibraryPath, $3 = BinaryPath (optional)
    binary_entry=""
    if [ -n "${3:-}" ]; then
        binary_entry="
            <key>BinaryPath</key>
            <string>$3</string>"
    fi
    cat > "$1" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>AvailableLibraries</key>
    <array>
        <dict>$binary_entry
            <key>LibraryIdentifier</key>
            <string>ios-arm64_x86_64-simulator</string>
            <key>LibraryPath</key>
            <string>$2</string>
            <key>SupportedArchitectures</key>
            <array>
                <string>arm64</string>
                <string>x86_64</string>
            </array>
            <key>SupportedPlatform</key>
            <string>ios</string>
            <key>SupportedPlatformVariant</key>
            <string>simulator</string>
        </dict>
    </array>
    <key>CFBundlePackageType</key>
    <string>XFWK</string>
    <key>XCFrameworkFormatVersion</key>
    <string>1.0</string>
</dict>
</plist>
PLIST
}

framework_plist() {
    cat > "$1" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$2</string>
    <key>CFBundleIdentifier</key>
    <string>dev.once.fixture.$2</string>
    <key>CFBundlePackageType</key>
    <string>FMWK</string>
</dict>
</plist>
PLIST
}

# build_swift_framework <name> <dest-root> <interface-only: yes/no> <extra swiftc args...>
# Reads Swift sources from "$SCRATCH/src/<name>" and extra C/ObjC objects from
# "$SCRATCH/obj/<name>/<arch>" when present.
build_swift_framework() {
    name="$1"; dest_root="$2"; interface_only="$3"; shift 3
    slice="$dest_root/$name.xcframework/ios-arm64_x86_64-simulator"
    fw="$slice/$name.framework"
    module_dir="$fw/Modules/$name.swiftmodule"
    mkdir -p "$module_dir"
    thin_archives=""
    for arch in $ARCHS; do
        out="$SCRATCH/out/$name/$arch"
        mkdir -p "$out"
        emit_module_args="-emit-module -emit-module-path $module_dir/$arch-apple-ios-simulator.swiftmodule"
        if [ "$interface_only" = "yes" ]; then
            emit_module_args="$emit_module_args -enable-library-evolution -swift-version 5 -emit-module-interface-path $module_dir/$arch-apple-ios-simulator.swiftinterface"
        fi
        # shellcheck disable=SC2086
        "$SWIFTC" -parse-as-library -emit-object \
            -module-name "$name" \
            $emit_module_args \
            -sdk "$SDK" -target "$arch-apple-ios$MIN_OS-simulator" \
            "$@" \
            -o "$out/$name.o" \
            "$SCRATCH/src/$name"/*.swift
        objects="$out/$name.o"
        if [ -d "$SCRATCH/obj/$name/$arch" ]; then
            objects="$objects $(echo "$SCRATCH/obj/$name/$arch"/*.o)"
        fi
        # shellcheck disable=SC2086
        ar crs "$out/lib.a" $objects
        thin_archives="$thin_archives $out/lib.a"
    done
    # shellcheck disable=SC2086
    lipo -create $thin_archives -output "$fw/$name"
    if [ "$interface_only" = "yes" ]; then
        rm -f "$module_dir"/*.swiftmodule "$module_dir"/*.swiftdoc "$module_dir"/*.abi.json
    fi
    printf 'framework module %s {\n    export *\n}\n' "$name" > "$fw/Modules/module.modulemap"
    framework_plist "$fw/Info.plist" "$name"
    xcframework_plist "$dest_root/$name.xcframework/Info.plist" "$name.framework" "$name.framework/$name"
}

# build_swift_static_library <name> <dest-root>
# Library-layout XCFramework: libNAME.a plus a loose swiftmodule directory at
# the slice root, resolved by consumers through SWIFT_INCLUDE_PATHS.
build_swift_static_library() {
    name="$1"; dest_root="$2"
    slice="$dest_root/$name.xcframework/ios-arm64_x86_64-simulator"
    module_dir="$slice/$name.swiftmodule"
    mkdir -p "$module_dir"
    thin_archives=""
    for arch in $ARCHS; do
        out="$SCRATCH/out/$name/$arch"
        mkdir -p "$out"
        "$SWIFTC" -parse-as-library -emit-object \
            -module-name "$name" \
            -emit-module -emit-module-path "$module_dir/$arch-apple-ios-simulator.swiftmodule" \
            -sdk "$SDK" -target "$arch-apple-ios$MIN_OS-simulator" \
            -o "$out/$name.o" \
            "$SCRATCH/src/$name"/*.swift
        ar crs "$out/lib.a" "$out/$name.o"
        thin_archives="$thin_archives $out/lib.a"
    done
    # shellcheck disable=SC2086
    lipo -create $thin_archives -output "$slice/lib$name.a"
    xcframework_plist "$dest_root/$name.xcframework/Info.plist" "lib$name.a" ""
}

compile_c_object() {
    # $1 = module, $2 = source, $3 = object basename, extra args follow
    module="$1"; source="$2"; object="$3"; shift 3
    for arch in $ARCHS; do
        mkdir -p "$SCRATCH/obj/$module/$arch"
        "$CLANG" -c -isysroot "$SDK" -target "$arch-apple-ios$MIN_OS-simulator" \
            "$@" -o "$SCRATCH/obj/$module/$arch/$object" "$source"
    done
}

mkdir -p "$VENDOR_DIR" "$REMOTE_DIR"

# --- VendorCore: plain Swift static framework with a binary module ---
mkdir -p "$SCRATCH/src/VendorCore"
cat > "$SCRATCH/src/VendorCore/VendorCore.swift" <<'SWIFT'
public func coreValue() -> Int { 3 }
SWIFT
build_swift_framework VendorCore "$VENDOR_DIR" no -enable-library-evolution

# --- VendorUI: Swift importing VendorCore, an old engine copy, and a
#     profile-instrumented Objective-C member that -ObjC will always load ---
mkdir -p "$SCRATCH/src/VendorUI"
cat > "$SCRATCH/src/VendorUI/VendorUI.swift" <<'SWIFT'
import VendorCore

@_silgen_name("engine_old")
func engineOldSymbol() -> Int32

public func render() -> Int { Int(engineOldSymbol()) + coreValue() }
SWIFT
cat > "$SCRATCH/engine_old.c" <<'C'
int engine_common(void) { return 1; }
int engine_old(void) { return engine_common() + 1; }
C
cat > "$SCRATCH/VendorUITelemetry.m" <<'OBJC'
#import <Foundation/Foundation.h>

@interface VendorUITelemetry : NSObject
- (int)beat;
@end

@implementation VendorUITelemetry
- (int)beat {
    return 1;
}
@end
OBJC
compile_c_object VendorUI "$SCRATCH/engine_old.c" engine_old.o
compile_c_object VendorUI "$SCRATCH/VendorUITelemetry.m" telemetry.o -fobjc-arc -fprofile-instr-generate
build_swift_framework VendorUI "$VENDOR_DIR" no \
    -F "$VENDOR_DIR/VendorCore.xcframework/ios-arm64_x86_64-simulator"

# --- VendorEngine: a newer engine copy sharing symbols with VendorUI's ---
mkdir -p "$SCRATCH/src/VendorEngine"
cat > "$SCRATCH/src/VendorEngine/VendorEngine.swift" <<'SWIFT'
@_silgen_name("engine_new")
func engineNewSymbol() -> Int32

public func engineNew() -> Int { Int(engineNewSymbol()) }
SWIFT
cat > "$SCRATCH/engine_new.c" <<'C'
int engine_common(void) { return 5; }
int engine_new(void) { return engine_common() + 2; }
C
compile_c_object VendorEngine "$SCRATCH/engine_new.c" engine_new.o
build_swift_framework VendorEngine "$VENDOR_DIR" no

# --- VendorData: static-library-layout XCFramework (SWIFT_INCLUDE_PATHS) ---
mkdir -p "$SCRATCH/src/VendorData"
cat > "$SCRATCH/src/VendorData/VendorData.swift" <<'SWIFT'
public func dataValue() -> Int { 11 }
SWIFT
build_swift_static_library VendorData "$VENDOR_DIR"

# --- VendorText: interface-only framework whose textual interface imports
#     VendorCore ---
mkdir -p "$SCRATCH/src/VendorText"
cat > "$SCRATCH/src/VendorText/VendorText.swift" <<'SWIFT'
import VendorCore

public func textValue() -> Int { coreValue() + 4 }
SWIFT
build_swift_framework VendorText "$VENDOR_DIR" yes \
    -F "$VENDOR_DIR/VendorCore.xcframework/ios-arm64_x86_64-simulator"

# --- VendorRemote: referenced by an absolute out-of-workspace path ---
mkdir -p "$SCRATCH/src/VendorRemote"
cat > "$SCRATCH/src/VendorRemote/VendorRemote.swift" <<'SWIFT'
public func remoteValue() -> Int { 2 }
SWIFT
build_swift_framework VendorRemote "$REMOTE_DIR" no
