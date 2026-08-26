# Generic host primitives provided by Rust:
#   host_arch()                -> "arm64" | "x86_64" | ...
#   host_os()                  -> "macos" | "linux" | ...
#   host_which(name)           -> absolute path to a binary on PATH, or fails
#   host_command(argv)         -> stdout string; fails on non-zero exit
#   glob(patterns)             -> sorted, deduplicated workspace-relative file paths
#                                 matching the patterns under the active package
#   declare_output(name)       -> workspace-relative output path under the active build_dir
#   run_action(argv=..., inputs=..., outputs=..., env={}, cacheable=True, toolchain_identity=None, identifier=None)
#
# Each impl receives a `ctx` dict built by the Rust analysis pass with:
#   ctx["label"]      -> {"package", "name", "id"}
#   ctx["attr"]       -> typed attribute dict
#   ctx["srcs"]       -> raw glob patterns declared on the target (impl calls glob() to expand)
#   ctx["deps"]       -> list of provider records returned by analyzed deps
#   ctx["build_dir"]  -> workspace-relative output directory for this target
#   ctx["capability"] -> active capability requested by the executor ("build", "test", "metadata")
#
# The impl returns a provider dict. Conventional keys downstream target kinds read:
#   "swiftmodule_dir" -> directory holding the .swiftmodule (added to -I by consumers)
#   "archive"         -> workspace-relative path to the .a archive

# Apple-specific helpers implemented in starlark on top of the generic
# primitives. Everything platform-specific (SDK names, triple format,
# xcrun resolution, file-extension filtering) lives here, not in Rust.

def _apple_sdk_name(platform, sdk_variant):
    # macOS doesn't ship a simulator SDK; the variant is ignored.
    if platform == "macos" or platform == "macosx":
        return "macosx"
    # For every other platform pick the device SDK or the simulator
    # SDK based on `sdk_variant`. Defaulting to simulator preserves
    # the previous behavior for manifests that don't set it.
    if platform == "ios":
        if sdk_variant == "device":
            return "iphoneos"
        return "iphonesimulator"
    if platform == "tvos":
        if sdk_variant == "device":
            return "appletvos"
        return "appletvsimulator"
    if platform == "watchos":
        if sdk_variant == "device":
            return "watchos"
        return "watchsimulator"
    if platform == "visionos" or platform == "xros":
        if sdk_variant == "device":
            return "xros"
        return "xrsimulator"
    fail("unsupported apple platform `" + platform + "`")

def _apple_triple_os(platform):
    if platform == "macos" or platform == "macosx":
        return "macosx"
    if platform == "ios":
        return "ios"
    if platform == "tvos":
        return "tvos"
    if platform == "watchos":
        return "watchos"
    if platform == "visionos" or platform == "xros":
        return "xros"
    return platform

def _apple_triple_suffix(platform, sdk_variant):
    # macOS has no simulator. Device variants on other platforms render
    # an empty suffix; simulators keep the `-simulator` tag swiftc
    # expects.
    if platform == "macos" or platform == "macosx":
        return ""
    if sdk_variant == "device":
        return ""
    return "-simulator"

def _apple_triple(platform, minimum_os, sdk_variant, arch, mac_catalyst):
    # Mac Catalyst surfaces as `<arch>-apple-ios<minOS>-macabi` no
    # matter which platform the manifest set; the iOS triple is what
    # swiftc and clang expect for the iOSMac variant of macOS.
    if mac_catalyst:
        return arch + "-apple-ios" + minimum_os + "-macabi"
    triple_os = _apple_triple_os(platform)
    suffix = _apple_triple_suffix(platform, sdk_variant)
    return arch + "-apple-" + triple_os + minimum_os + suffix

def _apple_swiftmodule_triple(platform, sdk_variant, arch, mac_catalyst):
    return _apple_triple(platform, "", sdk_variant, arch, mac_catalyst)

def _developer_env(xcode_developer_dir):
    env = {}
    if xcode_developer_dir:
        env["DEVELOPER_DIR"] = xcode_developer_dir
    return env

# When a target sets `xcode_developer_dir`, the build resolves tools and
# SDK paths directly from the layout under that directory rather than
# shelling out to `xcrun`. The xcrun fallback still applies when no
# developer dir is configured.
_XCTOOLCHAIN_BIN_REL = "Toolchains/XcodeDefault.xctoolchain/usr/bin"

_SDK_PATH_REL = {
    "macosx": "Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk",
    "iphoneos": "Platforms/iPhoneOS.platform/Developer/SDKs/iPhoneOS.sdk",
    "iphonesimulator": "Platforms/iPhoneSimulator.platform/Developer/SDKs/iPhoneSimulator.sdk",
    "appletvos": "Platforms/AppleTVOS.platform/Developer/SDKs/AppleTVOS.sdk",
    "appletvsimulator": "Platforms/AppleTVSimulator.platform/Developer/SDKs/AppleTVSimulator.sdk",
    "watchos": "Platforms/WatchOS.platform/Developer/SDKs/WatchOS.sdk",
    "watchsimulator": "Platforms/WatchSimulator.platform/Developer/SDKs/WatchSimulator.sdk",
    "xros": "Platforms/XROS.platform/Developer/SDKs/XROS.sdk",
    "xrsimulator": "Platforms/XRSimulator.platform/Developer/SDKs/XRSimulator.sdk",
}

_PLATFORM_PATH_REL = {
    "macosx": "Platforms/MacOSX.platform",
    "iphoneos": "Platforms/iPhoneOS.platform",
    "iphonesimulator": "Platforms/iPhoneSimulator.platform",
    "appletvos": "Platforms/AppleTVOS.platform",
    "appletvsimulator": "Platforms/AppleTVSimulator.platform",
    "watchos": "Platforms/WatchOS.platform",
    "watchsimulator": "Platforms/WatchSimulator.platform",
    "xros": "Platforms/XROS.platform",
    "xrsimulator": "Platforms/XRSimulator.platform",
}

def _developer_sdk_path(xcode_developer_dir, sdk_name):
    rel = _SDK_PATH_REL.get(sdk_name)
    if not rel:
        fail("no SDK layout entry for `" + sdk_name + "`; add it to _SDK_PATH_REL")
    return xcode_developer_dir + "/" + rel

def _developer_platform_path(xcode_developer_dir, sdk_name):
    rel = _PLATFORM_PATH_REL.get(sdk_name)
    if not rel:
        fail("no platform layout entry for `" + sdk_name + "`; add it to _PLATFORM_PATH_REL")
    return xcode_developer_dir + "/" + rel

def _xctoolchain_bin(xcode_developer_dir, name):
    return xcode_developer_dir + "/" + _XCTOOLCHAIN_BIN_REL + "/" + name

# Resolves a build tool and the active SDK path to absolute file paths.
# Returns a dict so the action helper can invoke the tool directly
# (`[tool_path, ...flags]`) instead of going through `xcrun --sdk X
# <tool>`. Direct mode skips `xcrun` entirely; the fallback still uses
# `xcrun --find` / `--show-sdk-path` for discovery but the cached
# action argv contains only the resolved tool path.
def _resolve_swiftc(platform, sdk_variant, xcode_developer_dir):
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _developer_env(xcode_developer_dir)
    if xcode_developer_dir:
        swiftc_path = _xctoolchain_bin(xcode_developer_dir, "swiftc")
        sdk_path = _developer_sdk_path(xcode_developer_dir, sdk)
    else:
        xcrun = host_which("xcrun")
        swiftc_path = host_command([xcrun, "--sdk", sdk, "--find", "swiftc"], env = env).strip()
        sdk_path = host_command([xcrun, "--sdk", sdk, "--show-sdk-path"], env = env).strip()
    version = host_command([swiftc_path, "--version"], env = env).strip()
    # Identity folds in the developer dir override so different Xcode
    # installations partition the action cache cleanly.
    identity = "once.apple.swiftc.v1\x00" + swiftc_path + "\x00" + version + "\x00" + (xcode_developer_dir or "")
    return {
        "argv": [swiftc_path, "-sdk", sdk_path],
        "swiftc_path": swiftc_path,
        "sdk_name": sdk,
        "sdk_path": sdk_path,
        "version": version,
        "identity": identity,
        "env": env,
    }

def _filter_swift_sources(paths):
    return _filter_by_extensions(paths, [".swift"])

def _filter_objc_sources(paths):
    return _filter_by_extensions(paths, [".m", ".mm"])

def _filter_c_sources(paths):
    return _filter_by_extensions(paths, [".c"])

def _filter_cxx_sources(paths):
    return _filter_by_extensions(paths, [".cc", ".cpp", ".cxx"])

def _filter_assembly_sources(paths):
    return _filter_by_extensions(paths, [".s", ".S"])

def _resolve_clang(platform, sdk_variant, xcode_developer_dir):
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _developer_env(xcode_developer_dir)
    if xcode_developer_dir:
        clang_path = _xctoolchain_bin(xcode_developer_dir, "clang")
        clangxx_path = _xctoolchain_bin(xcode_developer_dir, "clang++")
        sdk_path = _developer_sdk_path(xcode_developer_dir, sdk)
    else:
        xcrun = host_which("xcrun")
        clang_path = host_command([xcrun, "--sdk", sdk, "--find", "clang"], env = env).strip()
        clangxx_path = host_command([xcrun, "--sdk", sdk, "--find", "clang++"], env = env).strip()
        sdk_path = host_command([xcrun, "--sdk", sdk, "--show-sdk-path"], env = env).strip()
    version = host_command([clang_path, "--version"], env = env).strip()
    identity = "once.apple.clang.v1\x00" + clang_path + "\x00" + version + "\x00" + (xcode_developer_dir or "")
    return {
        "clang_path": clang_path,
        "clangxx_path": clangxx_path,
        "sdk_name": sdk,
        "sdk_path": sdk_path,
        "identity": identity,
        "env": env,
    }

def _resolve_libtool(platform, sdk_variant, xcode_developer_dir):
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _developer_env(xcode_developer_dir)
    if xcode_developer_dir:
        libtool_path = _xctoolchain_bin(xcode_developer_dir, "libtool")
    else:
        xcrun = host_which("xcrun")
        libtool_path = host_command([xcrun, "--sdk", sdk, "--find", "libtool"], env = env).strip()
    identity = "once.apple.libtool.v1\x00" + libtool_path + "\x00" + (xcode_developer_dir or "")
    return {
        "argv": [libtool_path],
        "libtool_path": libtool_path,
        "identity": identity,
        "env": env,
    }

def _resolve_lipo(platform, sdk_variant, xcode_developer_dir):
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _developer_env(xcode_developer_dir)
    if xcode_developer_dir:
        lipo_path = _xctoolchain_bin(xcode_developer_dir, "lipo")
    else:
        xcrun = host_which("xcrun")
        lipo_path = host_command([xcrun, "--sdk", sdk, "--find", "lipo"], env = env).strip()
    identity = "once.apple.lipo.v1\x00" + lipo_path + "\x00" + (xcode_developer_dir or "")
    return {
        "argv": [lipo_path],
        "lipo_path": lipo_path,
        "identity": identity,
        "env": env,
    }

def _resolve_codesign(xcode_developer_dir):
    env = _developer_env(xcode_developer_dir)
    # codesign is a system tool, not a toolchain binary under
    # DEVELOPER_DIR. Resolve it through xcrun so signing does not
    # depend on a shell search path replacement.
    xcrun = host_which("xcrun")
    codesign_path = host_command([xcrun, "--find", "codesign"], env = env).strip()
    identity = "once.apple.codesign.v1\x00" + codesign_path + "\x00" + (xcode_developer_dir or "")
    return {
        "codesign_path": codesign_path,
        "identity": identity,
        "env": env,
    }

def _resolve_derq(xcode_developer_dir):
    env = _developer_env(xcode_developer_dir)
    xcrun = host_which("xcrun")
    path = host_command([xcrun, "--find", "derq"], env = env).strip()
    return {
        "path": path,
        "identity": "once.apple.derq.v1\x00" + path + "\x00" + (xcode_developer_dir or ""),
        "env": env,
    }

def _resolve_actool(xcode_developer_dir):
    env = _developer_env(xcode_developer_dir)
    xcrun = host_which("xcrun")
    actool_path = host_command([xcrun, "--find", "actool"], env = env).strip()
    identity = "once.apple.actool.v1\x00" + actool_path + "\x00" + (xcode_developer_dir or "")
    return {
        "actool_path": actool_path,
        "identity": identity,
        "env": env,
    }

def _resolve_momc(xcode_developer_dir):
    env = _developer_env(xcode_developer_dir)
    xcrun = host_which("xcrun")
    path = host_command([xcrun, "--find", "momc"], env = env).strip()
    return {"path": path, "env": env, "identity": "once.apple.momc.v1\x00" + path + "\x00" + (xcode_developer_dir or "")}

def _resolve_ibtool(xcode_developer_dir):
    env = _developer_env(xcode_developer_dir)
    if xcode_developer_dir:
        path = xcode_developer_dir + "/usr/bin/ibtool"
    else:
        path = host_command([host_which("xcrun"), "--find", "ibtool"], env = env).strip()
    version = host_command([path, "--version"], env = env).strip()
    return {"path": path, "env": env, "identity": "once.apple.ibtool.v1\x00" + path + "\x00" + version + "\x00" + (xcode_developer_dir or "")}

def _resolve_intentbuilderc(xcode_developer_dir):
    env = _developer_env(xcode_developer_dir)
    xcrun = host_which("xcrun")
    path = host_command([xcrun, "--find", "intentbuilderc"], env = env).strip()
    return {"path": path, "env": env, "identity": "once.apple.intentbuilderc.v1\x00" + path + "\x00" + (xcode_developer_dir or "")}

def _apple_actool_platform(platform, sdk_variant):
    device = sdk_variant == "device"
    if platform == "macos" or platform == "macosx":
        return "macosx"
    if platform == "tvos":
        return "appletvos" if device else "appletvsimulator"
    if platform == "watchos":
        return "watchos" if device else "watchsimulator"
    if platform == "visionos":
        return "xros" if device else "xrsimulator"
    return "iphoneos" if device else "iphonesimulator"

def _resolve_apple_thinning_tools(xcode_developer_dir):
    env = _developer_env(xcode_developer_dir)
    xcrun = host_which("xcrun")
    if xcode_developer_dir:
        developer_dir = xcode_developer_dir
        ipatool = developer_dir + "/usr/bin/ipatool"
        xcodebuild = developer_dir + "/usr/bin/xcodebuild"
    else:
        ipatool = host_command([xcrun, "--find", "ipatool"], env = env).strip()
        suffix = "/usr/bin/ipatool"
        if not _ends_with(ipatool, suffix):
            fail("unable to derive the Xcode developer directory from ipatool at " + ipatool)
        developer_dir = ipatool[:len(ipatool) - len(suffix)]
        xcodebuild = host_command([xcrun, "--find", "xcodebuild"], env = env).strip()
    ruby = host_which("ruby")
    zip = host_which("zip")
    codesign = _resolve_codesign(developer_dir)
    xcode_version = host_command([xcodebuild, "-version"], env = env).strip()
    ruby_version = host_command([ruby, "--version"]).strip()
    zip_version = host_command([zip, "-v"]).strip()
    action_env = dict(env)
    action_path = developer_dir + "/Toolchains/XcodeDefault.xctoolchain/usr/bin:/usr/bin:/bin"
    action_env["PATH"] = action_path
    action_env["LANG"] = "C"
    action_env["LC_ALL"] = "C"
    action_env["TZ"] = "UTC"
    identity = "\x00".join([
        "once.apple.thinning.v1",
        developer_dir,
        ipatool,
        xcode_version,
        ruby,
        ruby_version,
        zip,
        zip_version,
        codesign["codesign_path"],
        action_path,
    ])
    return {
        "ruby": ruby,
        "ipatool": ipatool,
        "zip": zip,
        "codesign": codesign["codesign_path"],
        "toolchain_dir": developer_dir + "/Toolchains/XcodeDefault.xctoolchain/usr",
        "platforms_dir": developer_dir + "/Platforms",
        "identity": identity,
        "env": action_env,
    }

def _swift_testing_macros_plugin(swiftc_path):
    suffix = "/usr/bin/swiftc"
    if not _ends_with(swiftc_path, suffix):
        fail("unable to derive Swift toolchain path from swiftc at " + swiftc_path)
    toolchain_dir = swiftc_path[:len(swiftc_path) - len(suffix)]
    return toolchain_dir + "/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib"

def _unique_dirs(paths):
    seen = {}
    out = []
    for path in paths:
        directory = _parent_dir(path)
        if directory and directory not in seen:
            seen[directory] = True
            out.append(directory)
    return out

# --- Apple header map (.hmap) byte construction ---------------------
#
# Clang reads `.hmap` files via `-I <foo.hmap>`. The format is defined
# in LLVM's `clang/include/clang/Lex/HeaderMapTypes.h`. Layout (all
# little-endian on the platforms Once targets):
#
#   offset  size   field
#   ------  ----   -----
#     0      4     magic         = 0x686D6170
#     4      2     version       = 1
#     6      2     reserved      = 0
#     8      4     strings_off
#    12      4     num_entries
#    16      4     num_buckets   (power of two)
#    20      4     max_value_len
#    24    12*N    buckets[N]    each { key_off, prefix_off, suffix_off }
#    ...     ?     string table  starts with a single 0 byte
#
# A bucket whose `key_off` is 0 is empty. Each `(key, value)` pair is
# stored with `value` in the bucket's `prefix_off` and an empty suffix;
# clang resolves a lookup by concatenating prefix + suffix, so an
# empty suffix means the value reads back verbatim. Keys hash
# case-insensitively (sum of lowercase byte * 13) and collisions are
# resolved by linear probing.

def _u32_le(value):
    return [
        value & 0xFF,
        (value >> 8) & 0xFF,
        (value >> 16) & 0xFF,
        (value >> 24) & 0xFF,
    ]

def _u16_le(value):
    return [
        value & 0xFF,
        (value >> 8) & 0xFF,
    ]

def _hmap_hash(key):
    lowered = key.lower()
    result = 0
    for ch in lowered.elems():
        result = (result + ord(ch) * 13) & 0xFFFFFFFF
    return result

def _next_power_of_two(value):
    size = 1
    for _ in range(64):
        if size >= value:
            return size
        size = size * 2
    fail("hmap bucket count overflowed 2^64")

def _serialize_hmap(entries):
    # Build the string table. Offset 0 holds a single 0 byte so that
    # bucket slot 0 unambiguously means "empty".
    strings = [0]
    offset_for = {}

    def intern(string):
        if string in offset_for:
            return offset_for[string]
        offset = len(strings)
        for ch in string.elems():
            strings.append(ord(ch))
        strings.append(0)
        offset_for[string] = offset
        return offset

    entry_count = len(entries)
    raw_capacity = entry_count * 2
    if raw_capacity < 1:
        raw_capacity = 1
    num_buckets = _next_power_of_two(raw_capacity)

    buckets = []
    for _ in range(num_buckets):
        buckets.append((0, 0, 0))
    max_value_len = 0

    for key, value in entries.items():
        key_off = intern(key)
        prefix_off = intern(value)
        suffix_off = intern("")
        if len(value) > max_value_len:
            max_value_len = len(value)
        idx = _hmap_hash(key) & (num_buckets - 1)
        placed = False
        for _ in range(num_buckets):
            if buckets[idx][0] == 0:
                buckets[idx] = (key_off, prefix_off, suffix_off)
                placed = True
                break
            idx = (idx + 1) & (num_buckets - 1)
        if not placed:
            fail("hmap bucket array filled unexpectedly")

    HEADER_SIZE = 24
    BUCKET_SIZE = 12
    strings_off = HEADER_SIZE + num_buckets * BUCKET_SIZE

    out = []
    out.extend(_u32_le(0x686D6170))
    out.extend(_u16_le(1))
    out.extend(_u16_le(0))
    out.extend(_u32_le(strings_off))
    out.extend(_u32_le(entry_count))
    out.extend(_u32_le(num_buckets))
    out.extend(_u32_le(max_value_len))
    for bucket in buckets:
        out.extend(_u32_le(bucket[0]))
        out.extend(_u32_le(bucket[1]))
        out.extend(_u32_le(bucket[2]))
    out.extend(strings)
    return out

def _write_hmap(path, entries):
    write_path(path, _serialize_hmap(entries))

# Normalise a dep reference written in `[target.attrs]` (`./AppCore`,
# `../web/Common`, or a root-relative `apps/ios/AppCore`) to the
# absolute target id Once stores in `dep["label_id"]`. This keeps
# `exported_deps` membership checks correct even when the manifest
# author uses any of the three reference styles.
def _resolve_dep_ref(ref, package):
    if ref.startswith("./"):
        rest = ref[2:]
        if package:
            return package + "/" + rest
        return rest
    if ref.startswith("../"):
        slash = -1
        for i in range(len(package)):
            if package[i] == "/":
                slash = i
        if slash < 0:
            # `../` from a top-level package resolves at the workspace
            # root; drop the segment and keep walking.
            return _resolve_dep_ref(ref[3:], "")
        return _resolve_dep_ref(ref[3:], package[:slash])
    # Root-relative reference. Once normalises top-level `deps` to this
    # shape; the same convention applies here.
    return ref

def _apple_framework_bundle(path, module_name, files, label_id, absorbed_static_archives = []):
    return {
        "path": path,
        "module_name": module_name,
        "files": files,
        "label_id": label_id,
        "absorbed_static_archives": absorbed_static_archives,
    }

def _apple_framework_compile_files(bundle):
    path = bundle.get("path") or ""
    module_name = bundle.get("module_name") or ""
    binary = path + "/" + module_name if path and module_name else ""
    out = []
    for file in bundle.get("files") or []:
        is_interface = "/Headers/" in file or "/Modules/" in file
        is_module_file = file.endswith(".modulemap") or file.endswith(".swiftmodule") or file.endswith(".swiftinterface") or file.endswith(".swiftdoc")
        if file == binary or is_interface or is_module_file:
            out.append(file)
    return _unique(out)

def _apple_header_inputs(ctx, header_dirs):
    extensions = ["h", "hh", "hpp", "hxx", "inc", "inl", "ipp", "tpp", "def"]
    inputs = []
    for header_dir in header_dirs:
        if not header_dir or header_dir.startswith("/") or header_dir == ".once" or header_dir.startswith(".once/"):
            continue
        absolute_header_dir = workspace_root() + "/" + header_dir
        if not host_path_exists(absolute_header_dir):
            continue
        if not host_path_is_within(absolute_header_dir, workspace_root()):
            snapshot = declare_output("external-header-inputs/tree/" + header_dir)
            materialize_host_tree(absolute_header_dir, snapshot)
            inputs.append(snapshot)
            continue
        for candidate in walk_workspace_files(header_dir):
            is_header = False
            for extension in extensions:
                if candidate.endswith("." + extension):
                    is_header = True
                    break
            if not is_header:
                continue
            absolute_candidate = workspace_root() + "/" + candidate
            if host_path_is_within(absolute_candidate, workspace_root()):
                inputs.append(candidate)
            elif host_file_exists(absolute_candidate):
                snapshot = declare_output("external-header-inputs/file/" + candidate)
                materialize_host_file(absolute_candidate, snapshot)
                inputs.append(snapshot)
    return _unique(inputs)

def _apple_legacy_framework_bundles(dep, include_transitive):
    direct_path = dep.get("framework_path") or ""
    paths = dep.get("transitive_frameworks") if include_transitive else None
    if paths == None:
        paths = [direct_path] if direct_path else []
    out = []
    for path in paths:
        module_name = ""
        files = []
        absorbed_static_archives = []
        if path == direct_path:
            module_name = dep.get("framework_module_name") or ""
            files = dep.get("framework_files") or []
            absorbed_static_archives = dep.get("absorbed_static_archives") or []
        out.append(_apple_framework_bundle(
            path,
            module_name,
            files,
            dep.get("label_id") or "",
            absorbed_static_archives,
        ))
    return out

def _apple_dep_framework_bundles(dep, key, include_transitive):
    bundles = dep.get(key)
    if bundles != None:
        return bundles
    return _apple_legacy_framework_bundles(dep, include_transitive)

def _apple_collect_framework_bundles(deps, key, own_bundles, include_transitive):
    seen = {}
    out = []
    for bundle in own_bundles:
        path = bundle.get("path") or ""
        if path and path not in seen:
            seen[path] = True
            out.append(bundle)
    for dep in deps:
        for bundle in _apple_dep_framework_bundles(dep, key, include_transitive):
            path = bundle.get("path") or ""
            if path and path not in seen:
                seen[path] = True
                out.append(bundle)
    return out

def _apple_collect_link_framework_bundles(deps, own_bundles = []):
    return _apple_collect_framework_bundles(deps, "transitive_link_framework_bundles", own_bundles, False)

def _apple_collect_runtime_framework_bundles(deps, own_bundles = []):
    return _apple_collect_framework_bundles(deps, "transitive_framework_bundles", own_bundles, True)

def _apple_disable_static_framework_autolinking(argv, framework_bundles):
    for bundle in framework_bundles:
        module_name = bundle.get("module_name") or ""
        if bundle.get("linkage") == "static" and module_name:
            argv.extend(["-Xfrontend", "-disable-autolink-framework", "-Xfrontend", module_name])

def _apple_resource_bundle(path, files, label_id):
    return {
        "path": path,
        "files": files,
        "label_id": label_id,
    }

def _apple_collect_resource_bundles(deps, own_bundles = []):
    seen = {}
    out = []
    for bundle in own_bundles:
        path = bundle.get("path") or ""
        if path and path not in seen:
            seen[path] = True
            out.append(bundle)
    for dep in deps:
        for bundle in dep.get("transitive_resource_bundles") or []:
            path = bundle.get("path") or ""
            if path and path not in seen:
                seen[path] = True
                out.append(bundle)
    return out

def _apple_framework_bundle_paths(bundles):
    return [bundle["path"] for bundle in bundles]

def _apple_collect_alwayslink_archives(deps):
    archives = []
    for dep in deps:
        for archive in dep.get("transitive_alwayslink_archives") or []:
            if archive and archive not in archives:
                archives.append(archive)
    return archives

def _apple_append_archives(argv, archives, alwayslink_archives):
    for archive in archives:
        if archive in alwayslink_archives:
            argv.extend(["-Xlinker", "-force_load", "-Xlinker", archive])
        else:
            argv.append(archive)

def _apple_append_weak_framework(argv, framework):
    argv.extend(["-Xlinker", "-weak_framework", "-Xlinker", framework])

_APPLE_LINK_OPTION_ARITY = {
    "-F": 1,
    "-Fsystem": 1,
    "-L": 1,
    "-Xcc": 1,
    "-Xclang": 1,
    "-framework": 1,
    "-l": 1,
    "-weak_framework": 1,
}

_APPLE_FORWARDED_LINK_OPTION_ARITY = {
    "-alias": 2,
    "-bundle_loader": 1,
    "-compatibility_version": 1,
    "-current_version": 1,
    "-e": 1,
    "-exported_symbols_list": 1,
    "-filelist": 1,
    "-force_load": 1,
    "-framework": 1,
    "-install_name": 1,
    "-order_file": 1,
    "-rpath": 1,
    "-sectcreate": 3,
    "-segprot": 3,
    "-u": 1,
    "-undefined": 1,
    "-unexported_symbols_list": 1,
    "-weak_framework": 1,
}

def _apple_unique_linkopts(values):
    return _unique_args(
        values,
        option_arity = _APPLE_LINK_OPTION_ARITY,
        forwarder = "-Xlinker",
        forwarded_option_arity = _APPLE_FORWARDED_LINK_OPTION_ARITY,
    )

def _apple_collect_transitive_linkopts(deps, own_values):
    values = list(own_values)
    for dep in deps:
        values.extend(dep.get("transitive_linkopts") or [])
    return _apple_unique_linkopts(values)

def _validate_apple_native_deps(deps, consumer_label):
    for dep in deps:
        if dep.get("target_kind") != "rust_library":
            continue
        crate_type = dep.get("crate_type") or ""
        if crate_type == "staticlib":
            continue
        label = dep.get("label_id") or "dependency"
        fail(consumer_label + ": Rust library dep `" + label + "` has crate_type `" + crate_type + "` and does not provide an Apple static library; set crate_type = \"staticlib\" for Apple consumers")

def _apple_native_deps(ctx):
    out = []
    materialized = {}
    for dep in ctx["deps"]:
        out.append(_apple_materialize_native_dep(ctx, dep, materialized))
    return out

# A select-shape attribute value is a dict with exactly one `select`
# key whose value is itself a dict from configuration tokens to
# branches:
#
#   defines = { select = { ios = ["FOO"], default = [] } }
#
# `_is_select_shape` detects that shape so the resolver can fan out
# without conflating it with regular `dict` attribute values.

def _is_select_shape(value):
    if type(value) != "dict":
        return False
    if len(value) != 1:
        return False
    inner = value.get("select")
    if inner == None:
        return False
    return type(inner) == "dict"

# Active configuration tokens for an Apple target. Selects match
# against these tokens: `platform` ("ios", "macos", ...), `sdk_variant`
# ("simulator", "device"), each entry of `archs` ("arm64", "x86_64",
# ...), and the literal token `mac_catalyst` when the attribute is on.
#
# The four input attributes themselves cannot be selects (there is no
# way to resolve a select on `platform` because the resolver needs
# `platform` to decide). `_apple_config_tokens` fails loudly if any of
# them is a select-shape dict instead of resolving to a misleading
# empty token list.

def _apple_config_tokens(ctx, attrs, label_id):
    for input_key in ["platform", "sdk_variant", "archs", "mac_catalyst"]:
        if _is_select_shape(attrs.get(input_key)):
            fail(label_id + ": attribute `" + input_key + "` cannot use select() because the configuration depends on it")

    tokens = []
    platform = attrs.get("platform")
    if platform and type(platform) == "string":
        tokens.append(platform)
    sdk_variant = attrs.get("sdk_variant")
    if sdk_variant and type(sdk_variant) == "string":
        tokens.append(sdk_variant)
    archs = attrs.get("archs")
    if archs == None or (type(archs) == "list" and len(archs) == 0):
        archs = [host_arch()]
    if type(archs) == "list":
        for arch in archs:
            if type(arch) == "string" and arch not in tokens:
                tokens.append(arch)
    if attrs.get("mac_catalyst"):
        tokens.append("mac_catalyst")
    return _configuration_tokens(ctx, tokens)

def _select_branch_for_tokens(branches, tokens, label_id, attr_name):
    matching = []
    for key in branches.keys():
        if key == "default":
            continue
        match = True
        for part in key.split(":"):
            if part not in tokens:
                match = False
                break
        if match:
            matching.append(key)
    if len(matching) == 0:
        if "default" in branches:
            return "default"
        fail(label_id + ": select() on `" + attr_name + "` has no branch matching the configuration and no `default` (branches: " + str(branches.keys()) + ")")
    if len(matching) == 1:
        return matching[0]
    # Prefer the most specific (longest) key when several match. This
    # lets `ios:simulator` beat a bare `ios` branch when both are
    # eligible.
    longest = matching[0]
    for key in matching:
        if len(key) > len(longest):
            longest = key
    return longest

def _resolve_select(value, tokens, label_id, attr_name):
    if _is_select_shape(value):
        branches = value["select"]
        key = _select_branch_for_tokens(branches, tokens, label_id, attr_name)
        return _resolve_select(branches[key], tokens, label_id, attr_name)
    if type(value) == "list":
        return [_resolve_select(item, tokens, label_id, attr_name) for item in value]
    if type(value) == "dict":
        return {k: _resolve_select(v, tokens, label_id, attr_name) for k, v in value.items()}
    return value

def _resolve_attrs(ctx, attrs, label_id, non_configurable):
    tokens = _apple_config_tokens(ctx, attrs, label_id)
    out = {}
    for key, value in attrs.items():
        if key in non_configurable and _is_select_shape(value):
            fail(label_id + ": attribute `" + key + "` is not configurable but uses select()")
        out[key] = _resolve_select(value, tokens, label_id, key)
    return out

def _attr_has_value(value):
    if value == None:
        return False
    if type(value) == "string" and value == "":
        return False
    if type(value) == "list" and len(value) == 0:
        return False
    if type(value) == "dict" and len(value) == 0:
        return False
    return True

def _reject_unsupported_attrs(attrs, label_id, keys):
    for key in keys:
        if key in attrs and _attr_has_value(attrs.get(key)):
            fail(label_id + ": attribute `" + key + "` is declared but not implemented by this target kind yet")

def _select_mentions_any(branches, tokens):
    for key in branches.keys():
        for part in key.split(":"):
            if part in tokens:
                return True
    return False

def _reject_multi_arch_selects(attrs, label_id, archs):
    if len(archs) <= 1:
        return
    arch_tokens = {}
    for arch in archs:
        arch_tokens[arch] = True
    for key, value in attrs.items():
        if _is_select_shape(value) and _select_mentions_any(value["select"], arch_tokens):
            fail(label_id + ": attribute `" + key + "` cannot select on architecture when `archs` contains multiple values")

def _shell_literal(value):
    return "'" + value.replace("'", "'\"'\"'") + "'"

def _shell_words(values):
    out = []
    for value in values:
        out.append(_shell_literal(value))
    return " ".join(out)

def _json_escape(value):
    return value.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")

def _json_literal(value):
    return "\"" + _json_escape(value) + "\""

_IOS_SIMULATOR_BOOTED_FILTER = "/iPhone/ s/^.* (\\([0-9A-Fa-f-][0-9A-Fa-f-]*\\)) (Booted)[[:space:]]*$/\\1/p; /iPad/ s/^.* (\\([0-9A-Fa-f-][0-9A-Fa-f-]*\\)) (Booted)[[:space:]]*$/\\1/p"
_IOS_SIMULATOR_SHUTDOWN_FILTER = "/iPhone/ s/^.* (\\([0-9A-Fa-f-][0-9A-Fa-f-]*\\)) (Shutdown)[[:space:]]*$/\\1/p; /iPad/ s/^.* (\\([0-9A-Fa-f-][0-9A-Fa-f-]*\\)) (Shutdown)[[:space:]]*$/\\1/p"

def _ios_simulator_selection_script(xcrun):
    return """simulator_id="${{ONCE_APPLE_SIMULATOR_UDID:-}}"
if [ -z "$simulator_id" ]; then
  simulator_id=$({xcrun} simctl list devices booted | sed -n {booted_filter} | head -n 1)
fi
if [ -z "$simulator_id" ]; then
  simulator_id=$({xcrun} simctl list devices available | sed -n {shutdown_filter} | head -n 1)
fi
if [ -z "$simulator_id" ]; then
  echo "error: no booted or available iOS simulator found" >&2
  exit 1
fi
""".format(
        xcrun = _shell_literal(xcrun),
        booted_filter = _shell_literal(_IOS_SIMULATOR_BOOTED_FILTER),
        shutdown_filter = _shell_literal(_IOS_SIMULATOR_SHUTDOWN_FILTER),
    )

def _apple_ui_test_install_script(xcrun, bundle_id, app_path):
    return """{xcrun} simctl terminate "$simulator_id" {bundle_id} >/dev/null 2>&1 || true
{xcrun} simctl uninstall "$simulator_id" {bundle_id} >/dev/null 2>&1 || true
{xcrun} simctl install "$simulator_id" {app_path}
""".format(
        xcrun = _shell_literal(xcrun),
        bundle_id = _shell_literal(bundle_id),
        app_path = _shell_literal(app_path),
    )

def _shellspec_test_impl(ctx):
    attrs = ctx["attr"]
    shellspec = attrs.get("shellspec") or "shellspec"
    args = attrs.get("args") or []
    env = attrs.get("env") or {}
    data = attrs.get("data") or []
    labels = attrs.get("labels") or []
    timeout_ms = attrs.get("timeout_ms")
    srcs = glob(ctx["srcs"])
    inputs = []
    for src in srcs:
        inputs.append(src)
    for path in data:
        inputs.append(_package_relative(ctx, path))

    test_dir = ctx["build_dir"] + "/test"
    results = test_dir + "/test_results.json"
    log = test_dir + "/shellspec.log"
    native_results = test_dir + "/native_results.txt"
    action_env = {"HOME": test_dir + "/home"}
    for key in env:
        action_env[key] = env[key]
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "shellspec_test",
        "affected_inputs": inputs,
        "test_info": {
            "schema": "once.test_info.v1",
            "target": ctx["label"]["id"],
            "runner": {
                "type": "shellspec",
                "display_name": "ShellSpec",
                "metadata": {},
            },
            "command": {
                "argv": [shellspec] + args,
                "env": action_env,
                "cwd": ".",
            },
            "outputs": {
                "results": results,
                "logs": [log],
                "native_results": [native_results],
                "coverage": [],
            },
            "listing": {
                "supported": True,
                "strategy": "parse_shellspec_examples",
            },
            "filtering": {
                "case_filtering": "unsupported",
            },
            "sharding": {
                "supported": False,
            },
            "retries": {
                "supported": False,
                "default_attempts": 1,
            },
            "execution": {
                "cacheable": True,
                "timeout_ms": timeout_ms,
                "run_from_workspace_root": True,
            },
            "labels": labels,
            "metadata": {},
        },
    }
    if ctx["capability"] != "test":
        return provider

    shellspec_exec = shellspec
    if "/" not in shellspec:
        shellspec_exec = host_which(shellspec)

    spec_srcs = [src for src in srcs if src.endswith("_spec.sh")]
    runner_args = [shellspec_exec] + args + spec_srcs

    script = """set -eu
mkdir -p "$HOME"
log={log}
results={results}
native_results={native_results}
: > "$native_results"
set +e
{command} > "$log" 2>&1
status=$?
set -e
cp "$log" "$native_results"
total=0
cases_file="{test_dir}/cases.jsonl"
: > "$cases_file"
for spec in {specs}; do
  [ -f "$spec" ] || continue
  suite=${{spec#spec/}}
  suite=${{suite%_spec.sh}}
  while IFS= read -r line; do
    case "$line" in
      *"It '"*)
        name=${{line#*"It '"}}
        name=${{name%%"'"*}}
        total=$((total + 1))
        case_id="$spec::$name"
        if [ "$status" -eq 0 ]; then case_status=passed; else case_status=unknown; fi
        if [ "$total" -gt 1 ]; then printf ',\n' >> "$cases_file"; fi
        printf '{{"id":"%s","name":"%s","suite":"%s","file":"%s","status":"%s","attempts":[{{"status":"%s"}}],"runner_metadata":{{}}}}' "$case_id" "$name" "$suite" "$spec" "$case_status" "$case_status" >> "$cases_file"
        ;;
    esac
  done < "$spec"
done
if [ "$status" -eq 0 ]; then run_status=passed; failed=0; passed=$total; else run_status=failed; failed=1; passed=0; fi
{{
  printf '{{"schema":"once.test_results.v1","target":"%s","runner":{{"type":"shellspec","metadata":{{}}}},"status":"%s","summary":{{"total":%s,"passed":%s,"failed":%s,"skipped":0,"flaky":0}},"cases":[' "{target}" "$run_status" "$total" "$passed" "$failed"
  cat "$cases_file"
  printf '],"artifacts":{{"logs":["%s"],"native_results":["%s"]}}}}\n' "$log" "$native_results"
}} > "$results"
exit "$status"
""".format(
        test_dir = test_dir,
        log = _shell_literal(log),
        results = _shell_literal(results),
        native_results = _shell_literal(native_results),
        command = _shell_words(runner_args),
        specs = _shell_words(spec_srcs),
        target = ctx["label"]["id"],
    )
    prepare_path(test_dir, kind = "directory", identifier = "apple_shellspec_test_dir:" + ctx["label"]["id"])
    run_action(
        argv = [host_which("sh"), "-c", script],
        inputs = inputs,
        outputs = [test_dir, results, log, native_results],
        env = action_env,
        toolchain_identity = "once.shellspec_test.v1\x00" + shellspec,
        identifier = "shellspec_test:" + ctx["label"]["id"],
    )
    return provider

def _apple_test_cases_script(swift_srcs, cases_file, target, runner_type, selectors = []):
    # List every test case from the bundle's sources into `cases_file` as the
    # `cases` array of the normalized results. The case `id` embeds the
    # `Suite/method` selector after `{target}::` so the sharded re-run can turn a
    # unit id back into an `-XCTest` selector. XCTest methods (`func testX`) take
    # their enclosing `XCTestCase` subclass as the suite; Swift Testing
    # functions (`@Test func x`) take their enclosing type, defaulting to the
    # file name for free functions.
    specs = _shell_words(swift_srcs)
    selected_cases = _shell_words(selectors)
    return """total=0
cases_file={cases_file}
: > "$cases_file"
emit_case() {{
  case_name="$1"
  case_suite="$2"
  case_file="$3"
  if [ {selector_count} -gt 0 ]; then
    selected=false
    selector="$case_suite/$case_name"
    for selected_case in {selected_cases}; do
      if [ "$selector" = "$selected_case" ]; then
        selected=true
        break
      fi
    done
    [ "$selected" = true ] || return 0
  fi
  if [ "$status" -eq 0 ]; then case_status=passed; else case_status=unknown; fi
  total=$((total + 1))
  if [ "$total" -gt 1 ]; then printf ',\n' >> "$cases_file"; fi
  printf '{{"id":"%s::%s/%s","name":"%s","suite":"%s","file":"%s","status":"%s","attempts":[{{"status":"%s"}}],"runner_metadata":{{"runner":"%s"}}}}' "{target}" "$case_suite" "$case_name" "$case_name" "$case_suite" "$case_file" "$case_status" "$case_status" "{runner_type}" >> "$cases_file"
}}
for spec in {specs}; do
  [ -f "$spec" ] || continue
  file_suite=${{spec%.swift}}
  file_suite=${{file_suite##*/}}
  current_suite="$file_suite"
  while IFS= read -r line; do
    case "$line" in
      *"class "*XCTestCase*)
        decl=${{line#*class }}
        decl=${{decl%%:*}}
        decl=${{decl%%[!A-Za-z0-9_]*}}
        [ -n "$decl" ] && current_suite="$decl"
        ;;
      *"struct "*|*"final class "*|*"actor "*)
        decl=${{line#*struct }}
        case "$line" in
          *"final class "*) decl=${{line#*final class }} ;;
          *"actor "*) decl=${{line#*actor }} ;;
        esac
        decl=${{decl%%:*}}
        decl=${{decl%%[!A-Za-z0-9_]*}}
        [ -n "$decl" ] && current_suite="$decl"
        ;;
    esac
    case "$line" in
      *"@Test func "*)
        name=${{line#*"@Test func "}}
        name=${{name%%"("*}}
        emit_case "$name" "$current_suite" "$spec"
        ;;
      *"func test"*"("*)
        name=${{line#*func }}
        name=${{name%%"("*}}
        case "$name" in
          test*) emit_case "$name" "$current_suite" "$spec" ;;
        esac
        ;;
    esac
  done < "$spec"
done
""".format(
        cases_file = _shell_literal(cases_file),
        specs = specs,
        selector_count = len(selectors),
        selected_cases = selected_cases,
        target = target,
        runner_type = runner_type,
    )

def _apple_test_info(ctx, runner_type, command_argv, command_env, labels, results, log, native_results):
    return {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {
            "type": runner_type,
            "display_name": "Swift Testing" if runner_type == "swift_testing" else "XCTest",
            "metadata": {},
        },
        "command": {
            "argv": command_argv,
            "env": command_env,
            "cwd": ".",
        },
        "outputs": {
            "results": results,
            "logs": [log],
            "native_results": [native_results],
            "coverage": [],
        },
        # Both runners list their cases from the normalized results the runner
        # writes, and both accept a case subset through `-XCTest` selectors, so
        # a bundle can be sharded case by case. Once a project's test bundles
        # each shard by case, `once test` fans the whole group of bundles into
        # one batch per case for parallel and remote execution.
        "listing": {
            "supported": True,
            "strategy": "normalized_results",
        },
        "filtering": {
            "case_filtering": "runner_args",
        },
        "sharding": {
            "supported": True,
            "granularity": "case",
        },
        "retries": {
            "supported": False,
            "default_attempts": 1,
        },
        "execution": {
            "cacheable": True,
            "run_from_workspace_root": True,
        },
        "labels": labels,
        "metadata": {},
    }

def _apple_library_impl(ctx):
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["module_name"])
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    # A product name may contain spaces; the Swift module name must be a valid
    # identifier, so normalize it the way Xcode derives PRODUCT_MODULE_NAME.
    module_name = _apple_swift_module_name(attrs.get("module_name") or ctx["label"]["name"])
    sdk_frameworks = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    swift_flags = attrs.get("swift_flags") or []
    clang_flags = attrs.get("clang_flags") or []
    per_source_clang_flags = attrs.get("per_source_clang_flags") or {}
    defines = attrs.get("defines") or []
    swift_defines = _unique(defines + (attrs.get("swift_defines") or []))
    clang_defines = _unique(defines + (attrs.get("clang_defines") or []))
    enable_testing = attrs.get("enable_testing") or False
    swift_testing = attrs.get("swift_testing") or False
    xctest_support = attrs.get("xctest_support") or False
    library_evolution = attrs.get("library_evolution") or False
    emit_dsym = attrs.get("emit_dsym") or False
    alwayslink = attrs.get("alwayslink") or False
    resources = attrs.get("resources") or []
    structured_resources = attrs.get("structured_resources") or []
    resource_bundle_name = attrs.get("resource_bundle_name") or ""
    resource_bundle_id = attrs.get("resource_bundle_id") or ""
    exported_deps = attrs.get("exported_deps") or []
    bridging_header = attrs.get("bridging_header") or ""
    prefix_header = attrs.get("prefix_header") or ""
    exported_headers = attrs.get("exported_headers") or []
    exported_header_dirs = attrs.get("exported_header_dirs") or []
    private_header_dirs = attrs.get("private_header_dirs") or []
    enable_modules = attrs.get("enable_modules") or False
    authored_modulemap = attrs.get("modulemap") or ""
    declared_modulemap_headers = [_package_relative(ctx, path) for path in (attrs.get("modulemap_headers") or [])]
    auxiliary_modulemaps = [_package_relative(ctx, path) for path in (attrs.get("auxiliary_modulemaps") or [])]

    generated_srcs = _apple_run_prebuild_actions(ctx, attrs)
    all_srcs = _unique(glob(ctx["srcs"]) + _apple_declared_source_paths(ctx) + generated_srcs)
    swift_srcs = _filter_swift_sources(all_srcs)
    objc_srcs = _filter_objc_sources(all_srcs)
    c_srcs = _filter_c_sources(all_srcs)
    cxx_srcs = _filter_cxx_sources(all_srcs)
    assembly_srcs = _filter_assembly_sources(all_srcs)
    if len(cxx_srcs) > 0:
        linkopts = list(linkopts) + ["-lc++"]
    if len(swift_srcs) == 0 and len(objc_srcs) == 0 and len(c_srcs) == 0 and len(cxx_srcs) == 0 and len(assembly_srcs) == 0:
        fail("apple_library " + ctx["label"]["id"] + " has no compilable sources (.swift/.m/.mm/.c/.cc/.cpp/.cxx/.s/.S)")

    archs_attr = attrs.get("archs") or []
    archs = archs_attr if len(archs_attr) > 0 else [host_arch()]
    _reject_multi_arch_selects(ctx["attr"], ctx["label"]["id"], archs)
    mac_catalyst = attrs.get("mac_catalyst") or False
    if mac_catalyst and platform != "macos" and platform != "macosx":
        fail("apple_library " + ctx["label"]["id"] + " sets mac_catalyst = true but platform = `" + platform + "`; mac_catalyst requires platform = macos")
    is_universal = len(archs) > 1

    swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
    testing_framework_dir = ""
    testing_usr_lib_dir = ""
    testing_macros_plugin = ""
    if swift_testing or xctest_support:
        if xcode_developer_dir:
            testing_platform_path = _developer_platform_path(xcode_developer_dir, swiftc["sdk_name"])
        else:
            testing_platform_path = host_command([host_which("xcrun"), "--sdk", swiftc["sdk_name"], "--show-sdk-platform-path"], env = swiftc["env"]).strip()
        testing_framework_dir = testing_platform_path + "/Developer/Library/Frameworks"
        testing_usr_lib_dir = testing_platform_path + "/Developer/usr/lib"
    if swift_testing:
        testing_macros_plugin = _swift_testing_macros_plugin(swiftc["swiftc_path"])
    archive = declare_output(module_name + ".a")

    deps = _apple_native_deps(ctx)
    _validate_apple_native_deps(deps, ctx["label"]["id"])
    # Split deps into compile-visible (exported) and link-only.
    # exported_deps entries come straight from `[target.attrs]` and may
    # be `./Sibling`, `../web/Common`, or already root-relative; we
    # normalise each one to the absolute id format `dep["label_id"]`
    # carries so the membership check works regardless of how the
    # manifest author wrote the reference.
    package = ctx["label"]["package"]
    exported_dep_ids = {}
    for ref in exported_deps:
        exported_dep_ids[_resolve_dep_ref(ref, package)] = True
    exported_dep_indices = []
    for index, dep in enumerate(deps):
        dep_label = dep.get("label_id")
        if dep_label and dep_label in exported_dep_ids:
            exported_dep_indices.append(index)

    compile_swiftmodule_dirs = []
    for dep in deps:
        for dir in dep.get("transitive_swiftmodule_dirs") or []:
            if dir != ctx["build_dir"] and dir not in compile_swiftmodule_dirs:
                compile_swiftmodule_dirs.append(dir)

    # Compile-visible header dirs: direct deps' exported headers'
    # parent directories. Used as `-I` flags for both clang and
    # swiftc's `-Xcc -I` (so Swift can see ObjC types via the
    # bridging header or import path).
    compile_header_dirs = []
    for dep in deps:
        for h in dep.get("transitive_exported_header_dirs") or []:
            if h not in compile_header_dirs:
                compile_header_dirs.append(h)
    dep_modulemaps = []
    for dep in deps:
        for m in dep.get("transitive_modulemaps") or []:
            if m not in dep_modulemaps:
                dep_modulemaps.append(m)
    dep_hmaps = []
    for dep in deps:
        for h in dep.get("transitive_hmaps") or []:
            if h not in dep_hmaps:
                dep_hmaps.append(h)
    plugin_dylibs, plugin_executables = _apple_collect_swift_plugins(deps)
    compile_framework_bundles = _apple_collect_link_framework_bundles(deps)
    compile_framework_search_dirs = []
    compile_framework_files = []
    compile_vfs_overlays = []
    for bundle in compile_framework_bundles:
        framework_parent = _parent_dir(bundle["path"])
        if framework_parent and framework_parent not in compile_framework_search_dirs:
            compile_framework_search_dirs.append(framework_parent)
        for file in _apple_framework_compile_files(bundle):
            if file not in compile_framework_files:
                compile_framework_files.append(file)
    for dep in deps:
        for framework_dir in dep.get("transitive_framework_search_dirs") or []:
            if framework_dir and framework_dir not in compile_framework_search_dirs:
                compile_framework_search_dirs.append(framework_dir)
        for file in dep.get("transitive_framework_files") or []:
            if file and file not in compile_framework_files:
                compile_framework_files.append(file)
        for file in dep.get("transitive_generated_headers") or []:
            if file not in compile_framework_files:
                compile_framework_files.append(file)
        for file in dep.get("transitive_exported_headers") or []:
            if file not in compile_framework_files:
                compile_framework_files.append(file)
        for overlay in dep.get("transitive_vfs_overlays") or []:
            if overlay and overlay not in compile_vfs_overlays:
                compile_vfs_overlays.append(overlay)

    # Own exported headers as workspace-relative paths, plus the
    # dirs we expose to consumers.
    own_exported_headers = [_package_relative(ctx, h) for h in exported_headers]
    own_exported_header_dirs = _unique_dirs(own_exported_headers)
    auxiliary_modulemap_headers = []
    for auxiliary_modulemap in auxiliary_modulemaps:
        auxiliary_dir = _parent_dir(auxiliary_modulemap)
        if auxiliary_dir and auxiliary_dir not in own_exported_header_dirs:
            own_exported_header_dirs.append(auxiliary_dir)
        for header in glob([auxiliary_dir + "/**/*.h"]):
            if header not in auxiliary_modulemap_headers:
                auxiliary_modulemap_headers.append(header)
    for header_dir in exported_header_dirs:
        resolved_header_dir = _package_relative(ctx, header_dir)
        if resolved_header_dir and resolved_header_dir not in own_exported_header_dirs:
            own_exported_header_dirs.append(resolved_header_dir)
    own_private_header_dirs = []
    for header_dir in private_header_dirs:
        resolved_header_dir = _package_relative(ctx, header_dir)
        if resolved_header_dir and resolved_header_dir not in own_private_header_dirs:
            own_private_header_dirs.append(resolved_header_dir)
    own_private_header_files = _apple_header_inputs(ctx, own_private_header_dirs)

    staged_headers = []
    staged_headers_dir = ""
    if enable_modules and len(own_exported_headers) > 0:
        staged_headers_dir = ctx["build_dir"] + "/Headers"

    # Modulemap generation: if the target exports headers AND opts into
    # clang modules, write a modulemap so consumers can `import` the
    # module without listing each header on the command line. This is
    # the minimum Buck2 and Bazel Apple implementations do; framework modules and umbrella
    # headers can layer on later.
    modulemap_path = authored_modulemap
    compile_modulemap_path = authored_modulemap
    generated_modulemap_lines = []
    generated_framework_module = False
    generated_framework_search_dir = ""
    generated_framework_root = ""
    generated_framework_overlay = ""
    authored_modulemap_headers = _unique((glob([_parent_dir(authored_modulemap) + "/*.h"]) if authored_modulemap else []) + declared_modulemap_headers)
    if authored_modulemap:
        authored_modulemap_path = authored_modulemap if authored_modulemap.startswith("/") else workspace_root() + "/" + authored_modulemap
        authored_modulemap_contents = host_file_read(authored_modulemap_path)
        if authored_modulemap_contents.strip().startswith("framework module "):
            generated_framework_module = True
            modulemap_path = declare_output(module_name + ".framework/Modules/module.modulemap")
            generated_framework_root = _parent_dir(_parent_dir(modulemap_path))
            compile_modulemap_path = declare_output("Unextended/" + module_name + ".framework/Modules/module.modulemap")
            compile_framework_root = _parent_dir(_parent_dir(compile_modulemap_path))
            generated_framework_search_dir = _parent_dir(compile_framework_root)
            prepare_path(generated_framework_root, kind = "remove", identifier = "clean_framework_module_" + module_name)
            prepare_path(compile_framework_root, kind = "remove", identifier = "clean_unextended_framework_module_" + module_name)
            copy_path(
                authored_modulemap,
                compile_modulemap_path,
                inputs = [authored_modulemap],
                identifier = "stage_unextended_framework_modulemap_" + module_name,
            )
            consumer_modulemap_contents = authored_modulemap_contents
            if len(swift_srcs) > 0 and not is_universal:
                consumer_modulemap_contents = consumer_modulemap_contents.rstrip() + "\nmodule " + module_name + ".Swift {\n    header \"" + module_name + "-Swift.h\"\n    requires objc\n    export *\n}\n"
            write_path(modulemap_path, consumer_modulemap_contents)
            for header in _unique(own_exported_headers + authored_modulemap_headers):
                staged_header = generated_framework_root + "/Headers/" + _basename(header)
                copy_path(
                    header,
                    staged_header,
                    inputs = [header],
                    identifier = "stage_framework_header_" + module_name + "_" + _basename(header),
                )
                staged_headers.append(staged_header)
                compile_staged_header = compile_framework_root + "/Headers/" + _basename(header)
                copy_path(
                    header,
                    compile_staged_header,
                    inputs = [header],
                    identifier = "stage_unextended_framework_header_" + module_name + "_" + _basename(header),
                )
                staged_headers.append(compile_staged_header)
        else:
            # Keep one canonical path for an authored non-framework map. Clang
            # resolves relative header entries from the map's own directory and
            # can also discover a conventional `module.modulemap` through `-I`.
            modulemap_path = authored_modulemap
            compile_modulemap_path = authored_modulemap
    if not modulemap_path and enable_modules and len(own_exported_headers) > 0:
        umbrella_header = ""
        for header in own_exported_headers:
            if _basename(header) == module_name + ".h":
                umbrella_header = header
                break
        use_umbrella = umbrella_header != ""
        generated_framework_module = use_umbrella
        if use_umbrella:
            modulemap_path = declare_output(module_name + ".framework/Modules/module.modulemap")
            framework_root = _parent_dir(_parent_dir(modulemap_path))
            generated_framework_root = framework_root
            compile_modulemap_path = declare_output("Unextended/" + module_name + ".framework/Modules/module.modulemap")
            compile_framework_root = _parent_dir(_parent_dir(compile_modulemap_path))
            generated_framework_search_dir = _parent_dir(compile_framework_root)
            prepare_path(generated_framework_root, kind = "remove", identifier = "clean_framework_module_" + module_name)
            prepare_path(compile_framework_root, kind = "remove", identifier = "clean_unextended_framework_module_" + module_name)
            for header in own_exported_headers:
                staged_header = framework_root + "/Headers/" + _basename(header)
                copy_path(
                    header,
                    staged_header,
                    inputs = [header],
                    identifier = "stage_framework_header_" + module_name + "_" + _basename(header),
                )
                staged_headers.append(staged_header)
                compile_staged_header = compile_framework_root + "/Headers/" + _basename(header)
                copy_path(
                    header,
                    compile_staged_header,
                    inputs = [header],
                    identifier = "stage_unextended_framework_header_" + module_name + "_" + _basename(header),
                )
                staged_headers.append(compile_staged_header)
        else:
            modulemap_path = declare_output("underlying.modulemap" if len(swift_srcs) > 0 and not is_universal else "module.modulemap")
            compile_modulemap_path = modulemap_path
        modulemap_lines = [("framework module " if use_umbrella else "module ") + module_name + " {"]
        if use_umbrella:
            modulemap_lines.append("    umbrella header \"" + _basename(umbrella_header) + "\"")
        else:
            for header in sorted(own_exported_headers):
                relative_header = ("../" * len(ctx["build_dir"].split("/"))) + header
                modulemap_lines.append("    header \"" + relative_header + "\"")
        modulemap_lines.append("    export *")
        if use_umbrella:
            modulemap_lines.append("    module * { export * }")
        modulemap_lines.append("}")
        modulemap_lines.append("")
        generated_modulemap_lines = modulemap_lines
        if generated_framework_module:
            write_path(compile_modulemap_path, "\n".join(modulemap_lines))
            consumer_lines = list(modulemap_lines)
            if len(swift_srcs) > 0 and not is_universal:
                consumer_lines.extend([
                    "module " + module_name + ".Swift {",
                    "    header \"" + module_name + "-Swift.h\"",
                    "    requires objc",
                    "    export *",
                    "}",
                    "",
                ])
            write_path(modulemap_path, "\n".join(consumer_lines))
        else:
            write_path(modulemap_path, "\n".join(modulemap_lines))

    if generated_framework_module:
        generated_framework_overlay = declare_output("framework-headers-overlay.yaml")
        overlay_contents = []
        for header in _unique(own_exported_headers + authored_modulemap_headers):
            overlay_contents.append({
                "type": "file",
                "name": _basename(header),
                "external-contents": workspace_root() + "/" + header,
            })
        write_path(
            generated_framework_overlay,
            _json_encode({
                "version": 0,
                "case-sensitive": False,
                "roots": [{
                    "type": "directory",
                    "name": workspace_root() + "/" + generated_framework_root + "/Headers",
                    "contents": overlay_contents,
                }],
            }) + "\n",
        )

    # A Swift library can be consumed by Objective-C through `@import Foo`.
    # Give a pure Swift target a Clang module that exposes the generated
    # `Foo-Swift.h`, without making its Swift compiler import that module
    # before the header exists.
    swift_interop_modulemap = ""
    if len(swift_srcs) > 0 and not modulemap_path:
        swift_interop_modulemap = declare_output("Headers/" + module_name + "/module.modulemap")
        write_path(
            swift_interop_modulemap,
            "module " + module_name + " {\n    header \"" + module_name + "-Swift.h\"\n    export *\n}\n",
        )

    swift_submodulemap = ""
    swift_submodule_replaces_modulemap = False
    if len(swift_srcs) > 0 and modulemap_path and not is_universal and not generated_framework_module:
        swift_submodulemap = declare_output("module.modulemap" if generated_modulemap_lines else "swift.modulemap")
        if generated_modulemap_lines and not generated_framework_module:
            consumer_lines = generated_modulemap_lines[:-2]
            consumer_lines.extend([
                "    module Swift {",
                "        header \"Headers/" + module_name + "/" + module_name + "-Swift.h\"",
                "        requires objc",
                "        export *",
                "    }",
                "}",
                "",
            ])
            write_path(swift_submodulemap, "\n".join(consumer_lines))
            swift_submodule_replaces_modulemap = True
        else:
            write_path(
                swift_submodulemap,
                "module " + module_name + ".Swift {\n    header \"Headers/" + module_name + "/" + module_name + "-Swift.h\"\n    requires objc\n    export *\n}\n",
            )

    # Header map generation: cover the `#include "Foo.h"` and
    # `#include <Module/Foo.h>` lookup styles that a pure modulemap
    # doesn't help with. The target's own map includes private headers
    # because its sources may use module-qualified imports for them, but
    # only exported headers flow into dependency providers. Each entry
    # maps to the header's workspace-absolute path so compilation does
    # not depend on broad directory inputs.
    hmap_path = ""
    if enable_modules and len(own_exported_headers) > 0:
        hmap_path = declare_output(module_name + ".hmap")
        hmap_entries = {}
        for header in own_exported_headers:
            base = _basename(header)
            absolute_header = workspace_root() + "/" + header
            hmap_entries[base] = absolute_header
            hmap_entries[module_name + "/" + base] = absolute_header
        for header in own_private_header_files:
            base = _basename(header)
            absolute_header = workspace_root() + "/" + header
            if base not in hmap_entries:
                hmap_entries[base] = absolute_header
            module_key = module_name + "/" + base
            if module_key not in hmap_entries:
                hmap_entries[module_key] = absolute_header
        if len(swift_srcs) > 0 and staged_headers_dir:
            generated_swift_header = (generated_framework_root + "/Headers/" + module_name + "-Swift.h") if generated_framework_module else (staged_headers_dir + "/" + module_name + "/" + module_name + "-Swift.h")
            hmap_entries[module_name + "/" + module_name + "-Swift.h"] = workspace_root() + "/" + generated_swift_header
        _write_hmap(hmap_path, hmap_entries)

    exported_deps_records = [deps[i] for i in exported_dep_indices]
    transitive_swiftmodule_dirs = _collect_transitive(
        exported_deps_records,
        "transitive_swiftmodule_dirs",
        [ctx["build_dir"]] if len(swift_srcs) > 0 else [],
    )
    transitive_exported_header_dirs = _collect_transitive(
        exported_deps_records,
        "transitive_exported_header_dirs",
        own_exported_header_dirs,
    )
    transitive_exported_headers = _collect_transitive(
        exported_deps_records,
        "transitive_exported_headers",
        own_exported_headers,
    )
    own_modulemaps = ([] if generated_framework_module else (([swift_submodulemap] if swift_submodule_replaces_modulemap else ([modulemap_path] + ([swift_submodulemap] if swift_submodulemap else []))) if modulemap_path else ([swift_interop_modulemap] if swift_interop_modulemap else []))) + auxiliary_modulemaps
    transitive_modulemaps = _collect_transitive(
        exported_deps_records,
        "transitive_modulemaps",
        own_modulemaps,
    )
    transitive_hmaps = _collect_transitive(
        exported_deps_records,
        "transitive_hmaps",
        [hmap_path] if hmap_path and not generated_framework_module else [],
    )
    transitive_archives = _collect_transitive(deps, "transitive_archives", [archive])
    transitive_sdk_frameworks = _collect_transitive(deps, "transitive_sdk_frameworks", sdk_frameworks)
    transitive_weak_sdk_frameworks = _collect_transitive(deps, "transitive_weak_sdk_frameworks", weak_sdk_frameworks)
    transitive_sdk_dylibs = _collect_transitive(deps, "transitive_sdk_dylibs", sdk_dylibs)
    transitive_linkopts = _apple_collect_transitive_linkopts(deps, linkopts)
    transitive_plugin_dylibs = _collect_transitive(deps, "transitive_plugin_dylibs", plugin_dylibs)
    transitive_plugin_executables = _collect_transitive(deps, "transitive_plugin_executables", plugin_executables)
    transitive_defines = _collect_transitive(deps, "transitive_defines", defines)
    transitive_alwayslink_archives = _collect_transitive(deps, "transitive_alwayslink_archives", [archive] if alwayslink else [])
    transitive_link_framework_bundles = _apple_collect_link_framework_bundles(deps)
    transitive_framework_bundles = _apple_collect_runtime_framework_bundles(deps)

    # --- Per-arch compile pipeline -----------------------------------
    # When a target requests a single architecture (the default,
    # `host_arch()`), the per-arch archive is the final archive
    # directly and no lipo step runs. With more than one arch each
    # compile emits a per-arch archive and a final `lipo -create`
    # action combines them.
    swift_only = len(objc_srcs) == 0 and len(c_srcs) == 0 and len(cxx_srcs) == 0 and len(assembly_srcs) == 0
    per_arch_archives = []
    swift_objc_header_holder = [""]

    def _compile_for_arch(arch):
        triple = _apple_triple(platform, target_sdk_version, sdk_variant, arch, mac_catalyst)
        arch_suffix = "-" + arch if is_universal else ""

        per_arch_archive = declare_output(module_name + arch_suffix + ".a") if is_universal else archive
        if is_universal:
            swiftmodule = declare_output(module_name + ".swiftmodule/" + arch + ".swiftmodule") if len(swift_srcs) > 0 else ""
            swiftdoc = declare_output(module_name + ".swiftmodule/" + arch + ".swiftdoc") if len(swift_srcs) > 0 else ""
        else:
            swiftmodule = declare_output(module_name + ".swiftmodule") if len(swift_srcs) > 0 else ""
            swiftdoc = declare_output(module_name + ".swiftdoc") if len(swift_srcs) > 0 else ""
        swift_objc_header = declare_output((module_name + ".framework/Headers/" + module_name + arch_suffix + "-Swift.h") if generated_framework_module else (("Headers/" + module_name + "/" + module_name + arch_suffix + "-Swift.h") if (staged_headers_dir or swift_interop_modulemap) else (module_name + arch_suffix + "-Swift.h"))) if len(swift_srcs) > 0 else ""
        swift_objc_header_holder[0] = swift_objc_header

        # Swift output: per_arch_archive for swift-only, else
        # an intermediate that libtool merges with the clang objects.
        swift_archive = per_arch_archive if swift_only else (declare_output(module_name + "-swift" + arch_suffix + ".a") if len(swift_srcs) > 0 else "")

        if len(swift_srcs) > 0:
            swift_base_argv = list(swiftc["argv"]) + [
                "-module-name",
                module_name,
                "-target",
                triple,
            ]
            # Mixed-language targets compile Objective-C against Swift's
            # generated compatibility header. In library parsing mode Swift
            # omits internal `@objc` declarations from that header, even
            # though they are valid collaborators within the same target.
            # Swift-only libraries retain library parsing semantics.
            if swift_only:
                swift_base_argv.append("-parse-as-library")
            if emit_dsym:
                swift_base_argv.append("-g")
            if enable_testing:
                swift_base_argv.append("-enable-testing")
            if library_evolution:
                swift_base_argv.append("-enable-library-evolution")
            if bridging_header:
                swift_base_argv.extend(["-import-objc-header", _package_relative(ctx, bridging_header)])
            if compile_modulemap_path and not bridging_header:
                swift_base_argv.append("-import-underlying-module")
            if generated_framework_module:
                swift_base_argv.append("-explicit-module-build")
            for framework in sdk_frameworks:
                swift_base_argv.extend(["-framework", framework])
            for framework in weak_sdk_frameworks:
                _apple_append_weak_framework(swift_base_argv, framework)
            for dep_dir in compile_swiftmodule_dirs:
                swift_base_argv.extend(["-I", dep_dir])
            for framework_dir in compile_framework_search_dirs:
                swift_base_argv.extend(["-F", framework_dir])
            _apple_disable_static_framework_autolinking(swift_base_argv, compile_framework_bundles)
            if generated_framework_module:
                swift_base_argv.extend(["-F", generated_framework_search_dir])
            if swift_testing:
                swift_base_argv.extend(["-F", testing_framework_dir, "-framework", "Testing", "-load-plugin-library", testing_macros_plugin])
            if xctest_support:
                swift_base_argv.extend(["-F", testing_framework_dir, "-I", testing_usr_lib_dir, "-L", testing_usr_lib_dir, "-framework", "XCTest", "-lXCTestSwiftSupport"])
            # Header search paths flow through `-Xcc -I` so swiftc's
            # underlying Clang invocation (for bridging headers + ObjC
            # interop) can locate dep headers.
            for hdir in compile_header_dirs:
                swift_base_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
            for hdir in own_private_header_dirs:
                swift_base_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
            # Feed each dep's modulemap to swiftc's underlying Clang so
            # `import` of a clang-module dep resolves without manual
            # `-fmodule-map-file` from the user.
            if compile_modulemap_path and not generated_framework_module and not bridging_header:
                swift_base_argv.extend(["-Xcc", "-fmodule-map-file=" + compile_modulemap_path])
            for mmap in auxiliary_modulemaps:
                swift_base_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
            for mmap in dep_modulemaps:
                swift_base_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
            # Header maps flow through Clang's `-I` search, so the bridging
            # header (and any dep ObjC interop) can resolve `#include "Foo.h"`
            # without enumerating include directories.
            if hmap_path and not generated_framework_module:
                swift_base_argv.extend(["-Xcc", "-I", "-Xcc", hmap_path])
            for hmap in dep_hmaps:
                swift_base_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
            for overlay in compile_vfs_overlays:
                swift_base_argv.extend(["-Xcc", "-ivfsoverlay", "-Xcc", overlay])
            if enable_modules:
                swift_base_argv.extend(["-Xcc", "-fmodules"])
            _apple_add_swift_plugin_args(swift_base_argv, plugin_dylibs, plugin_executables)
            for define in swift_defines:
                swift_base_argv.extend(["-D", define])
            for define in clang_defines:
                swift_base_argv.extend(["-Xcc", "-D" + define])
            for flag in swift_flags:
                swift_base_argv.append(flag)

            swift_inputs = list(swift_srcs)
            if bridging_header:
                swift_inputs.append(_package_relative(ctx, bridging_header))
            # The bridging header may #include other headers, so feed
            # each exported header through as an action input too.
            for h in own_exported_headers:
                if h not in swift_inputs:
                    swift_inputs.append(h)
            for h in own_private_header_files:
                if h not in swift_inputs:
                    swift_inputs.append(h)
            if compile_modulemap_path:
                swift_inputs.append(compile_modulemap_path)
            for mmap in auxiliary_modulemaps:
                if mmap not in swift_inputs:
                    swift_inputs.append(mmap)
            for header in auxiliary_modulemap_headers:
                if header not in swift_inputs:
                    swift_inputs.append(header)
            for header in authored_modulemap_headers:
                if header not in swift_inputs:
                    swift_inputs.append(header)
            for mmap in dep_modulemaps:
                if mmap not in swift_inputs:
                    swift_inputs.append(mmap)
            if hmap_path:
                swift_inputs.append(hmap_path)
            for header in staged_headers:
                swift_inputs.append(header)
            for plugin_input in _apple_swift_plugin_inputs(plugin_dylibs, plugin_executables):
                if plugin_input not in swift_inputs:
                    swift_inputs.append(plugin_input)
            for file in compile_framework_files:
                if file not in swift_inputs:
                    swift_inputs.append(file)
            for overlay in compile_vfs_overlays:
                if overlay not in swift_inputs:
                    swift_inputs.append(overlay)

            swift_object_relative_dir = "Objects/" + module_name + arch_suffix
            swift_object_dir = ctx["build_dir"] + "/" + swift_object_relative_dir
            swift_object_map = {}
            swift_objects = []
            swift_output_file_map = ""
            if _apple_swift_emits_single_object(swift_flags):
                swift_objects.append(declare_output(module_name + arch_suffix + ".o"))
            else:
                for source_index in range(len(swift_srcs)):
                    src = swift_srcs[source_index]
                    object_name = str(source_index) + "-" + _apple_swift_module_name(src.replace("/", "_")) + ".o"
                    obj = declare_output(swift_object_relative_dir + "/" + object_name)
                    swift_objects.append(obj)
                    swift_object_map[src] = {"object": obj}
                swift_output_file_map = declare_output(module_name + arch_suffix + "-output-file-map.json")
                write_path(swift_output_file_map, _json_encode(swift_object_map))

            swift_compile_argv = list(swift_base_argv)
            if not swift_only:
                swift_compile_argv.extend([
                    "-parse-as-library",
                    "-Xfrontend",
                    "-emit-clang-header-min-access",
                    "-Xfrontend",
                    "internal",
                ])
            swift_compile_argv.extend([
                "-c",
                "-emit-module",
                "-emit-module-path",
                swiftmodule,
                "-emit-objc-header",
                "-emit-objc-header-path",
                swift_objc_header,
            ])
            if swift_output_file_map:
                swift_compile_argv.extend(["-output-file-map", swift_output_file_map])
            else:
                swift_compile_argv.extend(["-o", swift_objects[0]])
            for src in swift_srcs:
                swift_compile_argv.append(src)
            swift_compile_inputs = list(swift_inputs) + ([swift_output_file_map] if swift_output_file_map else [])
            swift_compile_outputs = [swiftmodule, swiftdoc, swift_objc_header] + swift_objects
            obsolete_modulemaps = [
                ctx["build_dir"] + "/module.modulemap",
                ctx["build_dir"] + "/swift.modulemap",
                ctx["build_dir"] + "/underlying.modulemap",
            ] if generated_framework_module else []

            if authored_modulemap:
                run_action(
                    argv = swift_compile_argv,
                    inputs = swift_compile_inputs,
                    outputs = swift_compile_outputs,
                    clean_paths = obsolete_modulemaps,
                    create_dirs = [swift_object_dir],
                    env = swiftc["env"],
                    sandbox = "off",
                    toolchain_identity = swiftc["identity"],
                    identifier = "swift_module_compile_" + module_name + arch_suffix,
                )
            else:
                run_action(
                    argv = swift_compile_argv,
                    inputs = swift_compile_inputs,
                    outputs = swift_compile_outputs,
                    clean_paths = obsolete_modulemaps,
                    create_dirs = [swift_object_dir],
                    env = swiftc["env"],
                    toolchain_identity = swiftc["identity"],
                    identifier = "swift_module_compile_" + module_name + arch_suffix,
                )

            swift_libtool = _resolve_libtool(platform, sdk_variant, xcode_developer_dir)
            run_action(
                argv = list(swift_libtool["argv"]) + ["-static", "-o", swift_archive] + swift_objects,
                inputs = list(swift_objects),
                outputs = [swift_archive],
                env = swift_libtool["env"],
                toolchain_identity = swift_libtool["identity"],
                identifier = "libtool_swift_archive_" + module_name + arch_suffix,
            )

        arch_clang_objects = []
        if len(objc_srcs) > 0 or len(c_srcs) > 0 or len(cxx_srcs) > 0 or len(assembly_srcs) > 0:
            clang = _resolve_clang(platform, sdk_variant, xcode_developer_dir)

            def compile_with_clang(src, language):
                is_assembly = language == "assembler-with-cpp"
                # Sanitise the source path into a stable .o filename
                # under the build dir: `apps/ios/AppCore/Sources/A.m`
                # → `apps_ios_AppCore_Sources_A.m.o` (with `-<arch>`
                # appended for universal builds).
                sanitised = src.replace("/", "_")
                obj = declare_output(sanitised + arch_suffix + ".o")
                argv = [
                    clang["clang_path"] if language != "c++" else clang["clangxx_path"],
                    "-c",
                    "-x",
                    language,
                    "-arch",
                    arch,
                    "-isysroot",
                    clang["sdk_path"],
                    "-target",
                    triple,
                    "-o",
                    obj,
                ]
                if language == "objective-c" or language == "objective-c++":
                    argv.append("-fobjc-arc")
                if emit_dsym:
                    argv.append("-g")
                if enable_modules and not is_assembly:
                    argv.append("-fmodules")
                    argv.append("-fmodule-name=" + module_name)
                if not is_assembly:
                    for framework in sdk_frameworks:
                        argv.extend(["-framework", framework])
                for hdir in compile_header_dirs:
                    argv.extend(["-I", hdir])
                for hdir in own_exported_header_dirs:
                    argv.extend(["-I", hdir])
                for hdir in own_private_header_dirs:
                    argv.extend(["-I", hdir])
                for framework_dir in compile_framework_search_dirs:
                    argv.extend(["-F", framework_dir])
                if modulemap_path and not is_assembly:
                    argv.append("-fmodule-map-file=" + modulemap_path)
                if not is_assembly:
                    for mmap in auxiliary_modulemaps:
                        argv.append("-fmodule-map-file=" + mmap)
                    for mmap in dep_modulemaps:
                        argv.append("-fmodule-map-file=" + mmap)
                if hmap_path:
                    argv.extend(["-I", hmap_path])
                for hmap in dep_hmaps:
                    argv.extend(["-I", hmap])
                for overlay in compile_vfs_overlays:
                    argv.extend(["-ivfsoverlay", overlay])
                if prefix_header and not is_assembly:
                    argv.extend(["-include", _package_relative(ctx, prefix_header)])
                for define in clang_defines:
                    argv.append("-D" + define)
                for flag in clang_flags:
                    if is_assembly and flag.startswith("-std="):
                        continue
                    if language != "c++" and flag.startswith("-std=c++"):
                        continue
                    argv.append(flag)
                for flag in json_decode(per_source_clang_flags.get(src) or "[]"):
                    if is_assembly and flag.startswith("-std="):
                        continue
                    if language != "c++" and flag.startswith("-std=c++"):
                        continue
                    argv.append(flag)
                argv.append(src)
                inputs = [src]
                for h in own_exported_headers:
                    if h not in inputs:
                        inputs.append(h)
                for h in own_private_header_files:
                    if h not in inputs:
                        inputs.append(h)
                if modulemap_path:
                    inputs.append(modulemap_path)
                for mmap in auxiliary_modulemaps:
                    if mmap not in inputs:
                        inputs.append(mmap)
                for header in auxiliary_modulemap_headers:
                    if header not in inputs:
                        inputs.append(header)
                for header in authored_modulemap_headers:
                    if header not in inputs:
                        inputs.append(header)
                for mmap in dep_modulemaps:
                    if mmap not in inputs:
                        inputs.append(mmap)
                if hmap_path:
                    inputs.append(hmap_path)
                for header in staged_headers:
                    inputs.append(header)
                if swift_objc_header:
                    inputs.append(swift_objc_header)
                if prefix_header and not is_assembly:
                    inputs.append(_package_relative(ctx, prefix_header))
                for file in compile_framework_files:
                    if file not in inputs:
                        inputs.append(file)
                for overlay in compile_vfs_overlays:
                    if overlay not in inputs:
                        inputs.append(overlay)
                if authored_modulemap:
                    run_action(
                        argv = argv,
                        inputs = inputs,
                        outputs = [obj],
                        env = clang["env"],
                        sandbox = "off",
                        toolchain_identity = clang["identity"],
                        identifier = "clang_compile_" + module_name + arch_suffix + "_" + sanitised,
                    )
                else:
                    run_action(
                        argv = argv,
                        inputs = inputs,
                        outputs = [obj],
                        env = clang["env"],
                        toolchain_identity = clang["identity"],
                        identifier = "clang_compile_" + module_name + arch_suffix + "_" + sanitised,
                    )
                arch_clang_objects.append(obj)

            for src in objc_srcs:
                compile_with_clang(src, "objective-c++" if src.endswith(".mm") else "objective-c")
            for src in c_srcs:
                compile_with_clang(src, "c")
            for src in cxx_srcs:
                compile_with_clang(src, "c++")
            for src in assembly_srcs:
                compile_with_clang(src, "assembler-with-cpp")

        # Libtool merge into per_arch_archive. Only needed when there
        # is at least one non-Swift input alongside Swift; Swift-only
        # and C-only libraries already wrote into per_arch_archive.
        if not swift_only and len(swift_srcs) > 0:
            libtool = _resolve_libtool(platform, sdk_variant, xcode_developer_dir)
            libtool_argv = list(libtool["argv"]) + [
                "-static",
                "-o",
                per_arch_archive,
                swift_archive,
            ]
            libtool_argv.extend(arch_clang_objects)
            libtool_inputs = [swift_archive]
            libtool_inputs.extend(arch_clang_objects)
            run_action(
                argv = libtool_argv,
                inputs = libtool_inputs,
                outputs = [per_arch_archive],
                env = libtool["env"],
                toolchain_identity = libtool["identity"],
                identifier = "libtool_merge_" + module_name + arch_suffix,
            )
        elif len(swift_srcs) == 0 and len(arch_clang_objects) > 0:
            libtool = _resolve_libtool(platform, sdk_variant, xcode_developer_dir)
            libtool_argv = list(libtool["argv"]) + ["-static", "-o", per_arch_archive]
            libtool_argv.extend(arch_clang_objects)
            run_action(
                argv = libtool_argv,
                inputs = list(arch_clang_objects),
                outputs = [per_arch_archive],
                env = libtool["env"],
                toolchain_identity = libtool["identity"],
                identifier = "libtool_archive_" + module_name + arch_suffix,
            )

        return per_arch_archive

    for arch in archs:
        per_arch_archives.append(_compile_for_arch(arch))

    # --- lipo merge --------------------------------------------------
    # For universal builds, combine the per-arch archives into the
    # final fat archive. Single-arch builds skip this entirely; the
    # one per-arch archive already wrote into `archive` directly.
    if is_universal:
        lipo = _resolve_lipo(platform, sdk_variant, xcode_developer_dir)
        lipo_argv = list(lipo["argv"]) + ["-create", "-output", archive]
        lipo_argv.extend(per_arch_archives)
        run_action(
            argv = lipo_argv,
            inputs = list(per_arch_archives),
            outputs = [archive],
            env = lipo["env"],
            toolchain_identity = lipo["identity"],
            identifier = "lipo_" + module_name,
        )

    swift_objc_header = swift_objc_header_holder[0]
    own_framework_search_dirs = [_parent_dir(generated_framework_root)] if generated_framework_module else []
    own_framework_files = []
    if generated_framework_module:
        own_framework_files.append(modulemap_path)
        own_framework_files.append(generated_framework_overlay)
        for header in staged_headers:
            if header.startswith(generated_framework_root + "/"):
                own_framework_files.append(header)
        if swift_objc_header:
            own_framework_files.append(swift_objc_header)
    transitive_framework_search_dirs = _collect_transitive(
        exported_deps_records,
        "transitive_framework_search_dirs",
        own_framework_search_dirs,
    )
    transitive_framework_files = _collect_transitive(
        exported_deps_records,
        "transitive_framework_files",
        own_framework_files,
    )
    transitive_vfs_overlays = _collect_transitive(
        exported_deps_records,
        "transitive_vfs_overlays",
        [generated_framework_overlay] if generated_framework_overlay else [],
    )
    transitive_generated_headers = _collect_transitive(
        exported_deps_records,
        "transitive_generated_headers",
        ([swift_objc_header] if swift_objc_header else []) + auxiliary_modulemap_headers,
    )
    own_resource_bundles = []
    if resource_bundle_name:
        own_resource_bundles.append(_apple_create_resource_bundle(
            ctx,
            resources,
            structured_resources,
            resource_bundle_name,
            resource_bundle_id or ("dev.once." + module_name + ".resources"),
            platform,
            minimum_os,
            xcode_developer_dir,
            module_name,
        ))
    transitive_resource_bundles = _apple_collect_resource_bundles(deps, own_resource_bundles)

    return {
        "label_id": ctx["label"]["id"],
        "swiftmodule_dir": ctx["build_dir"] if len(swift_srcs) > 0 else "",
        "archive": archive,
        "objc_header": swift_objc_header,
        "alwayslink": alwayslink,
        "exported_headers": own_exported_headers,
        "exported_header_dirs": own_exported_header_dirs,
        "modulemap": (swift_submodulemap if swift_submodule_replaces_modulemap else modulemap_path) or swift_interop_modulemap,
        "hmap": hmap_path,
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_exported_headers": transitive_exported_headers,
        "transitive_generated_headers": transitive_generated_headers,
        "transitive_framework_search_dirs": transitive_framework_search_dirs,
        "transitive_framework_files": transitive_framework_files,
        "transitive_vfs_overlays": transitive_vfs_overlays,
        "transitive_exported_header_dirs": transitive_exported_header_dirs,
        "transitive_modulemaps": transitive_modulemaps,
        "transitive_hmaps": transitive_hmaps,
        "transitive_archives": transitive_archives,
        "transitive_alwayslink_archives": transitive_alwayslink_archives,
        "transitive_sdk_frameworks": transitive_sdk_frameworks,
        "transitive_weak_sdk_frameworks": transitive_weak_sdk_frameworks,
        "transitive_sdk_dylibs": transitive_sdk_dylibs,
        "transitive_linkopts": transitive_linkopts,
        "transitive_plugin_dylibs": transitive_plugin_dylibs,
        "transitive_plugin_executables": transitive_plugin_executables,
        "transitive_defines": transitive_defines,
        "transitive_link_framework_bundles": transitive_link_framework_bundles,
        "transitive_framework_bundles": transitive_framework_bundles,
        "transitive_frameworks": _apple_framework_bundle_paths(transitive_framework_bundles),
        "transitive_resource_bundles": transitive_resource_bundles,
    }

def _apple_xcframework_platform(platform):
    if platform == "macos" or platform == "macosx":
        return "macos"
    if platform == "visionos" or platform == "xros":
        return "xros"
    return platform

def _apple_xcframework_module_name(modulemaps, fallback):
    for modulemap in modulemaps:
        for raw_line in host_file_read(workspace_root() + "/" + modulemap).split("\n"):
            parts = raw_line.strip().replace("{", " ").split()
            if len(parts) >= 2 and parts[0] == "module":
                return parts[1]
            if len(parts) >= 3 and parts[0] == "framework" and parts[1] == "module":
                return parts[2]
            if len(parts) >= 3 and parts[0] == "explicit" and parts[1] == "module":
                return parts[2]
    return fallback

def _apple_xcframework_static_library_name(path):
    name = _basename(path)
    if name.endswith(".a"):
        name = name[:len(name) - 2]
    if name.startswith("lib"):
        name = name[3:]
    return name

def _apple_xcframework_import_impl(ctx):
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["bundle"])
    bundle = attrs.get("bundle") or ""
    platform = attrs.get("platform") or ""
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    if not bundle or not platform:
        fail(ctx["label"]["id"] + ": bundle and platform are required")
    info = bundle + "/Info.plist"
    absolute_info = workspace_root() + "/" + info
    if not host_file_exists(absolute_info):
        fail(ctx["label"]["id"] + ": XCFramework bundle is missing Info.plist: `" + bundle + "`")
    plutil = host_which("plutil")
    data = json_decode(host_command([plutil, "-convert", "json", "-o", "-", absolute_info]))
    selected = None
    wanted_platform = _apple_xcframework_platform(platform)
    wanted_variant = "simulator" if sdk_variant == "simulator" and wanted_platform != "macos" else ""
    wanted_arch = attrs.get("arch") or host_arch()
    for library in data.get("AvailableLibraries") or []:
        if library.get("SupportedPlatform") != wanted_platform:
            continue
        if (library.get("SupportedPlatformVariant") or "") != wanted_variant:
            continue
        if wanted_arch not in (library.get("SupportedArchitectures") or []):
            continue
        selected = library
        break
    if selected == None:
        fail(ctx["label"]["id"] + ": XCFramework `" + bundle + "` has no " + wanted_platform + " " + wanted_variant + " slice for " + wanted_arch)
    library_path = selected.get("LibraryPath") or selected.get("BinaryPath") or ""
    if not library_path:
        fail(ctx["label"]["id"] + ": selected XCFramework slice has no library path")
    slice_path = bundle + "/" + selected["LibraryIdentifier"]
    library = slice_path + "/" + library_path
    binary_path = selected.get("BinaryPath") or ""
    if not binary_path:
        binary_path = library_path + "/" + _basename(library_path).replace(".framework", "") if library_path.endswith(".framework") else library_path
    binary = slice_path + "/" + binary_path
    absolute_slice = workspace_root() + "/" + slice_path
    files = [path[len(workspace_root()) + 1:] for path in host_command([host_which("find"), absolute_slice, "-type", "f"]).split("\n") if path]
    is_framework = library_path.endswith(".framework")
    expected_binary = binary if is_framework else library
    if not files or expected_binary not in files:
        fail(ctx["label"]["id"] + ": selected XCFramework slice is missing `" + library + "`")
    linkage = "static" if library_path.endswith(".a") or "current ar archive" in host_command([host_which("file"), workspace_root() + "/" + binary]) else "dynamic"
    if not is_framework:
        headers_path = selected.get("HeadersPath") or ""
        headers_dir = slice_path + "/" + headers_path if headers_path else ""
        modulemaps = []
        for path in files:
            if path.endswith(".modulemap") and (not headers_dir or path.startswith(headers_dir + "/")):
                modulemaps.append(path)
        if not modulemaps:
            for path in files:
                if path.endswith(".modulemap"):
                    modulemaps.append(path)
        module_name = attrs.get("module_name") or _apple_xcframework_module_name(modulemaps, _apple_xcframework_static_library_name(library_path))
        return {
            "label_id": ctx["label"]["id"],
            "framework_path": library,
            "framework_module_name": module_name,
            "framework_files": files,
            "transitive_exported_header_dirs": [headers_dir] if headers_dir else [],
            "transitive_modulemaps": modulemaps,
            "transitive_archives": [binary] if linkage == "static" else [],
            "transitive_framework_search_dirs": [],
            "transitive_framework_files": files,
            "transitive_link_framework_bundles": [],
            "transitive_framework_bundles": [],
            "transitive_frameworks": [],
            "transitive_sdk_frameworks": [],
            "transitive_weak_sdk_frameworks": [],
            "transitive_sdk_dylibs": [],
            "transitive_linkopts": [],
        }
    module_name = attrs.get("module_name") or _basename(library_path).replace(".framework", "")
    own_bundle = _apple_framework_bundle(library, module_name, files, ctx["label"]["id"])
    own_bundle["linkage"] = linkage
    return {
        "label_id": ctx["label"]["id"],
        "framework_path": library,
        "framework_module_name": module_name,
        "framework_files": files,
        "transitive_swiftmodule_dirs": [],
        "transitive_archives": [binary] if linkage == "static" else [],
        "transitive_link_framework_bundles": [own_bundle],
        "transitive_framework_bundles": [] if linkage == "static" else [own_bundle],
        "transitive_frameworks": [] if linkage == "static" else [library],
        "transitive_sdk_frameworks": [],
        "transitive_weak_sdk_frameworks": [],
        "transitive_sdk_dylibs": [],
        "transitive_linkopts": [],
    }

def _swift_macro_impl(ctx):
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["module_name"])
    minimum_os = attrs.get("minimum_os") or "13.0"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    module_name = attrs.get("module_name") or ctx["label"]["name"]
    swift_flags = attrs.get("swift_flags") or []

    all_srcs = _unique(glob(ctx["srcs"]) + _apple_declared_source_paths(ctx))
    swift_srcs = _filter_swift_sources(all_srcs)
    if len(swift_srcs) == 0:
        fail("swift_macro " + ctx["label"]["id"] + " has no Swift sources (.swift)")

    # Swift macros are host-loaded compiler plugins. They always build
    # for macOS in the simulator-equivalent SDK; macOS ignores the
    # variant anyway.
    swiftc = _resolve_swiftc("macos", "simulator", xcode_developer_dir)
    triple = _apple_triple("macos", minimum_os, "simulator", host_arch(), False)

    plugin_executable = declare_output(module_name + "-tool")
    plugin_swiftmodule = declare_output(module_name + ".swiftmodule")

    deps = _apple_native_deps(ctx)
    _validate_apple_native_deps(deps, ctx["label"]["id"])
    (
        dep_swiftmodule_dirs,
        dep_header_dirs,
        dep_modulemaps,
        dep_hmaps,
        dep_archives,
        dep_framework_search_dirs,
        dep_framework_module_names,
        dep_framework_files,
        dep_sdk_frameworks,
        dep_sdk_dylibs,
        dep_linkopts,
        dep_vfs_overlays,
        plugin_dylibs,
        plugin_executables,
    ) = _collect_dep_compile_inputs(deps, ctx["build_dir"])

    swift_argv = list(swiftc["argv"]) + [
        "-emit-executable",
        "-emit-module",
        "-module-name",
        module_name,
        "-emit-module-path",
        plugin_swiftmodule,
        "-target",
        triple,
        "-parse-as-library",
        "-o",
        plugin_executable,
    ]
    for d in dep_swiftmodule_dirs:
        swift_argv.extend(["-I", d])
    for hdir in dep_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for modulemap in dep_modulemaps:
        swift_argv.extend(["-Xcc", "-fmodule-map-file=" + modulemap])
    for hmap in dep_hmaps:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
    for overlay in dep_vfs_overlays:
        swift_argv.extend(["-Xcc", "-ivfsoverlay", "-Xcc", overlay])
    for framework_dir in dep_framework_search_dirs:
        swift_argv.extend(["-F", framework_dir])
    _apple_disable_static_framework_autolinking(swift_argv, _apple_collect_link_framework_bundles(deps))
    for framework in dep_framework_module_names:
        swift_argv.extend(["-framework", framework])
    for fw in dep_sdk_frameworks:
        swift_argv.extend(["-framework", fw])
    for dylib in dep_sdk_dylibs:
        swift_argv.extend(["-l" + dylib])
    _apple_add_swift_plugin_args(swift_argv, plugin_dylibs, plugin_executables)
    for flag in _apple_swift_link_flags(swift_flags):
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    # Dep archives appear as positional inputs; swiftc forwards
    # unknown-extension inputs to the linker.
    for ar in dep_archives:
        swift_argv.append(ar)
    for opt in dep_linkopts:
        swift_argv.append(opt)

    swift_inputs = list(swift_srcs)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)
    for modulemap in dep_modulemaps:
        if modulemap not in swift_inputs:
            swift_inputs.append(modulemap)
    for hmap in dep_hmaps:
        if hmap not in swift_inputs:
            swift_inputs.append(hmap)
    for file in dep_framework_files:
        if file not in swift_inputs:
            swift_inputs.append(file)
    for overlay in dep_vfs_overlays:
        if overlay not in swift_inputs:
            swift_inputs.append(overlay)
    for plugin_input in _apple_swift_plugin_inputs(plugin_dylibs, plugin_executables):
        if plugin_input not in swift_inputs:
            swift_inputs.append(plugin_input)
    for path in _apple_link_option_inputs(dep_linkopts):
        if path not in swift_inputs:
            swift_inputs.append(path)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [plugin_executable, plugin_swiftmodule],
        env = swiftc["env"],
        toolchain_identity = swiftc["identity"],
        identifier = "swift_macro_compile_" + module_name,
    )

    return {
        "label_id": ctx["label"]["id"],
        "plugin_executable": plugin_executable,
        "plugin_module_name": module_name,
        "transitive_plugin_executables": [plugin_executable + "#" + module_name],
    }

# --- Bundle helpers ----------------------------------------------------
#
# `_render_plist` emits a deterministic XML property list from a flat
# string-valued dict. Sufficient for the Info.plist payloads framework
# and application bundles need; richer types (arrays, bools) layer on
# through small per-key templates.

def _render_plist(entries, bool_entries = {}, array_entries = {}):
    lines = [
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">",
        "<plist version=\"1.0\">",
        "<dict>",
    ]
    for key in sorted(entries.keys()):
        lines.append("\t<key>" + key + "</key>")
        lines.append("\t<string>" + entries[key] + "</string>")
    for key in sorted(bool_entries.keys()):
        lines.append("\t<key>" + key + "</key>")
        lines.append("\t<true/>" if bool_entries[key] else "\t<false/>")
    for key in sorted(array_entries.keys()):
        lines.append("\t<key>" + key + "</key>")
        lines.append("\t<array>")
        for item in array_entries[key]:
            # The dict shape signals an integer (e.g. UIDeviceFamily); a
            # bare string is wrapped as a <string>.
            if type(item) == "dict" and "integer" in item:
                lines.append("\t\t<integer>" + str(item["integer"]) + "</integer>")
            else:
                lines.append("\t\t<string>" + str(item) + "</string>")
        lines.append("\t</array>")
    lines.append("</dict>")
    lines.append("</plist>")
    lines.append("")
    return "\n".join(lines)

def _apple_entitlements_with_application_identifier(contents, development_team, bundle_id):
    if not development_team or not bundle_id or "<key>application-identifier</key>" in contents:
        return contents
    closing = contents.find("</dict>")
    if closing < 0:
        return contents
    entry = "\t<key>application-identifier</key>\n\t<string>" + development_team + "." + bundle_id + "</string>\n"
    return contents[:closing] + entry + contents[closing:]

def _apple_resource_path(ctx, path):
    if not path or path.startswith("/") or path.startswith("."):
        return path
    package = ctx["label"]["package"]
    if package and (path == package or path.startswith(package + "/")):
        return path
    return _package_relative(ctx, path)

def _apple_workspace_relative(path):
    prefix = workspace_root() + "/"
    return path[len(prefix):] if path.startswith(prefix) else path

def _apple_is_directory(path):
    absolute = path if path.startswith("/") else workspace_root() + "/" + path
    if not host_path_exists(absolute):
        return False
    return bool(host_command([host_which("find"), absolute, "-maxdepth", "0", "-type", "d"]).strip())

def _apple_resource_tree_files(path):
    files = [file for file in glob([path + "/**"]) if file != path]
    if files:
        return files
    absolute = path if path.startswith("/") else workspace_root() + "/" + path
    if not _apple_is_directory(path):
        return []
    return [_apple_workspace_relative(file) for file in host_command([host_which("find"), absolute, "-type", "f"]).split("\n") if file]

def _apple_resource_models(path):
    if path.endswith(".xcdatamodeld") and _apple_is_directory(path):
        return [path]
    if not _apple_is_directory(path):
        return []
    absolute = path if path.startswith("/") else workspace_root() + "/" + path
    return [_apple_workspace_relative(model) for model in host_command([host_which("find"), absolute, "-type", "d", "-name", "*.xcdatamodeld"]).split("\n") if model]

def _apple_resource_output_prefix(source, resource_root, preserve_root):
    if _apple_is_directory(resource_root):
        rel = source[len(resource_root) + 1:] if source.startswith(resource_root + "/") else _basename(source)
        if preserve_root or resource_root.endswith(".bundle") or resource_root.endswith(".lproj"):
            return _basename(resource_root) + "/" + rel
        return rel
    parent = _basename(_parent_dir(source))
    if parent.endswith(".lproj"):
        return parent + "/" + _basename(source)
    return _basename(source)

def _apple_materialize_resources(ctx, raw_resources, destination, platform, minimum_os, xcode_developer_dir, module_name, identifier_prefix, raw_structured_resources = []):
    resources = []
    for raw in raw_resources:
        resolved = _apple_resource_path(ctx, raw)
        if "*" in resolved or "?" in resolved or "[" in resolved:
            resources.extend(glob([resolved]))
        elif resolved:
            resources.append(resolved)
    resources = _unique(resources)
    structured_resources = {}
    for raw in raw_structured_resources:
        resolved = _apple_resource_path(ctx, raw)
        if resolved:
            structured_resources[resolved] = True
    if not resources:
        return []

    models = []
    for resource in resources:
        for model in _apple_resource_models(resource):
            if model not in models:
                models.append(model)

    output_by_path = {}
    outputs = []

    def declare_resource_output(relative, source):
        output = declare_output(destination + "/" + relative)
        previous = output_by_path.get(output)
        if previous and previous != source:
            fail(ctx["label"]["id"] + ": resource output collision for `" + relative + "` between `" + previous + "` and `" + source + "`")
        output_by_path[output] = source
        if output not in outputs:
            outputs.append(output)
        return output

    def belongs_to_model(path):
        for model in models:
            if path == model or path.startswith(model + "/"):
                return True
        return False

    ibtool = None
    for resource in resources:
        source_files = _apple_resource_tree_files(resource) if _apple_is_directory(resource) else [resource]
        for source in source_files:
            if belongs_to_model(source):
                continue
            relative = _apple_resource_output_prefix(source, resource, resource in structured_resources)
            if source.endswith(".xib") or source.endswith(".storyboard"):
                if ibtool == None:
                    ibtool = _resolve_ibtool(xcode_developer_dir)
                source_suffix = ".storyboard" if source.endswith(".storyboard") else ".xib"
                output_suffix = ".storyboardc" if source.endswith(".storyboard") else ".nib"
                relative = relative[:len(relative) - len(source_suffix)] + output_suffix
                output = declare_resource_output(relative, source)
                argv = [
                    ibtool["path"],
                    "--errors",
                    "--warnings",
                    "--notices",
                    "--module",
                    module_name,
                ]
                if platform == "ios":
                    argv.extend(["--target-device", "iphone", "--target-device", "ipad"])
                elif platform == "tvos":
                    argv.extend(["--target-device", "tv"])
                elif platform == "watchos":
                    argv.extend(["--target-device", "watch"])
                argv.extend([
                    "--minimum-deployment-target",
                    minimum_os,
                    "--output-format",
                    "human-readable-text",
                    "--compile",
                    output,
                    source,
                ])
                run_action(
                    argv = argv,
                    inputs = [source],
                    outputs = [output],
                    create_dirs = [_parent_dir(output)],
                    env = ibtool["env"],
                    toolchain_identity = ibtool["identity"],
                    identifier = identifier_prefix + "_interface_" + _basename(source),
                )
                continue
            output = declare_resource_output(relative, source)
            copy_path(
                source,
                output,
                inputs = [source],
                identifier = identifier_prefix + "_copy_" + relative.replace("/", "_"),
            )

    if models:
        momc = _resolve_momc(xcode_developer_dir)
        swiftc = _resolve_swiftc(platform, "simulator" if platform == "macos" else (ctx["attr"].get("sdk_variant") or "simulator"), xcode_developer_dir)
        destination_path = ctx["build_dir"] + "/" + destination
        for model in models:
            model_name = _basename(model)[:len(_basename(model)) - len(".xcdatamodeld")]
            output = declare_resource_output(model_name + ".momd", model)
            model_inputs = _apple_resource_tree_files(model)
            run_action(
                argv = [
                    momc["path"],
                    "--sdkroot",
                    swiftc["sdk_path"],
                    "--" + swiftc["sdk_name"] + "-deployment-target",
                    minimum_os,
                    "--module",
                    module_name,
                    model,
                    destination_path,
                ],
                inputs = model_inputs,
                outputs = [output],
                create_dirs = [destination_path],
                env = momc["env"],
                toolchain_identity = momc["identity"],
                identifier = identifier_prefix + "_model_" + model_name,
            )
    return outputs

def _apple_create_resource_bundle(ctx, resources, structured_resources, bundle_name, bundle_id, platform, minimum_os, xcode_developer_dir, module_name):
    name = bundle_name if bundle_name.endswith(".bundle") else bundle_name + ".bundle"
    files = _apple_materialize_resources(
        ctx,
        resources,
        name,
        platform,
        minimum_os,
        xcode_developer_dir,
        module_name,
        "apple_resource_bundle_" + module_name,
        structured_resources,
    )
    info = declare_output(name + "/Info.plist")
    if info in files:
        fail(ctx["label"]["id"] + ": resource bundle `" + name + "` supplies an Info.plist that conflicts with generated bundle metadata")
    write_path(info, _render_plist({
        "CFBundleDevelopmentRegion": "en",
        "CFBundleIdentifier": _xml_escape(bundle_id),
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": _xml_escape(name[:len(name) - len(".bundle")]),
        "CFBundlePackageType": "BNDL",
        "MinimumOSVersion": minimum_os,
        "DTPlatformName": _apple_sdk_name(platform, ctx["attr"].get("sdk_variant") or "simulator"),
    }, {}, {
        "CFBundleSupportedPlatforms": [_apple_supported_platform(_apple_sdk_name(platform, ctx["attr"].get("sdk_variant") or "simulator"))],
    }))
    files.append(info)
    return _apple_resource_bundle(ctx["build_dir"] + "/" + name, files, ctx["label"]["id"])

def _apple_resource_bundle_target_impl(ctx):
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["bundle_name"])
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    bundle_name = attrs.get("bundle_name") or ctx["label"]["name"]
    module_name = _apple_swift_module_name(bundle_name)
    own_bundle = _apple_create_resource_bundle(
        ctx,
        attrs.get("resources") or [],
        attrs.get("structured_resources") or [],
        bundle_name,
        attrs.get("bundle_id") or ("dev.once." + module_name + ".resources"),
        platform,
        minimum_os,
        attrs.get("xcode_developer_dir") or "",
        module_name,
    )
    return {
        "label_id": ctx["label"]["id"],
        "transitive_resource_bundles": _apple_collect_resource_bundles(_apple_native_deps(ctx), [own_bundle]),
    }

def _xml_escape(value):
    return value.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace("\"", "&quot;").replace("'", "&apos;")

def _apple_hosted_xctestrun(module_name, test_bundle_path, host_application, xctest_framework_dir, xctest_usr_lib_dir, test_env = {}, test_arguments = [], skipped_tests = []):
    host_path = workspace_root() + "/" + host_application["app_path"]
    host_executable = workspace_root() + "/" + host_application["app_executable"]
    bundle_path = workspace_root() + "/" + test_bundle_path
    values = {
        "module": _xml_escape(module_name),
        "bundle": _xml_escape(bundle_path),
        "host": _xml_escape(host_path),
        "host_executable": _xml_escape(host_executable),
        "bundle_id": _xml_escape(host_application.get("bundle_id") or ""),
        "frameworks": _xml_escape(xctest_framework_dir),
        "libraries": _xml_escape(xctest_usr_lib_dir),
        "inject": _xml_escape(xctest_usr_lib_dir + "/libXCTestBundleInject.dylib"),
        "products": _xml_escape(workspace_root() + "/.once/out"),
    }
    environment_entries = []
    for key in sorted(test_env.keys()):
        environment_entries.append("      <key>" + _xml_escape(key) + "</key><string>" + _xml_escape(test_env[key]) + "</string>")
    values["test_environment"] = "\n".join(environment_entries)
    command_line_arguments = list(test_arguments)
    for key in ["AppleLanguages", "AppleLocale"]:
        if test_env.get(key):
            command_line_arguments.extend(["-" + key, test_env[key]])
    values["command_line_arguments"] = "".join(["<string>" + _xml_escape(argument) + "</string>" for argument in command_line_arguments])
    values["skipped_tests"] = "".join(["<string>" + _xml_escape(test) + "</string>" for test in skipped_tests])
    return """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>{module}</key>
  <dict>
    <key>ProductModuleName</key><string>{module}</string>
    <key>TestBundlePath</key><string>{bundle}</string>
    <key>TestExecutionOrdering</key><string>ordered</string>
    <key>IsAppHostedTestBundle</key><true/>
    <key>TestHostPath</key><string>{host}</string>
    <key>TestHostBundleIdentifier</key><string>{bundle_id}</string>
    <key>IsUITestBundle</key><false/>
    <key>IsXCTRunnerHostedTestBundle</key><false/>
    <key>CommandLineArguments</key><array>{command_line_arguments}</array>
    <key>SkipTestIdentifiers</key><array>{skipped_tests}</array>
    <key>DependentProductPaths</key>
    <array><string>{bundle}</string><string>{host}</string></array>
    <key>TestingEnvironmentVariables</key>
    <dict>
      <key>DYLD_INSERT_LIBRARIES</key><string>{inject}</string>
      <key>DYLD_LIBRARY_PATH</key><string>{libraries}</string>
      <key>DYLD_FRAMEWORK_PATH</key><string>{frameworks}</string>
      <key>XCInjectBundleInto</key><string>{host_executable}</string>
      <key>__XCODE_BUILT_PRODUCTS_DIR_PATHS</key><string>{products}</string>
{test_environment}
    </dict>
  </dict>
  <key>__xctestrun_metadata__</key>
  <dict><key>FormatVersion</key><integer>1</integer></dict>
</dict>
</plist>
""".format(**values)

def _apple_ui_xctestrun(module_name, test_bundle_path, runner_application_path, runner_bundle_id, target_application, xctest_framework_dir, xctest_usr_lib_dir, test_env = {}, test_arguments = [], skipped_tests = []):
    bundle_path = workspace_root() + "/" + test_bundle_path
    runner_path = workspace_root() + "/" + runner_application_path
    target_path = workspace_root() + "/" + target_application["app_path"]
    command_line_arguments = list(test_arguments)
    target_arguments = []
    for key in ["AppleLanguages", "AppleLocale"]:
        if test_env.get(key):
            command_line_arguments.extend(["-" + key, test_env[key]])
            target_arguments.extend(["-" + key, test_env[key]])
    environment_entries = []
    for key in sorted(test_env.keys()):
        environment_entries.append("      <key>" + _xml_escape(key) + "</key><string>" + _xml_escape(test_env[key]) + "</string>")
    values = {
        "module": _xml_escape(module_name),
        "bundle": _xml_escape(bundle_path),
        "runner": _xml_escape(runner_path),
        "runner_bundle_id": _xml_escape(runner_bundle_id),
        "target": _xml_escape(target_path),
        "frameworks": _xml_escape(xctest_framework_dir),
        "libraries": _xml_escape(xctest_usr_lib_dir),
        "products": _xml_escape(workspace_root() + "/.once/out"),
        "command_line_arguments": "".join(["<string>" + _xml_escape(argument) + "</string>" for argument in command_line_arguments]),
        "target_arguments": "".join(["<string>" + _xml_escape(argument) + "</string>" for argument in target_arguments]),
        "skipped_tests": "".join(["<string>" + _xml_escape(test) + "</string>" for test in skipped_tests]),
        "test_environment": "\n".join(environment_entries),
    }
    return """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>{module}</key>
  <dict>
    <key>ProductModuleName</key><string>{module}</string>
    <key>TestBundlePath</key><string>{bundle}</string>
    <key>TestExecutionOrdering</key><string>ordered</string>
    <key>TestHostPath</key><string>{runner}</string>
    <key>TestHostBundleIdentifier</key><string>{runner_bundle_id}</string>
    <key>IsUITestBundle</key><true/>
    <key>IsXCTRunnerHostedTestBundle</key><true/>
    <key>UITargetAppPath</key><string>{target}</string>
    <key>CommandLineArguments</key><array>{command_line_arguments}</array>
    <key>UITargetAppCommandLineArguments</key><array>{target_arguments}</array>
    <key>SkipTestIdentifiers</key><array>{skipped_tests}</array>
    <key>DependentProductPaths</key>
    <array><string>{bundle}</string><string>{target}</string><string>{runner}</string></array>
    <key>TestingEnvironmentVariables</key>
    <dict>
      <key>DYLD_LIBRARY_PATH</key><string>{libraries}</string>
      <key>DYLD_FRAMEWORK_PATH</key><string>{frameworks}</string>
      <key>__XCODE_BUILT_PRODUCTS_DIR_PATHS</key><string>{products}</string>
{test_environment}
    </dict>
  </dict>
  <key>__xctestrun_metadata__</key>
  <dict><key>FormatVersion</key><integer>1</integer></dict>
</dict>
</plist>
""".format(**values)

def _apple_supported_platform(sdk_name):
    names = {
        "macosx": "MacOSX",
        "iphoneos": "iPhoneOS",
        "iphonesimulator": "iPhoneSimulator",
        "appletvos": "AppleTVOS",
        "appletvsimulator": "AppleTVSimulator",
        "watchos": "WatchOS",
        "watchsimulator": "WatchSimulator",
        "xros": "XROS",
        "xrsimulator": "XRSimulator",
    }
    name = names.get(sdk_name)
    if not name:
        fail("no supported-platform property-list value for Apple SDK `" + sdk_name + "`")
    return name

def _apple_collect_swift_plugins(deps):
    dylibs = []
    executables = []
    for dep in deps:
        for dylib in dep.get("transitive_plugin_dylibs") or []:
            if dylib and dylib not in dylibs:
                dylibs.append(dylib)
        dylib = dep.get("plugin_dylib")
        if dylib and dylib not in dylibs:
            dylibs.append(dylib)
        for descriptor in dep.get("transitive_plugin_executables") or []:
            if descriptor and descriptor not in executables:
                executables.append(descriptor)
        executable = dep.get("plugin_executable") or ""
        module_name = dep.get("plugin_module_name") or ""
        if executable and module_name:
            descriptor = executable + "#" + module_name
            if descriptor not in executables:
                executables.append(descriptor)
    return dylibs, executables

def _apple_add_swift_plugin_args(argv, plugin_dylibs, plugin_executables):
    for dylib in plugin_dylibs:
        argv.extend(["-load-plugin-library", dylib])
    for descriptor in plugin_executables:
        argv.extend(["-Xfrontend", "-load-plugin-executable", "-Xfrontend", descriptor])

def _apple_swift_plugin_inputs(plugin_dylibs, plugin_executables):
    inputs = list(plugin_dylibs)
    for descriptor in plugin_executables:
        path = descriptor.split("#")[0]
        if path and path not in inputs:
            inputs.append(path)
    return inputs

def _collect_dep_compile_inputs(deps, build_dir):
    """Aggregate compile-visible inputs from dep providers.

    Returns (swiftmodule_dirs, header_dirs, modulemaps, hmaps, archives,
    framework_search_dirs, framework_module_names, framework_files, sdk_frameworks,
    sdk_dylibs, linkopts, plugin_dylibs, plugin_executables).
    """
    swiftmodule_dirs = []
    header_dirs = []
    modulemaps = []
    hmaps = []
    archives = []
    framework_search_dirs = []
    framework_module_names = []
    framework_files = []
    sdk_frameworks = []
    sdk_dylibs = []
    linkopts = []
    vfs_overlays = []
    plugin_dylibs, plugin_executables = _apple_collect_swift_plugins(deps)
    for dep in deps:
        for d in dep.get("transitive_swiftmodule_dirs") or []:
            if d and d != build_dir and d not in swiftmodule_dirs:
                swiftmodule_dirs.append(d)
        for h in dep.get("transitive_exported_header_dirs") or []:
            if h and h not in header_dirs:
                header_dirs.append(h)
        for m in dep.get("transitive_modulemaps") or []:
            if m and m not in modulemaps:
                modulemaps.append(m)
        for h in dep.get("transitive_hmaps") or []:
            if h and h not in hmaps:
                hmaps.append(h)
        for ar in dep.get("transitive_archives") or []:
            if ar and ar not in archives:
                archives.append(ar)
        for bundle in _apple_dep_framework_bundles(dep, "transitive_link_framework_bundles", False):
            framework_path = bundle.get("path") or ""
            for f in _apple_framework_compile_files(bundle):
                if f and f not in framework_files:
                    framework_files.append(f)
            framework_parent = _parent_dir(framework_path)
            if framework_parent and framework_parent not in framework_search_dirs:
                framework_search_dirs.append(framework_parent)
            module_name = bundle.get("module_name") or ""
            if bundle.get("linkage") != "static" and module_name and module_name not in framework_module_names:
                framework_module_names.append(module_name)
        for bundle in _apple_dep_framework_bundles(dep, "transitive_framework_bundles", True):
            framework_path = bundle.get("path") or ""
            framework_parent = _parent_dir(framework_path)
            if framework_parent and framework_parent not in framework_search_dirs:
                framework_search_dirs.append(framework_parent)
            for f in _apple_framework_compile_files(bundle):
                if f and f not in framework_files:
                    framework_files.append(f)
        for f in dep.get("transitive_generated_headers") or []:
            if f and f not in framework_files:
                framework_files.append(f)
        for f in dep.get("transitive_exported_headers") or []:
            if f and f not in framework_files:
                framework_files.append(f)
        # Framework search directories contributed without a specific framework
        # bundle. Swift autolinks imported frameworks, so a consumer only needs
        # the search path on the compile and link lines; the framework itself is
        # embedded separately. Swift Package Manager dependency sets use this to
        # expose a directory of built frameworks (binary package products).
        for d in dep.get("transitive_framework_search_dirs") or []:
            if d and d not in framework_search_dirs:
                framework_search_dirs.append(d)
        for f in dep.get("transitive_framework_files") or []:
            if f and f not in framework_files:
                framework_files.append(f)
        for fw in dep.get("transitive_sdk_frameworks") or []:
            if fw and fw not in sdk_frameworks:
                sdk_frameworks.append(fw)
        for dy in dep.get("transitive_sdk_dylibs") or []:
            if dy and dy not in sdk_dylibs:
                sdk_dylibs.append(dy)
        for opt in dep.get("transitive_linkopts") or []:
            if opt:
                linkopts.append(opt)
        for overlay in dep.get("transitive_vfs_overlays") or []:
            if overlay and overlay not in vfs_overlays:
                vfs_overlays.append(overlay)
    return (
        swiftmodule_dirs,
        header_dirs,
        modulemaps,
        hmaps,
        archives,
        framework_search_dirs,
        framework_module_names,
        framework_files,
        sdk_frameworks,
        sdk_dylibs,
        linkopts,
        vfs_overlays,
        plugin_dylibs,
        plugin_executables,
    )

def _apple_mixed_framework_impl(ctx):
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["product_name", "module_name"])
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    product_name = attrs.get("product_name") or ctx["label"]["name"]
    module_name = _apple_swift_module_name(attrs.get("module_name") or product_name)
    bundle_id = attrs.get("bundle_id") or ("dev.once." + product_name)
    resources = attrs.get("resources") or []
    structured_resources = attrs.get("structured_resources") or []

    library_attrs = dict(ctx["attr"])
    library_attrs["resources"] = []
    library_attrs["structured_resources"] = []
    library_attrs["resource_bundle_name"] = ""
    library_attrs["resource_bundle_id"] = ""
    library_ctx = dict(ctx)
    library_ctx["attr"] = library_attrs
    framework_deps = _apple_native_deps(ctx)
    library_ctx["deps"] = framework_deps
    library = _apple_library_impl(library_ctx)
    (
        dep_swiftmodule_dirs,
        dep_header_dirs,
        dep_modulemaps,
        dep_hmaps,
        _dep_archives,
        dep_framework_search_dirs,
        _dep_framework_module_names,
        dep_framework_files,
        _dep_sdk_frameworks,
        _dep_sdk_dylibs,
        _dep_linkopts,
        dep_vfs_overlays,
        _dep_plugin_dylibs,
        _dep_plugin_executables,
    ) = _collect_dep_compile_inputs(framework_deps, ctx["build_dir"])

    framework_dir = product_name + ".framework"
    framework_path = ctx["build_dir"] + "/" + framework_dir
    dylib = declare_output(framework_dir + "/" + product_name)
    info_plist = declare_output(framework_dir + "/Info.plist")
    framework_files = [dylib, info_plist]

    swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant, host_arch(), attrs.get("mac_catalyst") or False)
    link_argv = list(swiftc["argv"]) + [
        "-emit-library",
        "-module-name",
        module_name,
        "-target",
        triple,
        "-Xlinker",
        "-install_name",
        "-Xlinker",
        "@rpath/" + framework_dir + "/" + product_name,
        "-o",
        dylib,
    ]
    link_anchor = declare_output(module_name + "-framework-link-anchor.swift")
    write_path(link_anchor, "")
    framework_bundles = library.get("transitive_link_framework_bundles") or []
    framework_search_dirs = []
    for bundle in framework_bundles:
        if bundle.get("linkage") != "static":
            search_dir = _parent_dir(bundle["path"])
            if search_dir and search_dir not in framework_search_dirs:
                framework_search_dirs.append(search_dir)
                link_argv.extend(["-F", search_dir])
            link_argv.extend(["-framework", bundle["module_name"]])
    for search_dir in dep_framework_search_dirs:
        if search_dir and search_dir not in framework_search_dirs:
            framework_search_dirs.append(search_dir)
            link_argv.extend(["-F", search_dir])
    for framework in library.get("transitive_sdk_frameworks") or []:
        link_argv.extend(["-framework", framework])
    for framework in library.get("transitive_weak_sdk_frameworks") or []:
        _apple_append_weak_framework(link_argv, framework)
    for dylib_name in library.get("transitive_sdk_dylibs") or []:
        link_argv.append("-l" + dylib_name)
    for option in library.get("transitive_linkopts") or []:
        link_argv.append(option)
    archives = library.get("transitive_archives") or []
    alwayslink_archives = _apple_collect_alwayslink_archives(framework_deps)
    own_archive = library.get("archive") or ""
    if own_archive and own_archive not in alwayslink_archives:
        alwayslink_archives.append(own_archive)
    _apple_append_archives(link_argv, archives, alwayslink_archives)
    link_argv.append(link_anchor)

    link_inputs = [link_anchor] + list(archives)
    for bundle in framework_bundles:
        for file in bundle.get("files") or []:
            if file not in link_inputs:
                link_inputs.append(file)
    for file in dep_framework_files:
        if file not in link_inputs:
            link_inputs.append(file)
    for path in _apple_link_option_inputs(library.get("transitive_linkopts") or []):
        if path not in link_inputs:
            link_inputs.append(path)
    run_action(
        argv = link_argv,
        inputs = link_inputs,
        outputs = [dylib],
        env = swiftc["env"],
        toolchain_identity = swiftc["identity"],
        identifier = "apple_framework_link_" + module_name,
    )

    modulemap_source = library.get("modulemap") or ""
    if modulemap_source:
        modulemap = declare_output(framework_dir + "/Modules/module.modulemap")
        if modulemap_source != modulemap:
            copy_path(modulemap_source, modulemap, inputs = [modulemap_source], identifier = "apple_framework_modulemap_" + module_name)
        framework_files.append(modulemap)

    all_srcs = _unique(glob(ctx["srcs"]) + _apple_declared_source_paths(ctx))
    if _filter_swift_sources(all_srcs):
        archs = attrs.get("archs") or [host_arch()]
        is_universal = len(archs) > 1
        for arch in archs:
            if is_universal:
                swiftmodule_source = ctx["build_dir"] + "/" + module_name + ".swiftmodule/" + arch + ".swiftmodule"
                swiftdoc_source = ctx["build_dir"] + "/" + module_name + ".swiftmodule/" + arch + ".swiftdoc"
            else:
                swiftmodule_source = ctx["build_dir"] + "/" + module_name + ".swiftmodule"
                swiftdoc_source = ctx["build_dir"] + "/" + module_name + ".swiftdoc"
            module_triple = _apple_swiftmodule_triple(platform, sdk_variant, arch, attrs.get("mac_catalyst") or False)
            module_dir = framework_dir + "/Modules/" + module_name + ".swiftmodule"
            swiftmodule = declare_output(module_dir + "/" + module_triple + ".swiftmodule")
            swiftdoc = declare_output(module_dir + "/" + module_triple + ".swiftdoc")
            copy_path(swiftmodule_source, swiftmodule, inputs = [swiftmodule_source], identifier = "apple_framework_swiftmodule_" + module_name + "_" + arch)
            copy_path(swiftdoc_source, swiftdoc, inputs = [swiftdoc_source], identifier = "apple_framework_swiftdoc_" + module_name + "_" + arch)
            framework_files.extend([swiftmodule, swiftdoc])

    headers = list(library.get("exported_headers") or [])
    if library.get("objc_header"):
        headers.append(library["objc_header"])
    seen_header_names = {}
    for header in headers:
        header_name = _basename(header)
        previous = seen_header_names.get(header_name)
        if previous and previous != header:
            fail(ctx["label"]["id"] + ": framework header collision for `" + header_name + "` between `" + previous + "` and `" + header + "`")
        seen_header_names[header_name] = header
        output = declare_output(framework_dir + "/Headers/" + header_name)
        if header != output:
            copy_path(header, output, inputs = [header], identifier = "apple_framework_header_" + module_name + "_" + header_name)
        framework_files.append(output)

    resource_files = _apple_materialize_resources(
        ctx,
        resources,
        framework_dir,
        platform,
        minimum_os,
        xcode_developer_dir,
        module_name,
        "apple_framework_resource_" + module_name,
        structured_resources,
    )
    framework_files.extend(resource_files)

    asset_catalogs = [_package_relative(ctx, catalog) for catalog in (attrs.get("asset_catalogs") or [])]
    if asset_catalogs:
        actool = _resolve_actool(xcode_developer_dir)
        asset_car = declare_output(framework_dir + "/Assets.car")
        asset_partial_plist = declare_output(framework_dir + "/assetcatalog-info.plist")
        run_action(
            argv = [actool["actool_path"]] + asset_catalogs + [
                "--compile",
                framework_path,
                "--platform",
                _apple_actool_platform(platform, sdk_variant),
                "--minimum-deployment-target",
                minimum_os,
                "--bundle-identifier",
                bundle_id,
                "--output-partial-info-plist",
                asset_partial_plist,
            ],
            inputs = asset_catalogs,
            outputs = [asset_car, asset_partial_plist],
            create_dirs = [framework_path],
            env = actool["env"],
            toolchain_identity = actool["identity"],
            identifier = "apple_framework_assets_" + module_name,
        )
        framework_files.extend([asset_car, asset_partial_plist])

    privacy_manifest = attrs.get("privacy_manifest") or ""
    if privacy_manifest:
        privacy_source = _package_relative(ctx, privacy_manifest)
        privacy_output = declare_output(framework_dir + "/PrivacyInfo.xcprivacy")
        copy_path(privacy_source, privacy_output, inputs = [privacy_source], identifier = "apple_framework_privacy_" + module_name)
        framework_files.append(privacy_output)

    write_path(info_plist, _render_plist({
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": product_name,
        "CFBundleIdentifier": bundle_id,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": product_name,
        "CFBundlePackageType": "FMWK",
        "CFBundleShortVersionString": "1.0",
        "CFBundleVersion": "1",
        "MinimumOSVersion": minimum_os,
        "DTPlatformName": _apple_sdk_name(platform, sdk_variant),
    }, {}, {
        "CFBundleSupportedPlatforms": [_apple_supported_platform(_apple_sdk_name(platform, sdk_variant))],
    }))

    codesign = _resolve_codesign(xcode_developer_dir)
    cs_stamp = declare_output(framework_dir + "/_CodeSignature/CodeResources")
    run_action(
        argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none", framework_path],
        inputs = framework_files,
        outputs = [dylib, cs_stamp],
        env = codesign["env"],
        toolchain_identity = codesign["identity"],
        identifier = "apple_framework_codesign_" + module_name,
    )
    framework_files.append(cs_stamp)

    absorbed_static_archives = archives
    own_bundle = _apple_framework_bundle(framework_path, module_name, framework_files, ctx["label"]["id"], absorbed_static_archives)
    transitive_runtime_frameworks = _apple_collect_runtime_framework_bundles([library], [own_bundle])
    transitive_swiftmodule_dirs = dep_swiftmodule_dirs
    return {
        "label_id": ctx["label"]["id"],
        "framework_path": framework_path,
        "framework_module_name": module_name,
        "framework_files": framework_files,
        "swiftmodule_dir": framework_path + "/Modules",
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_exported_header_dirs": _unique(dep_header_dirs + (library.get("transitive_exported_header_dirs") or [])),
        "transitive_exported_headers": library.get("transitive_exported_headers") or [],
        "transitive_generated_headers": library.get("transitive_generated_headers") or [],
        "transitive_modulemaps": _unique(dep_modulemaps + (library.get("transitive_modulemaps") or [])),
        "transitive_hmaps": dep_hmaps,
        "transitive_framework_search_dirs": dep_framework_search_dirs,
        "transitive_framework_files": dep_framework_files,
        "transitive_vfs_overlays": library.get("transitive_vfs_overlays") or [],
        "transitive_archives": [],
        "absorbed_static_archives": absorbed_static_archives,
        "transitive_link_framework_bundles": [own_bundle],
        "transitive_framework_bundles": transitive_runtime_frameworks,
        "transitive_frameworks": _apple_framework_bundle_paths(transitive_runtime_frameworks),
        "transitive_sdk_frameworks": library.get("transitive_sdk_frameworks") or [],
        "transitive_weak_sdk_frameworks": library.get("transitive_weak_sdk_frameworks") or [],
        "transitive_sdk_dylibs": library.get("transitive_sdk_dylibs") or [],
        "transitive_linkopts": [],
        "transitive_plugin_dylibs": library.get("transitive_plugin_dylibs") or [],
        "transitive_plugin_executables": library.get("transitive_plugin_executables") or [],
        "transitive_resource_bundles": library.get("transitive_resource_bundles") or [],
    }

def _apple_framework_impl(ctx):
    framework_sources = _unique(glob(ctx["srcs"]) + _apple_declared_source_paths(ctx))
    framework_attrs = ctx["attr"]
    if _filter_objc_sources(framework_sources) or _filter_c_sources(framework_sources) or _filter_cxx_sources(framework_sources) or framework_attrs.get("resources") or framework_attrs.get("asset_catalogs") or framework_attrs.get("privacy_manifest") or framework_attrs.get("modulemap") or framework_attrs.get("bridging_header"):
        return _apple_mixed_framework_impl(ctx)
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["product_name"])
    _reject_unsupported_attrs(attrs, ctx["label"]["id"], ["headers", "exported_headers", "resources", "asset_catalogs", "privacy_manifest"])
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    product_name = attrs.get("product_name") or ctx["label"]["name"]
    module_name = attrs.get("module_name") or product_name
    bundle_id = attrs.get("bundle_id") or ("dev.once." + product_name)
    sdk_frameworks_attr = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs_attr = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    swift_flags = attrs.get("swift_flags") or []

    all_srcs = glob(ctx["srcs"])
    swift_srcs = _filter_swift_sources(all_srcs)
    if len(swift_srcs) == 0:
        fail("apple_framework " + ctx["label"]["id"] + " has no Swift sources (.swift)")

    # MVP: single host architecture. Multi-arch fan-out for frameworks
    # lands in a follow-up; today the same machinery as `apple_library`
    # can be wired in but the demo path doesn't need it.
    arch = host_arch()
    swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant, arch, False)

    framework_dir = product_name + ".framework"
    dylib = declare_output(framework_dir + "/" + product_name)
    module_dir = framework_dir + "/Modules/" + module_name + ".swiftmodule"
    module_triple = _apple_swiftmodule_triple(platform, sdk_variant, arch, False)
    swiftmodule = declare_output(module_dir + "/" + module_triple + ".swiftmodule")
    swiftdoc = declare_output(module_dir + "/" + module_triple + ".swiftdoc")
    modulemap = declare_output(framework_dir + "/Modules/module.modulemap")
    info_plist = declare_output(framework_dir + "/Info.plist")

    deps = _apple_native_deps(ctx)
    _validate_apple_native_deps(deps, ctx["label"]["id"])
    (
        compile_swiftmodule_dirs,
        compile_header_dirs,
        dep_modulemaps,
        dep_hmaps,
        dep_archives,
        framework_search_dirs,
        framework_module_names,
        dep_framework_files,
        dep_sdk_frameworks,
        dep_sdk_dylibs,
        dep_linkopts,
        dep_vfs_overlays,
        plugin_dylibs,
        plugin_executables,
    ) = _collect_dep_compile_inputs(deps, ctx["build_dir"])
    alwayslink_archives = _apple_collect_alwayslink_archives(deps)
    runtime_framework_bundles = _apple_collect_runtime_framework_bundles(deps)

    swift_argv = list(swiftc["argv"]) + [
        "-emit-library",
        "-emit-module",
        "-module-name",
        module_name,
        "-emit-module-path",
        swiftmodule,
        "-target",
        triple,
        "-parse-as-library",
        "-Xlinker",
        "-install_name",
        "-Xlinker",
        "@rpath/" + framework_dir + "/" + product_name,
        "-o",
        dylib,
    ]
    for d in compile_swiftmodule_dirs:
        swift_argv.extend(["-I", d])
    for hdir in compile_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for mmap in dep_modulemaps:
        swift_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
    for hmap in dep_hmaps:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
    for overlay in dep_vfs_overlays:
        swift_argv.extend(["-Xcc", "-ivfsoverlay", "-Xcc", overlay])
    for d in framework_search_dirs:
        swift_argv.extend(["-F", d])
    _apple_disable_static_framework_autolinking(swift_argv, _apple_collect_link_framework_bundles(deps))
    for fw in framework_module_names:
        swift_argv.extend(["-framework", fw])
    for fw in sdk_frameworks_attr:
        swift_argv.extend(["-framework", fw])
    for fw in dep_sdk_frameworks:
        if fw not in sdk_frameworks_attr:
            swift_argv.extend(["-framework", fw])
    for fw in weak_sdk_frameworks:
        _apple_append_weak_framework(swift_argv, fw)
    for dy in sdk_dylibs_attr:
        swift_argv.extend(["-l" + dy])
    for dy in dep_sdk_dylibs:
        if dy not in sdk_dylibs_attr:
            swift_argv.extend(["-l" + dy])
    for opt in _apple_unique_linkopts(linkopts + dep_linkopts):
        swift_argv.append(opt)
    _apple_add_swift_plugin_args(swift_argv, plugin_dylibs, plugin_executables)
    for flag in _apple_swift_link_flags(swift_flags):
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    _apple_append_archives(swift_argv, dep_archives, dep_archives)

    swift_inputs = list(swift_srcs)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)
    for f in dep_framework_files:
        if f not in swift_inputs:
            swift_inputs.append(f)
    for overlay in dep_vfs_overlays:
        if overlay not in swift_inputs:
            swift_inputs.append(overlay)
    for plugin_input in _apple_swift_plugin_inputs(plugin_dylibs, plugin_executables):
        if plugin_input not in swift_inputs:
            swift_inputs.append(plugin_input)
    for path in _apple_link_option_inputs(linkopts + dep_linkopts):
        if path not in swift_inputs:
            swift_inputs.append(path)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [dylib, swiftmodule, swiftdoc],
        env = swiftc["env"],
        toolchain_identity = swiftc["identity"],
        identifier = "apple_framework_compile_" + module_name,
    )

    # No `module * { export * }` line: that requires an umbrella
    # header declaration, and the bundled framework relies on the
    # Swift compiler reading the `.swiftmodule` in this same Modules/
    # directory rather than on an inferred ObjC submodule.
    write_path(modulemap, "framework module " + module_name + " {\n    export *\n}\n")

    plist_entries = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": product_name,
        "CFBundleIdentifier": bundle_id,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": product_name,
        "CFBundlePackageType": "FMWK",
        "CFBundleShortVersionString": "1.0",
        "CFBundleVersion": "1",
        "MinimumOSVersion": minimum_os,
    }
    write_path(info_plist, _render_plist(plist_entries))

    # Ad-hoc codesign so iOS simulator's dyld accepts the dylib when
    # the embedding app loads it.
    codesign = _resolve_codesign(xcode_developer_dir)
    cs_stamp = declare_output(framework_dir + "/_CodeSignature/CodeResources")
    run_action(
        argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none", ctx["build_dir"] + "/" + framework_dir],
        inputs = [dylib, info_plist, modulemap, swiftmodule],
        outputs = [dylib, cs_stamp],
        env = codesign["env"],
        toolchain_identity = codesign["identity"],
        identifier = "apple_framework_codesign_" + module_name,
    )

    absorbed_static_archives = _collect_transitive(deps, "transitive_archives", [])
    transitive_swiftmodule_dirs = _collect_transitive(deps, "transitive_swiftmodule_dirs", [])
    transitive_sdk_frameworks = _collect_transitive(deps, "transitive_sdk_frameworks", sdk_frameworks_attr)
    transitive_weak_sdk_frameworks = _collect_transitive(deps, "transitive_weak_sdk_frameworks", weak_sdk_frameworks)
    transitive_sdk_dylibs = _collect_transitive(deps, "transitive_sdk_dylibs", sdk_dylibs_attr)
    transitive_linkopts = _apple_collect_transitive_linkopts(deps, linkopts)
    transitive_plugin_dylibs = _collect_transitive(deps, "transitive_plugin_dylibs", plugin_dylibs)
    transitive_plugin_executables = _collect_transitive(deps, "transitive_plugin_executables", plugin_executables)

    framework_files = [dylib, swiftmodule, swiftdoc, modulemap, info_plist, cs_stamp]
    own_framework_bundle = _apple_framework_bundle(
        ctx["build_dir"] + "/" + framework_dir,
        module_name,
        framework_files,
        ctx["label"]["id"],
        absorbed_static_archives,
    )
    transitive_link_framework_bundles = [own_framework_bundle]
    transitive_framework_bundles = _apple_collect_runtime_framework_bundles(deps, [own_framework_bundle])

    return {
        "label_id": ctx["label"]["id"],
        "framework_path": ctx["build_dir"] + "/" + framework_dir,
        "framework_module_name": module_name,
        "framework_files": framework_files,
        "swiftmodule_dir": ctx["build_dir"] + "/" + framework_dir + "/Modules",
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_archives": [],
        "absorbed_static_archives": absorbed_static_archives,
        "transitive_link_framework_bundles": transitive_link_framework_bundles,
        "transitive_framework_bundles": transitive_framework_bundles,
        "transitive_frameworks": _apple_framework_bundle_paths(transitive_framework_bundles),
        "transitive_sdk_frameworks": transitive_sdk_frameworks,
        "transitive_weak_sdk_frameworks": transitive_weak_sdk_frameworks,
        "transitive_sdk_dylibs": transitive_sdk_dylibs,
        "transitive_linkopts": transitive_linkopts,
        "transitive_plugin_dylibs": transitive_plugin_dylibs,
        "transitive_plugin_executables": transitive_plugin_executables,
    }

def _apple_embed_framework_bundles(ctx, deps, bundle_dir, frameworks_dir, codesign, identifier_prefix):
    embedded_paths = []
    embedded_stamps = []
    embedded_files = []
    destination_sources = {}
    for bundle in _apple_collect_runtime_framework_bundles(deps):
        framework_path = bundle["path"]
        framework_basename = _basename(framework_path)
        previous_source = destination_sources.get(framework_basename)
        if previous_source and previous_source != framework_path:
            fail(ctx["label"]["id"] + ": framework bundle collision for `" + framework_basename + "` between `" + previous_source + "` and `" + framework_path + "`")
        destination_sources[framework_basename] = framework_path
        source_files = bundle.get("files") or [framework_path]
        framework_prefix = framework_path + "/"
        embedded_relative_path = bundle_dir + "/" + frameworks_dir + "/" + framework_basename
        embed_outputs = []
        for source in source_files:
            if source == framework_path:
                embed_outputs.append(declare_output(embedded_relative_path))
                continue
            if source.startswith(framework_prefix):
                rel = source[len(framework_prefix):]
                embed_outputs.append(declare_output(embedded_relative_path + "/" + rel))
        embedded_stamp = declare_output(embedded_relative_path + "/_CodeSignature/CodeResources")
        if embedded_stamp not in embed_outputs:
            embed_outputs.append(embedded_stamp)
        embedded_files.extend(embed_outputs)
        embedded_framework_path = ctx["build_dir"] + "/" + embedded_relative_path
        embedded_paths.append(embedded_framework_path)
        copy_path(
            framework_path,
            embedded_framework_path,
            kind = "tree",
            inputs = source_files,
            identifier = identifier_prefix + "_copy_" + framework_basename,
        )
        run_action(
            argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none", embedded_framework_path],
            inputs = [embedded_framework_path],
            outputs = embed_outputs,
            env = codesign["env"],
            toolchain_identity = codesign["identity"],
            identifier = identifier_prefix + "_" + framework_basename,
        )
        embedded_stamps.append(embedded_stamp)
    return {
        "paths": embedded_paths,
        "stamps": embedded_stamps,
        "files": embedded_files,
    }

def _apple_embed_resource_bundles(ctx, deps, bundle_dir, codesign, identifier_prefix, own_bundles = []):
    embedded_paths = []
    embedded_stamps = []
    embedded_files = []
    destination_sources = {}
    for bundle in _apple_collect_resource_bundles(deps, own_bundles):
        bundle_path = bundle["path"]
        bundle_basename = _basename(bundle_path)
        previous_source = destination_sources.get(bundle_basename)
        if previous_source and previous_source != bundle_path:
            fail(ctx["label"]["id"] + ": resource bundle collision for `" + bundle_basename + "` between `" + previous_source + "` and `" + bundle_path + "`")
        destination_sources[bundle_basename] = bundle_path
        source_files = bundle.get("files") or [bundle_path]
        embedded_relative_path = bundle_dir + "/" + bundle_basename
        embedded_path = ctx["build_dir"] + "/" + embedded_relative_path
        embedded_stamp = declare_output(embedded_relative_path + "/_CodeSignature/CodeResources")
        copy_path(
            bundle_path,
            embedded_path,
            kind = "tree",
            inputs = source_files,
            identifier = identifier_prefix + "_copy_" + bundle_basename,
        )
        run_action(
            argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none", embedded_path],
            inputs = [embedded_path],
            outputs = [embedded_path, embedded_stamp],
            env = codesign["env"],
            toolchain_identity = codesign["identity"],
            identifier = identifier_prefix + "_sign_" + bundle_basename,
        )
        embedded_paths.append(embedded_path)
        embedded_stamps.append(embedded_stamp)
        embedded_files.extend([embedded_path, embedded_stamp])
    return {
        "paths": embedded_paths,
        "stamps": embedded_stamps,
        "files": embedded_files,
    }

def shell_quote_for_action(path):
    # Single-quote the path and escape any embedded single quotes by
    # closing, escaping with double-quoted apostrophe, and reopening.
    escaped = path.replace("'", "'\"'\"'")
    return "'" + escaped + "'"

def _apple_application_run_script(label_id, platform, sdk_variant, xcrun, app_path, bundle_id, run_dir, run_record, run_log, visible):
    target_json = _json_literal(label_id)
    platform_json = _json_literal(platform)
    bundle_json = _json_literal(bundle_id)
    app_json = _json_literal(app_path)
    if platform == "macos" or platform == "macosx":
        record_json = '{"schema":"once.run.v1","target":' + target_json + ',"kind":"apple_application","status":"launched","platform":' + platform_json + ',"bundle_id":' + bundle_json + ',"app_path":' + app_json + '}'
        command = """/usr/bin/open -n {app} >> {log} 2>&1
printf '%s\\n' {record_json} > {record}
""".format(
            app = _shell_literal(app_path),
            log = _shell_literal(run_log),
            record_json = _shell_literal(record_json),
            record = _shell_literal(run_record),
        )
    elif platform == "ios" and sdk_variant == "simulator":
        record_prefix = '{"schema":"once.run.v1","target":' + target_json + ',"kind":"apple_application","status":"launched","platform":' + platform_json + ',"sdk_variant":"simulator","bundle_id":' + bundle_json + ',"app_path":' + app_json + ',"simulator_id":"'
        record_suffix = '"}'
        visible_command = """/usr/bin/open -a Simulator --args -CurrentDeviceUDID "$simulator_id" >> {log} 2>&1 || true
""".format(log = _shell_literal(run_log)) if visible else ""
        command = _ios_simulator_selection_script(xcrun) + """
{xcrun} simctl boot "$simulator_id" >> {log} 2>&1 || true
{visible_command}{xcrun} simctl bootstatus "$simulator_id" -b >> {log} 2>&1
{xcrun} simctl install "$simulator_id" {app} >> {log} 2>&1
{xcrun} simctl launch "$simulator_id" {bundle_id} >> {log} 2>&1
printf '%s%s%s\\n' {record_prefix} "$simulator_id" {record_suffix} > {record}
""".format(
            xcrun = _shell_literal(xcrun),
            log = _shell_literal(run_log),
            visible_command = visible_command,
            app = _shell_literal(app_path),
            bundle_id = _shell_literal(bundle_id),
            record_prefix = _shell_literal(record_prefix),
            record_suffix = _shell_literal(record_suffix),
            record = _shell_literal(run_record),
        )
    else:
        fail(label_id + ": apple_application run supports macos and ios simulator targets")
    return """set -eu
: > {log}
{command}
""".format(
        log = _shell_literal(run_log),
        command = command,
    )

def _apple_swift_module_name(name):
    # Turn a product name into a valid Swift module identifier the way Xcode's
    # `c99extidentifier` transform does: non-identifier characters become
    # underscores, and a leading digit is prefixed with an underscore.
    out = ""
    for ch in (name or "").elems():
        if (ch >= "a" and ch <= "z") or (ch >= "A" and ch <= "Z") or (ch >= "0" and ch <= "9") or ch == "_":
            out += ch
        else:
            out += "_"
    if out and out[0] >= "0" and out[0] <= "9":
        out = "_" + out
    return out or "Module"

def _apple_link_option_inputs(options):
    return _unique([
        option
        for option in options
        if not option.startswith("/") and (option.endswith(".a") or option.endswith(".dylib") or option.endswith(".o"))
    ])

def _apple_run_prebuild_actions(ctx, attrs):
    # Prebuild actions are intentionally generic records. They model source
    # generation that must complete before the compiler expands its inputs.
    generated_sources = []
    for encoded in attrs.get("prebuild_actions") or []:
        action = json_decode(encoded)
        contents = action.get("contents")
        if contents != None:
            for output in action.get("outputs") or []:
                write_path(output, contents)
                if output.endswith(".swift") or output.endswith(".m") or output.endswith(".mm") or output.endswith(".c") or output.endswith(".cc") or output.endswith(".cpp"):
                    generated_sources.append(output)
            continue
        argv = action.get("argv") or []
        action_env = action.get("env") or {}
        identity = None
        if not argv and action.get("tool") == "momc":
            momc = _resolve_momc(attrs.get("xcode_developer_dir") or "")
            argv = [momc["path"]] + (action.get("args") or [])
            action_env = dict(momc["env"])
            action_env.update(action.get("env") or {})
            identity = momc["identity"]
        if not argv and action.get("tool") == "intentbuilderc":
            intentbuilderc = _resolve_intentbuilderc(attrs.get("xcode_developer_dir") or "")
            argv = [intentbuilderc["path"]] + (action.get("args") or [])
            action_env = dict(intentbuilderc["env"])
            action_env.update(action.get("env") or {})
            identity = intentbuilderc["identity"]
        if not argv:
            shell = action.get("shell") or host_which("sh")
            if "/" not in shell:
                shell = host_which(shell)
            argv = [shell, "-c", action.get("script") or ""]
            identity = "once.apple.prebuild.shell.v1\0" + shell + "\0" + host_file_sha256(shell)
        cacheable = action.get("cacheable") == True
        output_dirs = _unique([
            _parent_dir(output)
            for output in (action.get("outputs") or [])
            if _parent_dir(output)
        ])
        run_action(
            argv = argv,
            inputs = action.get("inputs") or [],
            outputs = action.get("outputs") or [],
            cwd = action.get("cwd") or None,
            env = action_env,
            cacheable = cacheable,
            inherit_parent_env = not cacheable,
            sandbox = "off",
            create_dirs = output_dirs,
            toolchain_identity = identity or "",
            identifier = "prebuild_action:" + ctx["label"]["id"] + ":" + (action.get("name") or "script"),
        )
        for output in action.get("outputs") or []:
            if _filter_swift_sources([output]) or _filter_objc_sources([output]) or _filter_c_sources([output]) or _filter_cxx_sources([output]) or _filter_assembly_sources([output]):
                generated_sources.append(output)
    return _unique(generated_sources)

def _apple_declared_source_paths(ctx):
    # `glob` cannot return a source that a preceding build phase creates. Keep
    # exact source paths so declared generators can materialize them first.
    out = []
    for path in ctx["srcs"]:
        if "*" in path or "?" in path or "[" in path:
            continue
        if _filter_swift_sources([path]) or _filter_objc_sources([path]) or _filter_c_sources([path]) or _filter_cxx_sources([path]) or _filter_assembly_sources([path]):
            out.append(path)
    return _unique(out)

def _apple_swift_emits_single_object(flags):
    whole_module = False
    thread_count = "0"
    for index in range(len(flags)):
        flag = flags[index]
        if flag in ["-wmo", "-whole-module-optimization", "-Owholemodule"]:
            whole_module = True
        elif flag == "-num-threads" and index + 1 < len(flags):
            thread_count = str(flags[index + 1])
        elif flag.startswith("-num-threads="):
            thread_count = flag[len("-num-threads="):]
    return whole_module and thread_count == "0"

def _apple_swift_link_flags(flags):
    return [flag for flag in flags if flag != "-enable-batch-mode"]

def _apple_application_impl(ctx):
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["product_name"])
    _reject_unsupported_attrs(attrs, ctx["label"]["id"], ["provisioning_profile", "signing_identity"])
    if attrs.get("signing") and attrs.get("signing") != "ad_hoc":
        fail(ctx["label"]["id"] + ": attribute `signing` only supports `ad_hoc` today")
    platform = attrs["platform"]
    bundle_id = attrs["bundle_id"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    product_name = attrs.get("product_name") or ctx["label"]["name"]
    # The Swift module name must be a valid identifier, while the product name
    # can contain spaces (a bundle displayed as "Ice Cubes" has module name
    # "Ice_Cubes"), so derive the module name separately.
    module_name = _apple_swift_module_name(attrs.get("module_name") or product_name)
    families = attrs.get("families") or ["iphone"]
    sdk_frameworks_attr = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs_attr = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    defines = attrs.get("defines") or []
    swift_defines = _unique(defines + (attrs.get("swift_defines") or []))
    clang_defines = _unique(defines + (attrs.get("clang_defines") or []))
    swift_flags = attrs.get("swift_flags") or []
    resources = attrs.get("resources") or []
    structured_resources = attrs.get("structured_resources") or []
    bridging_header = attrs.get("bridging_header") or ""
    prefix_header = attrs.get("prefix_header") or ""
    entitlements = attrs.get("entitlements") or ""
    entitlements_substitutions = attrs.get("entitlements_substitutions") or {}
    info_plist_template = attrs.get("info_plist") or ""
    info_plist_substitutions = attrs.get("info_plist_substitutions") or {}
    development_team = attrs.get("development_team") or ""
    private_header_dirs = []
    for header_dir in (attrs.get("exported_header_dirs") or []) + (attrs.get("private_header_dirs") or []):
        resolved_header_dir = _package_relative(ctx, header_dir)
        absolute_header_dir = resolved_header_dir if resolved_header_dir.startswith("/") else workspace_root() + "/" + resolved_header_dir
        if resolved_header_dir and host_path_exists(absolute_header_dir) and resolved_header_dir not in private_header_dirs:
            private_header_dirs.append(resolved_header_dir)
    private_header_files = _apple_header_inputs(ctx, private_header_dirs)
    application_extension = attrs.get("application_extension") or False
    enable_testing = attrs.get("enable_testing") or False
    if enable_testing:
        swift_flags = list(swift_flags) + ["-enable-testing"]

    generated_srcs = _apple_run_prebuild_actions(ctx, attrs)
    all_srcs = _unique(glob(ctx["srcs"]) + _apple_declared_source_paths(ctx) + generated_srcs)
    swift_srcs = _filter_swift_sources(all_srcs)
    objc_srcs = _filter_objc_sources(all_srcs)
    c_srcs = _filter_c_sources(all_srcs)
    cxx_srcs = _filter_cxx_sources(all_srcs)
    assembly_srcs = _filter_assembly_sources(all_srcs)
    if len(swift_srcs) == 0:
        fail("apple_application " + ctx["label"]["id"] + " has no Swift sources (.swift)")
    emits_swift_module = enable_testing or len(objc_srcs) > 0
    swiftmodule = declare_output(product_name + ".swiftmodule") if emits_swift_module else ""
    swiftdoc = declare_output(product_name + ".swiftdoc") if emits_swift_module else ""
    swift_objc_header = declare_output(module_name + "-Swift.h") if len(objc_srcs) > 0 else ""

    # Asset catalogs: generate the type-safe `ImageResource`/`ColorResource`
    # accessors so sources that reference them compile, and compile the catalog
    # into the `Assets.car` the app loads at runtime. `actool` only emits the
    # Swift symbols when a compile pass runs, and only emits `Assets.car` when
    # the symbol pass is absent, so the two run as separate actions.
    asset_catalogs = [_package_relative(ctx, catalog) for catalog in (attrs.get("asset_catalogs") or [])]
    app_icon = attrs.get("app_icon") or ""
    asset_car = ""
    if asset_catalogs:
        actool = _resolve_actool(xcode_developer_dir)
        actool_platform = _apple_actool_platform(platform, sdk_variant)
        asset_symbols = declare_output("GeneratedAssetSymbols.swift")
        # `actool` only writes the Swift symbols when a compile pass runs, so a
        # throwaway compile directory is supplied; it is not a declared output.
        symbol_argv = [actool["actool_path"]] + asset_catalogs + [
            "--compile",
            ctx["build_dir"] + "/AssetSymbolsCompile",
            "--generate-swift-asset-symbols",
            asset_symbols,
            "--platform",
            actool_platform,
            "--minimum-deployment-target",
            minimum_os,
            "--bundle-identifier",
            bundle_id,
        ]
        run_action(
            argv = symbol_argv,
            inputs = asset_catalogs,
            outputs = [asset_symbols],
            create_dirs = [ctx["build_dir"] + "/AssetSymbolsCompile"],
            clean_paths = [ctx["build_dir"] + "/AssetSymbolsCompile"],
            env = actool["env"],
            toolchain_identity = actool["identity"],
            identifier = "apple_application_asset_symbols_" + product_name,
        )
        swift_srcs = swift_srcs + [asset_symbols]

        app_dir = product_name + ".app"
        asset_car = declare_output(app_dir + "/Assets.car")
        car_partial_plist = declare_output("assets-partial.plist")
        car_argv = [actool["actool_path"]] + asset_catalogs + [
            "--compile",
            ctx["build_dir"] + "/" + app_dir,
            "--platform",
            actool_platform,
            "--minimum-deployment-target",
            minimum_os,
            "--output-partial-info-plist",
            car_partial_plist,
        ]
        if app_icon:
            car_argv.extend(["--app-icon", app_icon])
        run_action(
            argv = car_argv,
            inputs = asset_catalogs,
            outputs = [asset_car, car_partial_plist],
            env = actool["env"],
            toolchain_identity = actool["identity"],
            identifier = "apple_application_assets_" + product_name,
        )

    arch = host_arch()
    swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant, arch, False)

    app_dir = product_name + ".app"
    app_path = ctx["build_dir"] + "/" + app_dir
    if ctx["capability"] == "run":
        run_dir = ctx["build_dir"] + "/run"
        run_record = run_dir + "/run.json"
        run_log = run_dir + "/run.log"
        # simctl-based runners need `xcrun` on PATH at execution time;
        # resolve it here so the build path stays xcrun-free.
        runner_xcrun = host_which("xcrun") if platform == "ios" and sdk_variant == "simulator" else ""
        run_visible = (ctx.get("run") or {}).get("visible") or False
        prepare_path(run_dir, kind = "directory", identifier = "apple_application_run_dir:" + ctx["label"]["id"])
        run_action(
            argv = [host_which("sh"), "-c", _apple_application_run_script(ctx["label"]["id"], platform, sdk_variant, runner_xcrun, app_path, bundle_id, run_dir, run_record, run_log, run_visible)],
            outputs = [run_dir, run_record, run_log],
            env = swiftc["env"],
            cacheable = False,
            toolchain_identity = "once.apple.application.run.v1\x00" + swiftc["identity"],
            identifier = "apple_application_run_" + product_name,
        )
        return {
            "label_id": ctx["label"]["id"],
            "target_kind": "apple_application",
            "app_path": app_path,
            "bundle_id": bundle_id,
            "platform": platform,
            "sdk_variant": sdk_variant,
            "xcode_developer_dir": xcode_developer_dir,
            "product_name": product_name,
        }

    executable = declare_output(app_dir + "/" + product_name)
    info_plist = declare_output(app_dir + "/Info.plist")
    compile_module_cache = ctx["build_dir"] + "/ModuleCache/Compile"
    testable_module_cache = ctx["build_dir"] + "/ModuleCache/TestableModule"

    processed_entitlements = ""
    der_entitlements = ""
    embeds_simulator_entitlements = entitlements and sdk_variant == "simulator" and platform != "macos" and platform != "macosx"
    if entitlements:
        entitlements_source = _package_relative(ctx, entitlements)
        entitlements_path = entitlements_source if entitlements_source.startswith("/") else workspace_root() + "/" + entitlements_source
        entitlements_content = host_file_read(entitlements_path)
        for key, value in entitlements_substitutions.items():
            entitlements_content = entitlements_content.replace("$(" + key + ")", value).replace("${" + key + "}", value)
        if embeds_simulator_entitlements:
            entitlements_content = _apple_entitlements_with_application_identifier(entitlements_content, development_team, bundle_id)
        processed_entitlements = declare_output(ctx["label"]["name"] + "/processed-entitlements.plist")
        write_path(processed_entitlements, entitlements_content)
        if embeds_simulator_entitlements:
            derq = _resolve_derq(xcode_developer_dir)
            der_entitlements = declare_output(ctx["label"]["name"] + "/processed-entitlements.der")
            run_action(
                argv = [derq["path"], "query", "-f", "xml", "-i", processed_entitlements, "-o", der_entitlements, "--raw"],
                inputs = [processed_entitlements],
                outputs = [der_entitlements],
                env = derq["env"],
                toolchain_identity = derq["identity"],
                identifier = "apple_application_der_entitlements_" + product_name,
            )

    deps = _apple_native_deps(ctx)
    _validate_apple_native_deps(deps, ctx["label"]["id"])
    (
        compile_swiftmodule_dirs,
        compile_header_dirs,
        dep_modulemaps,
        dep_hmaps,
        dep_archives,
        framework_search_dirs,
        framework_module_names,
        dep_framework_files,
        dep_sdk_frameworks,
        dep_sdk_dylibs,
        dep_linkopts,
        dep_vfs_overlays,
        plugin_dylibs,
        plugin_executables,
    ) = _collect_dep_compile_inputs(deps, ctx["build_dir"])
    alwayslink_archives = _apple_collect_alwayslink_archives(deps)
    runtime_framework_bundles = _apple_collect_runtime_framework_bundles(deps)
    has_main_source = False
    for src in swift_srcs:
        if _basename(src) == "main.swift":
            has_main_source = True
            break

    swift_argv = list(swiftc["argv"]) + [
        "-module-name",
        module_name,
        "-target",
        triple,
        "-Xlinker",
        "-rpath",
        "-Xlinker",
        "@executable_path/Frameworks",
        "-o",
        executable,
        # Actions run with a cleared environment, so give the Clang importer an
        # explicit, writable module cache under the build directory. Without it,
        # importing a source-built module map (an Objective-C Swift package
        # dependency) fails because the implicit cache has nowhere to go.
        "-module-cache-path",
        compile_module_cache,
    ]
    if embeds_simulator_entitlements:
        swift_argv.extend([
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__entitlements",
            "-Xlinker",
            processed_entitlements,
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__ents_der",
            "-Xlinker",
            der_entitlements,
        ])
    if not has_main_source:
        swift_argv.append("-parse-as-library")
    if application_extension:
        # An app extension is not a normal executable: it is entered through
        # `NSExtensionMain` (from Foundation) rather than `main`, and is built
        # against the app-extension-safe API surface.
        swift_argv.extend([
            "-application-extension",
            "-Xlinker",
            "-e",
            "-Xlinker",
            "_NSExtensionMain",
        ])
    if bridging_header:
        swift_argv.extend(["-import-objc-header", _package_relative(ctx, bridging_header)])
    for d in compile_swiftmodule_dirs:
        swift_argv.extend(["-I", d])
    for hdir in compile_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for hdir in private_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for mmap in dep_modulemaps:
        swift_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
    for hmap in dep_hmaps:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
    for overlay in dep_vfs_overlays:
        swift_argv.extend(["-Xcc", "-ivfsoverlay", "-Xcc", overlay])
    for d in framework_search_dirs:
        swift_argv.extend(["-F", d])
    _apple_disable_static_framework_autolinking(swift_argv, _apple_collect_link_framework_bundles(deps))
    for fw in framework_module_names:
        swift_argv.extend(["-framework", fw])
    for fw in sdk_frameworks_attr:
        swift_argv.extend(["-framework", fw])
    for fw in dep_sdk_frameworks:
        if fw not in sdk_frameworks_attr:
            swift_argv.extend(["-framework", fw])
    for fw in weak_sdk_frameworks:
        _apple_append_weak_framework(swift_argv, fw)
    for dy in sdk_dylibs_attr:
        swift_argv.extend(["-l" + dy])
    for dy in dep_sdk_dylibs:
        if dy not in sdk_dylibs_attr:
            swift_argv.extend(["-l" + dy])
    for opt in _apple_unique_linkopts(linkopts + dep_linkopts):
        swift_argv.append(opt)
    _apple_add_swift_plugin_args(swift_argv, plugin_dylibs, plugin_executables)
    for define in swift_defines:
        swift_argv.extend(["-D", define])
    for define in clang_defines:
        swift_argv.extend(["-Xcc", "-D" + define])
    for flag in _apple_swift_link_flags(swift_flags):
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    _apple_append_archives(swift_argv, dep_archives, alwayslink_archives)

    swift_inputs = list(swift_srcs)
    if embeds_simulator_entitlements:
        swift_inputs.extend([processed_entitlements, der_entitlements])
    if bridging_header:
        bridging_header_path = _package_relative(ctx, bridging_header)
        if bridging_header_path not in swift_inputs:
            swift_inputs.append(bridging_header_path)
    for mmap in dep_modulemaps:
        if mmap not in swift_inputs:
            swift_inputs.append(mmap)
    for hmap in dep_hmaps:
        if hmap not in swift_inputs:
            swift_inputs.append(hmap)
    for header in private_header_files:
        if header not in swift_inputs:
            swift_inputs.append(header)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)
    for f in dep_framework_files:
        if f not in swift_inputs:
            swift_inputs.append(f)
    for overlay in dep_vfs_overlays:
        if overlay not in swift_inputs:
            swift_inputs.append(overlay)
    for plugin_input in _apple_swift_plugin_inputs(plugin_dylibs, plugin_executables):
        if plugin_input not in swift_inputs:
            swift_inputs.append(plugin_input)
    for path in _apple_link_option_inputs(linkopts + dep_linkopts):
        if path not in swift_inputs:
            swift_inputs.append(path)

    # Emit the application module separately so hosted test bundles can
    # `@testable import` it and mixed-language targets can compile Objective-C
    # sources against the generated Swift compatibility header. The main
    # compile links prebuilt archives, which conflicts with `-emit-module` in a
    # single swiftc invocation, so the module is produced by a compile-only
    # action that reuses the same sources and search paths. Attribute-based
    # entry points require library parsing, while a conventional `main.swift`
    # must retain top-level statements.
    if emits_swift_module:
        module_argv = list(swiftc["argv"]) + [
            "-module-name",
            module_name,
            "-target",
            triple,
            "-emit-module",
            "-emit-module-path",
            swiftmodule,
        ]
        if enable_testing:
            module_argv.append("-enable-testing")
        if swift_objc_header:
            module_argv.extend([
                "-emit-objc-header",
                "-emit-objc-header-path",
                swift_objc_header,
            ])
        if not has_main_source:
            module_argv.append("-parse-as-library")
        module_argv.extend(["-module-cache-path", testable_module_cache])
        if application_extension:
            module_argv.append("-application-extension")
        if bridging_header:
            module_argv.extend(["-import-objc-header", _package_relative(ctx, bridging_header)])
        for d in compile_swiftmodule_dirs:
            module_argv.extend(["-I", d])
        for hdir in compile_header_dirs:
            module_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
        for hdir in private_header_dirs:
            module_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
        for mmap in dep_modulemaps:
            module_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
        for overlay in dep_vfs_overlays:
            module_argv.extend(["-Xcc", "-ivfsoverlay", "-Xcc", overlay])
        for d in framework_search_dirs:
            module_argv.extend(["-F", d])
        _apple_disable_static_framework_autolinking(module_argv, _apple_collect_link_framework_bundles(deps))
        for fw in framework_module_names:
            module_argv.extend(["-framework", fw])
        for fw in sdk_frameworks_attr:
            module_argv.extend(["-framework", fw])
        _apple_add_swift_plugin_args(module_argv, plugin_dylibs, plugin_executables)
        for define in swift_defines:
            module_argv.extend(["-D", define])
        for define in clang_defines:
            module_argv.extend(["-Xcc", "-D" + define])
        for flag in swift_flags:
            module_argv.append(flag)
        for src in swift_srcs:
            module_argv.append(src)
        module_inputs = list(swift_srcs)
        if bridging_header and _package_relative(ctx, bridging_header) not in module_inputs:
            module_inputs.append(_package_relative(ctx, bridging_header))
        for mmap in dep_modulemaps:
            if mmap not in module_inputs:
                module_inputs.append(mmap)
        for hmap in dep_hmaps:
            if hmap not in module_inputs:
                module_inputs.append(hmap)
        for header in private_header_files:
            if header not in module_inputs:
                module_inputs.append(header)
        for overlay in dep_vfs_overlays:
            if overlay not in module_inputs:
                module_inputs.append(overlay)
        for f in dep_framework_files:
            if f not in module_inputs:
                module_inputs.append(f)
        for plugin_input in _apple_swift_plugin_inputs(plugin_dylibs, plugin_executables):
            if plugin_input not in module_inputs:
                module_inputs.append(plugin_input)
        run_action(
            argv = module_argv,
            inputs = module_inputs,
            outputs = [swiftmodule, swiftdoc] + ([swift_objc_header] if swift_objc_header else []),
            env = swiftc["env"],
            toolchain_identity = swiftc["identity"],
            identifier = "apple_application_module_" + product_name,
        )

    clang_objects = []
    if len(objc_srcs) > 0 or len(c_srcs) > 0 or len(cxx_srcs) > 0 or len(assembly_srcs) > 0:
        clang = _resolve_clang(platform, sdk_variant, xcode_developer_dir)
        clang_flags = attrs.get("clang_flags") or []
        per_source_clang_flags = attrs.get("per_source_clang_flags") or {}

        def compile_application_source(src, language):
            is_assembly = language == "assembler-with-cpp"
            sanitised = src.replace("/", "_")
            obj = declare_output("Objects/" + sanitised + ".o")
            argv = [
                clang["clangxx_path"] if language == "c++" or language == "objective-c++" else clang["clang_path"],
                "-c",
                "-x",
                language,
                "-arch",
                arch,
                "-isysroot",
                clang["sdk_path"],
                "-target",
                triple,
                "-o",
                obj,
            ]
            if not is_assembly:
                argv.extend(["-fmodules", "-fmodule-name=" + module_name])
            if language == "objective-c" or language == "objective-c++":
                argv.append("-fobjc-arc")
            for hdir in compile_header_dirs:
                argv.extend(["-I", hdir])
            for hdir in private_header_dirs:
                argv.extend(["-I", hdir])
            if swift_objc_header:
                argv.extend(["-I", ctx["build_dir"]])
            if not is_assembly:
                for mmap in dep_modulemaps:
                    argv.append("-fmodule-map-file=" + mmap)
            for hmap in dep_hmaps:
                argv.extend(["-I", hmap])
            for overlay in dep_vfs_overlays:
                argv.extend(["-ivfsoverlay", overlay])
            if prefix_header and not is_assembly:
                argv.extend(["-include", _package_relative(ctx, prefix_header)])
            for framework_dir in framework_search_dirs:
                argv.extend(["-F", framework_dir])
            for define in clang_defines:
                argv.append("-D" + define)
            for flag in clang_flags:
                if is_assembly and flag.startswith("-std="):
                    continue
                if language != "c++" and language != "objective-c++" and flag.startswith("-std=c++"):
                    continue
                argv.append(flag)
            for flag in json_decode(per_source_clang_flags.get(src) or "[]"):
                if is_assembly and flag.startswith("-std="):
                    continue
                if language != "c++" and language != "objective-c++" and flag.startswith("-std=c++"):
                    continue
                argv.append(flag)
            argv.append(src)

            inputs = [src]
            if bridging_header:
                inputs.append(_package_relative(ctx, bridging_header))
            if swift_objc_header:
                inputs.append(swift_objc_header)
            if prefix_header and not is_assembly:
                inputs.append(_package_relative(ctx, prefix_header))
            for header in private_header_files:
                if header not in inputs:
                    inputs.append(header)
            for mmap in dep_modulemaps:
                if mmap not in inputs:
                    inputs.append(mmap)
            for hmap in dep_hmaps:
                if hmap not in inputs:
                    inputs.append(hmap)
            for file in dep_framework_files:
                if file not in inputs:
                    inputs.append(file)
            for overlay in dep_vfs_overlays:
                if overlay not in inputs:
                    inputs.append(overlay)
            run_action(
                argv = argv,
                inputs = inputs,
                outputs = [obj],
                env = clang["env"],
                toolchain_identity = clang["identity"],
                identifier = "apple_application_clang_compile_" + module_name + "_" + sanitised,
            )
            clang_objects.append(obj)

        for src in objc_srcs:
            compile_application_source(src, "objective-c++" if src.endswith(".mm") else "objective-c")
        for src in c_srcs:
            compile_application_source(src, "c")
        for src in cxx_srcs:
            compile_application_source(src, "c++")
        for src in assembly_srcs:
            compile_application_source(src, "assembler-with-cpp")

    for obj in clang_objects:
        swift_argv.append(obj)
        if obj not in swift_inputs:
            swift_inputs.append(obj)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [executable],
        env = swiftc["env"],
        toolchain_identity = swiftc["identity"],
        identifier = "apple_application_compile_" + product_name,
    )

    if info_plist_template:
        info_plist_source = _package_relative(ctx, info_plist_template)
        info_plist_path = info_plist_source if info_plist_source.startswith("/") else workspace_root() + "/" + info_plist_source
        info_plist_contents = host_file_read(info_plist_path)
        for key, value in info_plist_substitutions.items():
            info_plist_contents = info_plist_contents.replace("$(" + key + ")", value).replace("${" + key + "}", value)
        write_path(info_plist, info_plist_contents)
    else:
        plist_entries = {
            "CFBundleDevelopmentRegion": "en",
            "CFBundleExecutable": product_name,
            "CFBundleIdentifier": bundle_id,
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": product_name,
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": "1.0",
            "CFBundleVersion": "1",
            "MinimumOSVersion": minimum_os,
            "DTPlatformName": swiftc["sdk_name"],
        }
        bool_entries = {"LSRequiresIPhoneOS": True}
        device_family_codes = []
        for family in families:
            if family == "iphone":
                device_family_codes.append({"integer": 1})
            elif family == "ipad":
                device_family_codes.append({"integer": 2})
        array_entries = {
            "CFBundleSupportedPlatforms": [_apple_supported_platform(swiftc["sdk_name"])],
            "UIDeviceFamily": device_family_codes,
        }
        write_path(info_plist, _render_plist(plist_entries, bool_entries, array_entries))

    resource_files = _apple_materialize_resources(
        ctx,
        resources,
        app_dir,
        platform,
        minimum_os,
        xcode_developer_dir,
        module_name,
        "apple_application_resource_" + module_name,
        structured_resources,
    )

    codesign = _resolve_codesign(xcode_developer_dir)
    embedded_frameworks = _apple_embed_framework_bundles(
        ctx,
        deps,
        app_dir,
        "Frameworks",
        codesign,
        "apple_application_embed",
    )
    embedded_resource_bundles = _apple_embed_resource_bundles(
        ctx,
        deps,
        app_dir,
        codesign,
        "apple_application_embed_resource",
    )

    # Ad-hoc codesign the .app bundle itself. Must run after embedded
    # frameworks land so their signature is included in the bundle's
    # resource envelope.
    app_cs_stamp = declare_output(app_dir + "/_CodeSignature/CodeResources")
    cs_inputs = [executable, info_plist]
    if asset_car:
        cs_inputs.append(asset_car)
    cs_inputs.extend(resource_files)
    for stamp in embedded_frameworks["stamps"]:
        cs_inputs.append(stamp)
    for stamp in embedded_resource_bundles["stamps"]:
        cs_inputs.append(stamp)
    codesign_argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none"]
    if processed_entitlements and not embeds_simulator_entitlements:
        codesign_argv.extend(["--entitlements", processed_entitlements])
        cs_inputs.append(processed_entitlements)
    codesign_argv.append(ctx["build_dir"] + "/" + app_dir)
    run_action(
        argv = codesign_argv,
        inputs = cs_inputs,
        outputs = [executable, app_cs_stamp],
        env = codesign["env"],
        toolchain_identity = codesign["identity"],
        identifier = "apple_application_codesign_" + product_name,
    )

    transitive_swiftmodule_dirs = []
    if enable_testing:
        transitive_swiftmodule_dirs.append(ctx["build_dir"])
        for dep in deps:
            for d in dep.get("transitive_swiftmodule_dirs") or []:
                if d and d not in transitive_swiftmodule_dirs:
                    transitive_swiftmodule_dirs.append(d)
    transitive_exported_header_dirs = _unique(private_header_dirs + compile_header_dirs)
    transitive_generated_headers = []
    for dep in deps:
        for header in dep.get("transitive_generated_headers") or []:
            if header and header not in transitive_generated_headers:
                transitive_generated_headers.append(header)
    if swift_objc_header and swift_objc_header not in transitive_generated_headers:
        transitive_generated_headers.append(swift_objc_header)
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "apple_application",
        "app_path": app_path,
        "app_executable": executable,
        "application_extension": application_extension,
        "host_link_archives": dep_archives,
        "app_files": [executable, info_plist, app_cs_stamp] + resource_files + embedded_frameworks["files"] + embedded_resource_bundles["files"] + ([asset_car] if asset_car else []) + ([swiftmodule, swiftdoc] if enable_testing else []),
        "bundle_id": bundle_id,
        "platform": platform,
        "sdk_variant": sdk_variant,
        "xcode_developer_dir": xcode_developer_dir,
        "product_name": product_name,
        "swiftmodule_dir": ctx["build_dir"] if enable_testing else "",
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_exported_header_dirs": transitive_exported_header_dirs,
        "transitive_modulemaps": dep_modulemaps,
        "transitive_hmaps": dep_hmaps,
        "transitive_generated_headers": transitive_generated_headers,
        "transitive_framework_search_dirs": framework_search_dirs,
        "transitive_framework_files": dep_framework_files,
        "transitive_vfs_overlays": dep_vfs_overlays,
    }

def _apple_thinning_application(deps, label_id):
    if len(deps) != 1:
        fail(label_id + ": apple_thinned_package requires exactly one apple_application dependency")
    application = deps[0]
    if application.get("target_kind") != "apple_application" or not application.get("app_path"):
        fail(label_id + ": dependency must provide an apple_application bundle")
    if application.get("platform") != "ios":
        fail(label_id + ": apple_thinned_package only supports applications with platform = \"ios\"")
    if application.get("sdk_variant") != "device":
        fail(label_id + ": apple_thinned_package requires an application with sdk_variant = \"device\"")
    return application

def _apple_thinning_adapter():
    return '''require "fileutils"
require "find"
require "json"
require "open3"

ruby_arguments = [
  :ipatool,
  :codesign,
  :zip,
  :input,
  :device,
  :product,
  :target,
  :variants,
  :report,
  :packages,
  :manifest,
  :toolchain,
  :platforms,
]
options = ruby_arguments.zip(ARGV).to_h

def run_quietly(argv, failure, working_directory = nil, input = nil)
  _stdout, _stderr, status = if working_directory && input
    Open3.capture3(*argv, chdir: working_directory, stdin_data: input)
  elsif working_directory
    Open3.capture3(*argv, chdir: working_directory)
  else
    Open3.capture3(*argv)
  end
  raise failure unless status.success?
end

def safe_name(value)
  value.gsub(/[^A-Za-z0-9._-]+/, "-").gsub(/^-+|-+$/, "")
end

begin
  ipatool_argv = [
    options.fetch(:ipatool),
    options.fetch(:input),
    "--create-thinned=#{options.fetch(:device)}",
    "--validate-output",
    "--validate-output-zero-variants",
    "--toolchain=#{options.fetch(:toolchain)}",
    "--platforms=#{options.fetch(:platforms)}",
    "--json=#{options.fetch(:report)}",
    "--output=#{options.fetch(:variants)}",
    "--quiet",
  ]
  _stdout, _stderr, status = Open3.capture3(*ipatool_argv)
  unless status.success?
    alerts = if File.file?(options.fetch(:report))
      JSON.parse(File.read(options.fetch(:report))).fetch("alerts", [])
    else
      []
    end
    descriptions = alerts.each_with_object([]) do |alert, values|
      values << alert["description"] if alert["level"] == "ERROR" && alert["description"]
    end
    detail = descriptions.empty? ? "Xcode app thinning failed" : descriptions.join("\n")
    raise detail
  end

  report = JSON.parse(File.read(options.fetch(:report)))
  records = report.fetch("thinnedIPAs", []).select do |record|
    Array(record["devices"]).include?(options.fetch(:device))
  end
  raise "Xcode produced no thinned application for #{options.fetch(:device)}" if records.empty?

  records.sort_by! do |record|
    [
      Array(record["devices"]).join(","),
      JSON.generate(Array(record["installTargets"])),
      record.fetch("path"),
    ]
  end
  FileUtils.mkdir_p(options.fetch(:packages))
  fixed_time = Time.utc(1980, 1, 1)
  packages = records.each_with_index.map do |record, index|
    expanded = record.fetch("path")
    applications = Dir.glob(File.join(expanded, "Payload", "*.app")).sort
    raise "thinned output must contain exactly one application bundle" unless applications.length == 1

    signables = []
    Find.find(applications.first) do |path|
      if File.directory?(path) && [".app", ".appex", ".framework", ".xpc"].include?(File.extname(path))
        signables << path
      elsif File.file?(path) && File.extname(path) == ".dylib"
        signables << path
      end
    end
    signables.uniq.sort_by { |path| [-path.count("/"), path] }.each do |path|
      run_quietly(
        [options.fetch(:codesign), "--force", "--sign", "-", "--timestamp=none", path],
        "failed to sign #{File.basename(path)} after app thinning",
      )
    end

    Find.find(expanded) do |path|
      File.utime(fixed_time, fixed_time, path) unless File.symlink?(path)
    end
    suffix = records.length == 1 ? "" : "-#{index + 1}"
    filename = "#{safe_name(options.fetch(:product))}-#{safe_name(options.fetch(:device))}#{suffix}.ipa"
    package = File.join(options.fetch(:packages), filename)
    package_absolute = File.expand_path(package)
    archive_entries = []
    Find.find(expanded) do |path|
      next if path == expanded
      if File.file?(path) || File.symlink?(path)
        archive_entries << path.delete_prefix(expanded + "/")
      end
    end
    archive_entries.sort!
    run_quietly(
      [
        options.fetch(:zip),
        "-X",
        "-q",
        "-y",
        package_absolute,
        "-@",
      ],
      "failed to package #{filename}",
      expanded,
      archive_entries.join("\n") + "\n",
    )
    install_targets = Array(record["installTargets"]).sort_by do |target|
      [target["deviceModel"].to_s, target["operatingSystemVersion"].to_s]
    end
    {
      "path" => package,
      "devices" => Array(record["devices"]).sort,
      "installTargets" => install_targets,
    }
  end

  manifest = {
    "schema" => "once.apple.thinned-package.v1",
    "target" => options.fetch(:target),
    "deviceModel" => options.fetch(:device),
    "packages" => packages,
  }
  File.write(options.fetch(:manifest), JSON.pretty_generate(manifest) + "\n")
rescue StandardError => error
  warn error.message
  exit 1
end
'''

def _apple_thinned_package_impl(ctx):
    device_model = (ctx["attr"].get("device_model") or "").strip()
    if not device_model:
        fail(ctx["label"]["id"] + ": attribute `device_model` must name an Apple device model, such as `iPhone17,1`")
    if device_model == "all":
        fail(ctx["label"]["id"] + ": attribute `device_model` must name one device model; declare one target per model")

    application = _apple_thinning_application(ctx["deps"], ctx["label"]["id"])
    product_name = application.get("product_name") or ctx["label"]["name"]
    app_path = application["app_path"]
    app_files = application.get("app_files") or [app_path]
    xcode_developer_dir = application.get("xcode_developer_dir") or ""
    tools = _resolve_apple_thinning_tools(xcode_developer_dir)

    staged_app = declare_output("thinning-input/Payload/" + product_name + ".app")
    adapter = declare_output("apple-thinning-package.rb")
    packages = declare_output("ipas")
    manifest = declare_output("thinned-packages.json")
    scratch = ctx["scratch_dir"] + "/apple-thinning"
    variants = scratch + "/variants"
    report = scratch + "/ipatool-report.json"

    copy_path(
        app_path,
        staged_app,
        kind = "tree",
        inputs = app_files,
        toolchain_identity = "once.apple.thinning.input.v1",
        identifier = "apple_thinned_package_stage:" + ctx["label"]["id"],
    )
    write_path(adapter, _apple_thinning_adapter())
    run_action(
        argv = [
            tools["ruby"],
            adapter,
            tools["ipatool"],
            tools["codesign"],
            tools["zip"],
            _parent_dir(_parent_dir(staged_app)),
            device_model,
            product_name,
            ctx["label"]["id"],
            variants,
            report,
            packages,
            manifest,
            tools["toolchain_dir"],
            tools["platforms_dir"],
        ],
        inputs = [adapter, staged_app],
        outputs = [packages, manifest],
        clean_paths = [scratch, packages, manifest],
        create_dirs = [scratch],
        env = tools["env"],
        toolchain_identity = tools["identity"] + "\x00device\x00" + device_model,
        identifier = "apple_thinned_package:" + ctx["label"]["id"],
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "apple_thinned_package",
        "device_model": device_model,
        "ipa_directory": packages,
        "manifest": manifest,
    }

def _apple_test_bundle_impl(ctx):
    attrs = _resolve_attrs(ctx, ctx["attr"], ctx["label"]["id"], ["product_name"])
    _reject_unsupported_attrs(attrs, ctx["label"]["id"], ["test_host", "entitlements", "destination", "test_plan"])
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    product_name = attrs.get("product_name") or ctx["label"]["name"]
    bundle_id = attrs.get("bundle_id") or "dev.once.tests." + product_name
    # The Swift module name must be a valid identifier, while the product name
    # can contain spaces (a bundle named "Alamofire macOS Tests" has module name
    # "Alamofire_macOS_Tests"), so derive the module name separately.
    module_name = _apple_swift_module_name(attrs.get("module_name") or product_name)
    swift_flags = attrs.get("swift_flags") or []
    clang_flags = attrs.get("clang_flags") or []
    per_source_clang_flags = attrs.get("per_source_clang_flags") or {}
    defines = attrs.get("defines") or []
    swift_defines = _unique(defines + (attrs.get("swift_defines") or []))
    clang_defines = _unique(defines + (attrs.get("clang_defines") or []))
    sdk_frameworks = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    bridging_header = attrs.get("bridging_header") or ""
    prefix_header = attrs.get("prefix_header") or ""
    private_header_dirs = []
    for header_dir in (attrs.get("exported_header_dirs") or []) + (attrs.get("private_header_dirs") or []):
        resolved_header_dir = _package_relative(ctx, header_dir)
        absolute_header_dir = resolved_header_dir if resolved_header_dir.startswith("/") else workspace_root() + "/" + resolved_header_dir
        if resolved_header_dir and host_path_exists(absolute_header_dir) and resolved_header_dir not in private_header_dirs:
            private_header_dirs.append(resolved_header_dir)
    private_header_files = _apple_header_inputs(ctx, private_header_dirs)
    swift_testing = attrs.get("swift_testing") or False
    ui_testing = attrs.get("ui_testing") or False
    test_env = attrs.get("test_env") or {}
    test_arguments = attrs.get("test_arguments") or []
    skipped_tests = attrs.get("skipped_tests") or []
    labels = attrs.get("labels") or []
    resources = attrs.get("resources") or []
    structured_resources = attrs.get("structured_resources") or []
    resource_bundle_name = attrs.get("resource_bundle_name") or ""
    resource_bundle_id = attrs.get("resource_bundle_id") or ""
    info_plist_template = attrs.get("info_plist") or ""
    info_plist_substitutions = attrs.get("info_plist_substitutions") or {}

    generated_srcs = _apple_run_prebuild_actions(ctx, attrs)
    all_srcs = _unique(glob(ctx["srcs"]) + _apple_declared_source_paths(ctx) + generated_srcs)
    swift_srcs = _filter_swift_sources(all_srcs)
    objc_srcs = _filter_objc_sources(all_srcs)
    c_srcs = _filter_c_sources(all_srcs)
    cxx_srcs = _filter_cxx_sources(all_srcs)
    assembly_srcs = _filter_assembly_sources(all_srcs)
    if len(swift_srcs) == 0 and len(objc_srcs) == 0 and len(c_srcs) == 0 and len(cxx_srcs) == 0 and len(assembly_srcs) == 0:
        fail("apple_test_bundle " + ctx["label"]["id"] + " has no compilable sources")

    test_dir = ctx["build_dir"] + "/test"
    results = test_dir + "/test_results.json"
    log = test_dir + "/swift-testing.log" if swift_testing else test_dir + "/xctest.log"
    native_results = test_dir + "/native_results.txt"
    action_env = {"HOME": test_dir + "/home"}
    requested_simulator = host_env("ONCE_APPLE_SIMULATOR_UDID")
    if requested_simulator:
        action_env["ONCE_APPLE_SIMULATOR_UDID"] = requested_simulator
    for key in test_env:
        action_env[key] = test_env[key]
    arch = host_arch()
    swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant, arch, False)
    for key in swiftc["env"]:
        action_env[key] = swiftc["env"][key]

    # XCTest lives in the platform's developer-frameworks tree, not the
    # SDK's default search path. With a developer dir we resolve it
    # directly; otherwise we ask xcrun for `--show-sdk-platform-path`.
    if xcode_developer_dir:
        platform_path = _developer_platform_path(xcode_developer_dir, swiftc["sdk_name"])
    else:
        xcrun = host_which("xcrun")
        platform_path = host_command([xcrun, "--sdk", swiftc["sdk_name"], "--show-sdk-platform-path"], env = swiftc["env"]).strip()
    xctest_framework_dir = platform_path + "/Developer/Library/Frameworks"
    xctest_usr_lib_dir = platform_path + "/Developer/usr/lib"
    testing_macros_plugin = _swift_testing_macros_plugin(swiftc["swiftc_path"])

    runner_name = product_name + "-Runner"
    runner_bundle_dir = runner_name + ".app"
    runner_bundle_id = bundle_id + ".xctrunner"
    bundle_dir = (runner_bundle_dir + "/PlugIns/" if ui_testing else "") + product_name + ".xctest"
    if platform == "macos" or platform == "macosx":
        test_binary = declare_output(bundle_dir + "/Contents/MacOS/" + product_name)
        info_plist = declare_output(bundle_dir + "/Contents/Info.plist")
        framework_bundle_dir = "Contents/Frameworks"
        framework_loader_path = "@loader_path/../Frameworks"
    else:
        test_binary = declare_output(bundle_dir + "/" + product_name)
        info_plist = declare_output(bundle_dir + "/Info.plist")
        framework_bundle_dir = "Frameworks"
        framework_loader_path = "@loader_path/Frameworks"
    test_bundle_path = ctx["build_dir"] + "/" + bundle_dir
    runner_type = "xcuitest" if ui_testing else ("swift_testing" if swift_testing else "xctest")
    # `xctest` and `simctl` are macOS-only runtime tools. Resolve
    # `xcrun` here so the provider's `command_argv` always carries an
    # absolute path. Outside the test capability the value is
    # informational, but we still emit a resolved path rather than the
    # literal string `xcrun` so consumers don't have to special-case
    # the placeholder.
    runner_xcrun = host_which("xcrun")
    command_argv = [runner_xcrun, "xctest", test_bundle_path]

    deps = _apple_native_deps(ctx)
    _validate_apple_native_deps(deps, ctx["label"]["id"])
    host_applications = [
        dep
        for dep in deps
        if dep.get("target_kind") == "apple_application" and not dep.get("application_extension") and dep.get("app_executable")
    ]
    if len(host_applications) > 1:
        fail(ctx["label"]["id"] + ": test bundle has more than one host application executable")
    host_application = host_applications[0] if host_applications else {}
    host_executable = "" if ui_testing else (host_application.get("app_executable") or "")
    host_link_archives = [] if ui_testing else (host_application.get("host_link_archives") or [])
    (
        compile_swiftmodule_dirs,
        compile_header_dirs,
        dep_modulemaps,
        dep_hmaps,
        dep_archives,
        framework_search_dirs,
        framework_module_names,
        dep_framework_files,
        dep_sdk_frameworks,
        dep_sdk_dylibs,
        dep_linkopts,
        dep_vfs_overlays,
        plugin_dylibs,
        plugin_executables,
    ) = _collect_dep_compile_inputs(deps, ctx["build_dir"])
    dep_archives = [archive for archive in dep_archives if archive not in host_link_archives]
    alwayslink_archives = _apple_collect_alwayslink_archives(deps)
    runtime_framework_bundles = _apple_collect_runtime_framework_bundles(deps)
    runner_xcodebuild = ""
    xctestrun_file = ""
    if ui_testing and not host_application:
        fail(ctx["label"]["id"] + ": user-interface tests require one application target under test")
    if ui_testing and (platform == "macos" or platform == "macosx" or sdk_variant != "simulator"):
        fail(ctx["label"]["id"] + ": user-interface test execution currently supports simulator targets")
    runner_application_path = ctx["build_dir"] + "/" + runner_bundle_dir if ui_testing else ""
    if ctx["capability"] == "test" and host_application and sdk_variant == "simulator" and platform != "macos" and platform != "macosx":
        runner_xcodebuild = host_which("xcodebuild")
        xctestrun_file = declare_output("runner/tests.xctestrun")
        write_path(
            xctestrun_file,
            _apple_ui_xctestrun(module_name, test_bundle_path, runner_application_path, runner_bundle_id, host_application, xctest_framework_dir, xctest_usr_lib_dir, test_env, test_arguments, skipped_tests) if ui_testing else _apple_hosted_xctestrun(module_name, test_bundle_path, host_application, xctest_framework_dir, xctest_usr_lib_dir, test_env, test_arguments, skipped_tests),
        )
        command_argv = [runner_xcodebuild, "test-without-building", "-xctestrun", xctestrun_file]
    affected_inputs = list(all_srcs)
    for path in resources + structured_resources + (attrs.get("asset_catalogs") or []):
        resolved = _package_relative(ctx, path)
        if resolved not in affected_inputs:
            affected_inputs.append(resolved)
    if info_plist_template:
        resolved = _package_relative(ctx, info_plist_template)
        if resolved not in affected_inputs:
            affected_inputs.append(resolved)
    provider = {
        "label_id": ctx["label"]["id"],
        "test_bundle_path": test_bundle_path,
        "affected_inputs": affected_inputs,
        "test_info": _apple_test_info(ctx, runner_type, command_argv, action_env, labels, results, log, native_results),
    }

    # An XCTest bundle is a Mach-O loadable bundle; swiftc takes
    # `-emit-library` and the linker `-bundle` flag is plumbed through
    # `-Xlinker`. The XCTest framework lives under the platform's
    # `Developer/Library/Frameworks`; add it to both the framework
    # search path (`-F`) and the dyld rpath so the test runner can
    # load it at simulator launch time.
    swift_argv = list(swiftc["argv"]) + [
        "-emit-library",
        "-module-name",
        module_name,
        "-target",
        triple,
        "-working-directory",
        ".",
        "-parse-as-library",
        "-Xlinker",
        "-bundle",
        "-F",
        xctest_framework_dir,
        "-I",
        xctest_usr_lib_dir,
        "-L",
        xctest_usr_lib_dir,
        "-Xlinker",
        "-rpath",
        "-Xlinker",
        xctest_framework_dir,
        "-Xlinker",
        "-rpath",
        "-Xlinker",
        xctest_usr_lib_dir,
        "-Xlinker",
        "-rpath",
        "-Xlinker",
        framework_loader_path,
        "-framework",
        "XCTest",
        "-lXCTestSwiftSupport",
        "-o",
        test_binary,
    ]
    if swift_testing:
        swift_argv.extend([
            "-framework",
            "Testing",
            "-load-plugin-library",
            testing_macros_plugin,
        ])
    if host_executable:
        swift_argv.extend([
            "-Xlinker",
            "-bundle_loader",
            "-Xlinker",
            host_executable,
        ])
    if bridging_header and len(swift_srcs) > 0:
        swift_argv.extend(["-import-objc-header", _package_relative(ctx, bridging_header)])
    for d in compile_swiftmodule_dirs:
        swift_argv.extend(["-I", d])
    for hdir in compile_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for hdir in private_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for mmap in dep_modulemaps:
        swift_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
    for hmap in dep_hmaps:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
    for overlay in dep_vfs_overlays:
        swift_argv.extend(["-Xcc", "-ivfsoverlay", "-Xcc", overlay])
    for d in framework_search_dirs:
        swift_argv.extend(["-F", d])
    _apple_disable_static_framework_autolinking(swift_argv, _apple_collect_link_framework_bundles(deps))
    for fw in framework_module_names:
        swift_argv.extend(["-framework", fw])
    for fw in dep_sdk_frameworks:
        swift_argv.extend(["-framework", fw])
    for fw in sdk_frameworks:
        if fw not in dep_sdk_frameworks:
            swift_argv.extend(["-framework", fw])
    for fw in weak_sdk_frameworks:
        _apple_append_weak_framework(swift_argv, fw)
    for dy in dep_sdk_dylibs:
        swift_argv.extend(["-l" + dy])
    for dy in sdk_dylibs:
        if dy not in dep_sdk_dylibs:
            swift_argv.extend(["-l" + dy])
    for opt in _apple_unique_linkopts(linkopts + dep_linkopts):
        swift_argv.append(opt)
    _apple_add_swift_plugin_args(swift_argv, plugin_dylibs, plugin_executables)
    for define in swift_defines:
        swift_argv.extend(["-D", define])
    for define in clang_defines:
        swift_argv.extend(["-Xcc", "-D" + define])
    for flag in _apple_swift_link_flags(swift_flags):
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    _apple_append_archives(swift_argv, dep_archives, alwayslink_archives)

    swift_inputs = list(swift_srcs)
    if host_executable:
        swift_inputs.append(host_executable)
    if bridging_header and len(swift_srcs) > 0:
        swift_inputs.append(_package_relative(ctx, bridging_header))
    for header in private_header_files:
        if header not in swift_inputs:
            swift_inputs.append(header)
    for mmap in dep_modulemaps:
        if mmap not in swift_inputs:
            swift_inputs.append(mmap)
    for hmap in dep_hmaps:
        if hmap not in swift_inputs:
            swift_inputs.append(hmap)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)
    for f in dep_framework_files:
        if f not in swift_inputs:
            swift_inputs.append(f)
    for overlay in dep_vfs_overlays:
        if overlay not in swift_inputs:
            swift_inputs.append(overlay)
    for plugin_input in _apple_swift_plugin_inputs(plugin_dylibs, plugin_executables):
        if plugin_input not in swift_inputs:
            swift_inputs.append(plugin_input)
    for path in _apple_link_option_inputs(linkopts + dep_linkopts):
        if path not in swift_inputs:
            swift_inputs.append(path)

    clang_objects = []
    if len(objc_srcs) > 0 or len(c_srcs) > 0 or len(cxx_srcs) > 0 or len(assembly_srcs) > 0:
        clang = _resolve_clang(platform, sdk_variant, xcode_developer_dir)

        def compile_test_source(src, language):
            is_assembly = language == "assembler-with-cpp"
            sanitised = src.replace("/", "_")
            obj = declare_output("Objects/" + sanitised + ".o")
            argv = [
                clang["clangxx_path"] if language == "c++" or language == "objective-c++" else clang["clang_path"],
                "-c",
                "-x",
                language,
                "-arch",
                arch,
                "-isysroot",
                clang["sdk_path"],
                "-target",
                triple,
                "-F",
                xctest_framework_dir,
                "-I",
                xctest_usr_lib_dir,
                "-o",
                obj,
            ]
            if not is_assembly:
                argv.extend(["-fmodules", "-fmodule-name=" + module_name])
            if language == "objective-c" or language == "objective-c++":
                argv.append("-fobjc-arc")
            if prefix_header:
                argv.extend(["-include", _package_relative(ctx, prefix_header)])
            for hdir in compile_header_dirs + private_header_dirs:
                argv.extend(["-I", hdir])
            if not is_assembly:
                for mmap in dep_modulemaps:
                    argv.append("-fmodule-map-file=" + mmap)
            for hmap in dep_hmaps:
                argv.extend(["-I", hmap])
            for overlay in dep_vfs_overlays:
                argv.extend(["-ivfsoverlay", overlay])
            for framework_dir in framework_search_dirs:
                argv.extend(["-F", framework_dir])
            for define in clang_defines:
                argv.append("-D" + define)
            for flag in clang_flags:
                if is_assembly and flag.startswith("-std="):
                    continue
                if language != "c++" and language != "objective-c++" and flag.startswith("-std=c++"):
                    continue
                argv.append(flag)
            for flag in json_decode(per_source_clang_flags.get(src) or "[]"):
                if is_assembly and flag.startswith("-std="):
                    continue
                if language != "c++" and language != "objective-c++" and flag.startswith("-std=c++"):
                    continue
                argv.append(flag)
            argv.append(src)

            inputs = [src]
            if bridging_header:
                inputs.append(_package_relative(ctx, bridging_header))
            if prefix_header:
                inputs.append(_package_relative(ctx, prefix_header))
            for header in private_header_files:
                if header not in inputs:
                    inputs.append(header)
            for mmap in dep_modulemaps:
                if mmap not in inputs:
                    inputs.append(mmap)
            for hmap in dep_hmaps:
                if hmap not in inputs:
                    inputs.append(hmap)
            for file in dep_framework_files:
                if file not in inputs:
                    inputs.append(file)
            for overlay in dep_vfs_overlays:
                if overlay not in inputs:
                    inputs.append(overlay)
            run_action(
                argv = argv,
                inputs = inputs,
                outputs = [obj],
                env = clang["env"],
                toolchain_identity = clang["identity"],
                identifier = "apple_test_bundle_clang_compile_" + module_name + "_" + sanitised,
            )
            clang_objects.append(obj)

        for src in objc_srcs:
            compile_test_source(src, "objective-c++" if src.endswith(".mm") else "objective-c")
        for src in c_srcs:
            compile_test_source(src, "c")
        for src in cxx_srcs:
            compile_test_source(src, "c++")
        for src in assembly_srcs:
            compile_test_source(src, "assembler-with-cpp")

    for obj in clang_objects:
        swift_argv.append(obj)
        if obj not in swift_inputs:
            swift_inputs.append(obj)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [test_binary],
        env = swiftc["env"],
        toolchain_identity = swiftc["identity"],
        identifier = "apple_test_bundle_compile_" + module_name,
    )

    if info_plist_template:
        info_plist_source = _package_relative(ctx, info_plist_template)
        info_plist_path = info_plist_source if info_plist_source.startswith("/") else workspace_root() + "/" + info_plist_source
        info_plist_contents = host_file_read(info_plist_path)
        for key, value in info_plist_substitutions.items():
            info_plist_contents = info_plist_contents.replace("$(" + key + ")", value).replace("${" + key + "}", value)
        write_path(info_plist, info_plist_contents)
    else:
        plist_entries = {
            "CFBundleDevelopmentRegion": "en",
            "CFBundleExecutable": product_name,
            "CFBundleIdentifier": bundle_id,
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": product_name,
            "CFBundlePackageType": "BNDL",
            "CFBundleShortVersionString": "1.0",
            "CFBundleVersion": "1",
            "MinimumOSVersion": minimum_os,
        }
        write_path(info_plist, _render_plist(plist_entries, {"XCTContainsUITests": True} if ui_testing else {}))

    resource_destination = bundle_dir + "/Contents/Resources" if platform == "macos" or platform == "macosx" else bundle_dir
    resource_bundle = None
    if resource_bundle_name:
        resource_bundle = _apple_create_resource_bundle(
            ctx,
            resources,
            structured_resources,
            resource_bundle_name,
            resource_bundle_id or ("dev.once." + module_name + ".resources"),
            platform,
            minimum_os,
            xcode_developer_dir,
            module_name,
        )
        resource_files = resource_bundle["files"]
    else:
        resource_files = _apple_materialize_resources(
            ctx,
            resources,
            resource_destination,
            platform,
            minimum_os,
            xcode_developer_dir,
            module_name,
            "apple_test_bundle_resource_" + module_name,
            structured_resources,
        )

    asset_catalogs = [_package_relative(ctx, catalog) for catalog in (attrs.get("asset_catalogs") or [])]
    asset_files = []
    if asset_catalogs:
        actool = _resolve_actool(xcode_developer_dir)
        asset_car = declare_output(resource_destination + "/Assets.car")
        asset_partial_plist = declare_output("assetcatalog-info.plist")
        run_action(
            argv = [actool["actool_path"]] + asset_catalogs + [
                "--compile",
                ctx["build_dir"] + "/" + resource_destination,
                "--platform",
                _apple_actool_platform(platform, sdk_variant),
                "--minimum-deployment-target",
                minimum_os,
                "--bundle-identifier",
                bundle_id,
                "--output-partial-info-plist",
                asset_partial_plist,
            ],
            inputs = asset_catalogs,
            outputs = [asset_car, asset_partial_plist],
            create_dirs = [ctx["build_dir"] + "/" + resource_destination],
            env = actool["env"],
            toolchain_identity = actool["identity"],
            identifier = "apple_test_bundle_assets_" + module_name,
        )
        asset_files.extend([asset_car, asset_partial_plist])

    codesign = _resolve_codesign(xcode_developer_dir)
    embedded_frameworks = _apple_embed_framework_bundles(
        ctx,
        deps,
        bundle_dir,
        framework_bundle_dir,
        codesign,
        "apple_test_bundle_embed",
    )
    embedded_resource_bundles = _apple_embed_resource_bundles(
        ctx,
        deps,
        resource_destination,
        codesign,
        "apple_test_bundle_embed_resource",
        [resource_bundle] if resource_bundle != None else [],
    )
    if platform == "macos" or platform == "macosx":
        test_cs_stamp = declare_output(bundle_dir + "/Contents/_CodeSignature/CodeResources")
    else:
        test_cs_stamp = declare_output(bundle_dir + "/_CodeSignature/CodeResources")
    test_codesign_inputs = [test_binary, info_plist]
    test_codesign_inputs.extend(resource_files)
    test_codesign_inputs.extend(asset_files)
    test_codesign_inputs.extend(embedded_frameworks["stamps"])
    test_codesign_inputs.extend(embedded_resource_bundles["stamps"])
    run_action(
        argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none", test_bundle_path],
        inputs = test_codesign_inputs,
        outputs = [test_binary, test_cs_stamp],
        env = codesign["env"],
        toolchain_identity = codesign["identity"],
        identifier = "apple_test_bundle_codesign_" + module_name,
    )

    runner_files = []
    if ui_testing:
        runner_source = platform_path + "/Developer/Library/Xcode/Agents/XCTRunner.app"
        runner_template = declare_output("runner-template/XCTRunner.app")
        materialize_host_tree(runner_source, runner_template)
        runner_executable = declare_output(runner_bundle_dir + "/" + runner_name)
        runner_pkg_info = declare_output(runner_bundle_dir + "/PkgInfo")
        runner_info_plist = declare_output(runner_bundle_dir + "/Info.plist")
        copy_path(
            runner_template + "/XCTRunner",
            runner_executable,
            inputs = [runner_template],
            identifier = "apple_ui_test_runner_executable_" + module_name,
        )
        copy_path(
            runner_template + "/PkgInfo",
            runner_pkg_info,
            inputs = [runner_template],
            identifier = "apple_ui_test_runner_pkg_info_" + module_name,
        )
        runner_info_contents = host_command([host_which("plutil"), "-convert", "xml1", "-o", "-", runner_source + "/Info.plist"])
        runner_info_contents = runner_info_contents.replace("$(WRAPPEDPRODUCTNAME)", runner_name)
        runner_info_contents = runner_info_contents.replace("$(WRAPPEDPRODUCTBUNDLEIDENTIFIER)", runner_bundle_id)
        write_path(runner_info_plist, runner_info_contents)
        runner_files.extend([runner_executable, runner_pkg_info, runner_info_plist])

        private_framework_dir = platform_path + "/Developer/Library/PrivateFrameworks"
        runner_frameworks = [
            [xctest_framework_dir + "/XCTest.framework", "XCTest.framework"],
            [xctest_framework_dir + "/XCUIAutomation.framework", "XCUIAutomation.framework"],
            [xctest_framework_dir + "/Testing.framework", "Testing.framework"],
            [private_framework_dir + "/XCUnit.framework", "XCUnit.framework"],
            [private_framework_dir + "/XCTAutomationSupport.framework", "XCTAutomationSupport.framework"],
            [private_framework_dir + "/XCTestCore.framework", "XCTestCore.framework"],
            [private_framework_dir + "/XCTestSupport.framework", "XCTestSupport.framework"],
        ]
        for source, name in runner_frameworks:
            destination = declare_output(runner_bundle_dir + "/Frameworks/" + name)
            materialize_host_tree(source, destination)
            runner_files.append(destination)
        swift_support = declare_output(runner_bundle_dir + "/Frameworks/libXCTestSwiftSupport.dylib")
        materialize_host_file(xctest_usr_lib_dir + "/libXCTestSwiftSupport.dylib", swift_support)
        runner_files.append(swift_support)

        runner_cs_stamp = declare_output(runner_bundle_dir + "/_CodeSignature/CodeResources")
        runner_codesign_inputs = list(runner_files)
        runner_codesign_inputs.extend([test_binary, info_plist, test_cs_stamp])
        runner_codesign_inputs.extend(resource_files)
        runner_codesign_inputs.extend(asset_files)
        runner_codesign_inputs.extend(embedded_frameworks["stamps"])
        runner_codesign_inputs.extend(embedded_resource_bundles["stamps"])
        run_action(
            argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none", runner_application_path],
            inputs = runner_codesign_inputs,
            outputs = [runner_executable, runner_cs_stamp],
            env = codesign["env"],
            toolchain_identity = codesign["identity"],
            identifier = "apple_ui_test_runner_codesign_" + module_name,
        )
        runner_files.append(runner_cs_stamp)

    if ctx["capability"] == "test":
        cases_file = test_dir + "/cases.jsonl"
        # When a shard runs, the planner passes the batch's unit ids as
        # `ctx["test"]["filters"]`. Each id is `<target>::<Suite>/<method>`, so
        # dropping the `<target>::` prefix yields the `-XCTest` selector. With no
        # filters the runner selects `All`, matching a plain full-bundle run.
        case_filters = (ctx.get("test") or {}).get("filters") or []
        selector_prefix = ctx["label"]["id"] + "::"
        selectors = []
        for case_filter in case_filters:
            if case_filter.startswith(selector_prefix):
                selectors.append(case_filter[len(selector_prefix):])
            else:
                selectors.append(case_filter)
        xctest_spec = ",".join(selectors) if selectors else "All"
        if platform == "macos" or platform == "macosx":
            runner_command = """DYLD_LIBRARY_PATH={usr_lib}${{DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}} DYLD_FALLBACK_FRAMEWORK_PATH={frameworks}${{DYLD_FALLBACK_FRAMEWORK_PATH:+:$DYLD_FALLBACK_FRAMEWORK_PATH}} {command}""".format(
                usr_lib = _shell_literal(xctest_usr_lib_dir),
                frameworks = _shell_literal(xctest_framework_dir),
                command = _shell_words([runner_xcrun, "xctest", "-XCTest", xctest_spec, test_bundle_path]),
            )
        elif sdk_variant == "simulator":
            simulator_setup = _ios_simulator_selection_script(runner_xcrun) + """
{xcrun} simctl boot "$simulator_id" >/dev/null 2>&1 || true
{xcrun} simctl bootstatus "$simulator_id" -b
""".format(xcrun = _shell_literal(runner_xcrun))
            if host_application:
                hosted_argv = [runner_xcodebuild, "test-without-building", "-xctestrun", xctestrun_file]
                for skipped_test in skipped_tests:
                    hosted_argv.append("-skip-testing:" + module_name + "/" + skipped_test)
                for selector in selectors:
                    hosted_argv.append("-only-testing:" + module_name + "/" + selector)
                install_application = _apple_ui_test_install_script(runner_xcrun, host_application.get("bundle_id") or "", host_application["app_path"]) if ui_testing else ""
                runner_command = simulator_setup + install_application + """
{command} -destination "id=$simulator_id"
""".format(command = _shell_words(hosted_argv))
            else:
                runner_command = simulator_setup + """
tmpdir=$(mktemp -d "${{TMPDIR:-/tmp}}/once-xctest.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT
cp -R {bundle} "$tmpdir/"
find "$tmpdir/{bundle_name}" -type d -exec chmod 755 {{}} +
find "$tmpdir/{bundle_name}" -type f -exec chmod 644 {{}} +
chmod 755 "$tmpdir/{bundle_name}/{binary_name}"
SIMCTL_CHILD_DYLD_LIBRARY_PATH={usr_lib} SIMCTL_CHILD_DYLD_FALLBACK_FRAMEWORK_PATH={frameworks} {xcrun} simctl spawn "$simulator_id" {xctest_agent} -XCTest {xctest_spec} "$tmpdir/{bundle_name}"
""".format(
                xcrun = _shell_literal(runner_xcrun),
                bundle = _shell_literal(test_bundle_path),
                bundle_name = bundle_dir,
                binary_name = product_name,
                usr_lib = _shell_literal(xctest_usr_lib_dir),
                frameworks = _shell_literal(xctest_framework_dir),
                xctest_agent = _shell_literal(platform_path + "/Developer/Library/Xcode/Agents/xctest"),
                xctest_spec = _shell_literal(xctest_spec),
            )
        else:
            fail(ctx["label"]["id"] + ": apple_test_bundle execution supports macos and simulator targets; device runners need xctestrun support")
        script = """set -eu
mkdir -p "$HOME"
log={log}
results={results}
native_results={native_results}
: > "$native_results"
set +e
(
{runner_command}
) > "$log" 2>&1
status=$?
set -e
cp "$log" "$native_results"
{cases_script}
if [ "$status" -eq 0 ]; then run_status=passed; failed=0; passed=$total; else run_status=failed; failed=1; passed=0; fi
{{
  printf '{{"schema":"once.test_results.v1","target":"%s","runner":{{"type":"%s","metadata":{{}}}},"status":"%s","summary":{{"total":%s,"passed":%s,"failed":%s,"skipped":0,"flaky":0}},"cases":[' "{target}" "{runner_type}" "$run_status" "$total" "$passed" "$failed"
  cat "$cases_file"
  printf '],"artifacts":{{"logs":["%s"],"native_results":["%s"]}}}}\n' "$log" "$native_results"
}} > "$results"
exit "$status"
""".format(
            test_dir = _shell_literal(test_dir),
            log = _shell_literal(log),
            results = _shell_literal(results),
            native_results = _shell_literal(native_results),
            runner_command = runner_command,
            cases_script = _apple_test_cases_script(swift_srcs, cases_file, ctx["label"]["id"], runner_type, selectors),
            target = ctx["label"]["id"],
            runner_type = runner_type,
        )
        test_inputs = [test_binary, info_plist, test_cs_stamp]
        test_inputs.extend(resource_files)
        test_inputs.extend(asset_files)
        test_inputs.extend(runner_files)
        if xctestrun_file:
            test_inputs.append(xctestrun_file)
        for app_file in host_application.get("app_files") or []:
            if app_file not in test_inputs:
                test_inputs.append(app_file)
        test_inputs.extend(embedded_frameworks["paths"])
        test_inputs.extend(embedded_resource_bundles["paths"])
        for src in swift_srcs:
            if src not in test_inputs:
                test_inputs.append(src)
        prepare_path(test_dir, kind = "directory", identifier = "apple_" + runner_type + "_test_dir:" + ctx["label"]["id"])
        run_action(
            argv = [host_which("sh"), "-c", script],
            inputs = test_inputs,
            outputs = [test_dir, results, log, native_results],
            env = action_env,
            cacheable = False,
            toolchain_identity = "once.apple." + runner_type + ".runner.v2\x00" + swiftc["identity"],
            identifier = "apple_" + runner_type + ":" + ctx["label"]["id"],
        )

    return provider

def _swiftpm_sanitize(value):
    lowered = value.lower()
    out = ""
    previous_dash = False
    for index in range(len(lowered)):
        character = lowered[index]
        allowed = character in "abcdefghijklmnopqrstuvwxyz0123456789"
        if allowed:
            out += character
            previous_dash = False
        elif out and not previous_dash:
            out += "-"
            previous_dash = True
    if _ends_with(out, "-"):
        out = out[:len(out) - 1]
    return out or "package"

def _swiftpm_normalized_pin(raw):
    state = raw.get("state") or {}
    identity = raw.get("identity") or raw.get("package") or ""
    location = raw.get("location") or raw.get("repositoryURL") or ""
    if not identity and location:
        identity = _basename(location)
        if _ends_with(identity, ".git"):
            identity = identity[:len(identity) - 4]
    if not identity:
        fail("Package.resolved contains a pin without an identity or location")
    return {
        "identity": identity.lower(),
        "kind": raw.get("kind") or "remoteSourceControl",
        "location": location,
        "version": state.get("version") or "",
        "revision": state.get("revision") or "",
        "branch": state.get("branch") or "",
        "checksum": state.get("checksum") or raw.get("checksum") or "",
    }

def _swiftpm_resolved_pins(document):
    version = document.get("version")
    if version == 1:
        raw_pins = (document.get("object") or {}).get("pins") or []
    elif version == 2 or version == 3:
        raw_pins = document.get("pins") or []
    else:
        fail("unsupported Package.resolved schema version `" + str(version) + "`; expected 1, 2, or 3")
    pins = []
    seen = {}
    for raw in raw_pins:
        pin = _swiftpm_normalized_pin(raw)
        identity = pin["identity"]
        if identity in seen:
            fail("Package.resolved contains duplicate identity `" + identity + "`")
        seen[identity] = True
        pins.append(pin)
    return pins

def _swiftpm_visit_graph_node(node, nodes, visiting, depth):
    identity = (node.get("identity") or node.get("name") or "").lower()
    if not identity:
        fail("Swift package graph contains a dependency without an identity")
    if depth > 1024:
        fail("Swift package graph exceeds the maximum dependency depth of 1024")
    if visiting.get(identity):
        fail("Swift package graph contains a dependency cycle through `" + identity + "`")
    existing = nodes.get(identity)
    if existing:
        return
    visiting[identity] = True
    dependencies = []
    for child in node.get("dependencies") or []:
        child_identity = (child.get("identity") or child.get("name") or "").lower()
        if not child_identity:
            fail("Swift package graph dependency under `" + identity + "` has no identity")
        if child_identity not in dependencies:
            dependencies.append(child_identity)
        _swiftpm_visit_graph_node(child, nodes, visiting, depth + 1)
    visiting.pop(identity)
    nodes[identity] = {
        "identity": identity,
        "name": node.get("name") or identity,
        "location": node.get("url") or "",
        "path": node.get("path") or "",
        "version": node.get("version") or "",
        "dependencies": dependencies,
    }

def _swiftpm_graph_nodes(root):
    nodes = {}
    visiting = {}

    for dependency in root.get("dependencies") or []:
        _swiftpm_visit_graph_node(dependency, nodes, visiting, 1)
    return nodes

def _swiftpm_pin_requires_network(pin):
    kind = (pin.get("kind") or "").lower()
    return kind not in ["local", "localsourcecontrol", "filesystem"]

def _swiftpm_pin_token(pin):
    checksum = pin.get("checksum") or ""
    revision = pin.get("revision") or ""
    version = pin.get("version") or ""
    branch = pin.get("branch") or ""
    if checksum:
        return "checksum-" + _swiftpm_sanitize(checksum[:12])
    if revision:
        return "revision-" + _swiftpm_sanitize(revision[:12])
    if version:
        return "version-" + _swiftpm_sanitize(version)
    if branch:
        return "branch-" + _swiftpm_sanitize(branch)
    return "local"

def _swiftpm_target_name(pin):
    return "swiftpm-" + _swiftpm_sanitize(pin["identity"]) + "-" + _swiftpm_pin_token(pin)

def _swiftpm_workspace_path(path):
    if not path:
        return ""
    root = workspace_root().replace("\\", "/")
    normalized = path.replace("\\", "/")
    if "://" in normalized or normalized.startswith("git@"):
        return ""
    if normalized == ".." or normalized.startswith("../") or "/../" in normalized or normalized.endswith("/.."):
        return ""
    if normalized.startswith("./"):
        normalized = normalized[2:]
    prefix = root + "/"
    if normalized.startswith(prefix):
        return normalized[len(prefix):]
    if normalized == root or normalized.startswith("/"):
        return ""
    if len(normalized) > 2 and normalized[1] == ":":
        return ""
    return normalized

def _swiftpm_graph_target_specs(pins, graph):
    graph_nodes = _swiftpm_graph_nodes(graph)
    pins_by_identity = {}
    for pin in pins:
        pins_by_identity[pin["identity"]] = pin
        node = graph_nodes.get(pin["identity"])
        if node == None:
            fail("Swift package graph is stale relative to Package.resolved; locked package `" + pin["identity"] + "` is missing")
        pin_version = pin.get("version") or ""
        node_version = node.get("version") or ""
        if pin_version and node_version and node_version != "unspecified" and pin_version != node_version:
            fail("Swift package graph version `" + node_version + "` for `" + pin["identity"] + "` does not match Package.resolved version `" + pin_version + "`")

    combined = {}
    for identity, node in graph_nodes.items():
        pin = pins_by_identity.get(identity)
        if pin:
            combined[identity] = dict(pin)
        else:
            local_path = _swiftpm_workspace_path(node.get("location") or node.get("path") or "")
            if not local_path:
                fail("Swift package graph is stale relative to Package.resolved; package `" + identity + "` is neither locked nor a workspace-local dependency")
            combined[identity] = {
                "identity": identity,
                "kind": "localSourceControl",
                "location": local_path,
                "version": "",
                "revision": "",
                "branch": "",
                "checksum": "",
            }

    names = {}
    for identity, pin in combined.items():
        name = _swiftpm_target_name(pin)
        if name in names.values():
            fail("Swift package identities produce duplicate synthetic target `" + name + "`")
        names[identity] = name

    targets = []
    for identity in sorted(combined.keys()):
        pin = combined[identity]
        node = graph_nodes.get(identity) or {}
        deps = []
        for dependency in node.get("dependencies") or []:
            dependency_name = names.get(dependency)
            if dependency_name:
                deps.append("./" + dependency_name)
        checkout_path = _swiftpm_workspace_path(node.get("path") or "")
        targets.append({
            "name": names[identity],
            "kind": "swift_package_pin",
            "deps": deps,
            "attrs": {
                "identity": identity,
                "package_name": node.get("name") or identity,
                "source_kind": pin.get("kind") or "",
                "location": pin.get("location") or node.get("location") or "",
                "version": pin.get("version") or "",
                "revision": pin.get("revision") or "",
                "branch": pin.get("branch") or "",
                "checksum": pin.get("checksum") or "",
                "checkout_path": checkout_path,
            },
        })

    roots = []
    for node in graph.get("dependencies") or []:
        identity = (node.get("identity") or node.get("name") or "").lower()
        name = names.get(identity)
        if name and name not in roots:
            roots.append(name)
    remote_identities = []
    locked_pins = []
    for identity in sorted(combined.keys()):
        pin = combined[identity]
        locked_pins.append("\x1f".join([
            identity,
            pin.get("kind") or "",
            pin.get("location") or "",
            pin.get("version") or "",
            pin.get("revision") or "",
            pin.get("branch") or "",
            pin.get("checksum") or "",
        ]))
        if _swiftpm_pin_requires_network(pin):
            remote_identities.append(identity)
    return {
        "targets": targets,
        "roots": roots,
        "attrs": {
            "resolved_identities": sorted(combined.keys()),
            "_remote_identities": remote_identities,
            "_locked_pins": locked_pins,
        },
    }

def _swiftpm_package_file(attrs, value):
    package_path = attrs.get("package_path") or "."
    if package_path.startswith("./"):
        package_path = package_path[2:]
    if not package_path or package_path == ".":
        return value
    if package_path.endswith("/"):
        package_path = package_path[:-1]
    if value.startswith(package_path + "/"):
        return value
    return package_path + "/" + value

def _swiftpm_manifest_file(attrs):
    return _swiftpm_package_file(attrs, "Package.swift")

def _swiftpm_package_path(ctx, value):
    package = ctx["label"]["package"]
    if not value or value == ".":
        return package or "."
    if value.startswith("/") or (len(value) > 2 and value[1] == ":"):
        fail(ctx["label"]["id"] + ": Swift package paths must be workspace-relative, got `" + value + "`")
    if value.startswith("./"):
        value = value[2:]
    if value == ".." or value.startswith("../") or "/../" in value:
        fail(ctx["label"]["id"] + ": Swift package paths must stay inside the owner package, got `" + value + "`")
    if package:
        return package + "/" + value
    return value

def _swiftpm_build_triple_dir(platform, sdk_variant, arch):
    return _apple_swiftmodule_triple(platform, sdk_variant, arch, False)

def _swiftpm_absolute_package_path(ctx, value):
    path = _swiftpm_package_path(ctx, value)
    if path.startswith("/"):
        return path
    return workspace_root() + "/" + path

def _swiftpm_swift_executable(requested_swift, xcode_developer_dir, swiftc_path):
    if xcode_developer_dir:
        return _xctoolchain_bin(xcode_developer_dir, "swift")
    if not requested_swift or requested_swift == "swift":
        return _parent_dir(swiftc_path) + "/swift"
    swift = _resolve_host_executable(requested_swift)
    if not swift:
        fail("unable to resolve Swift executable `" + requested_swift + "`")
    return swift

def _swift_package_dependencies_resolver(ctx):
    attrs = ctx["attrs"]
    if attrs.get("_lazy_resolution") or False:
        return {"targets": []}
    files = ctx["files"]
    resolved_file = _swiftpm_package_file(attrs, attrs.get("resolved_file") or "Package.resolved")
    resolved_content = files.get(resolved_file)
    if resolved_content == None:
        fail(ctx["label"]["id"] + ": resolver source `" + resolved_file + "` is missing; include it in resolver_inputs, or srcs when resolver_inputs is omitted")
    pins = _swiftpm_resolved_pins(json_decode(resolved_content))
    manifest_file = _swiftpm_manifest_file(attrs)
    manifest_content = files.get(manifest_file)
    if manifest_content == None:
        fail(ctx["label"]["id"] + ": Swift package manifest `" + manifest_file + "` is missing; include it in resolver_inputs, or srcs when resolver_inputs is omitted")

    graph_file_value = attrs.get("graph_file") or ""
    graph_file = _swiftpm_package_file(attrs, graph_file_value) if graph_file_value else ""
    if graph_file:
        graph_content = files.get(graph_file)
        if graph_content == None:
            fail(ctx["label"]["id"] + ": graph snapshot `" + graph_file + "` is missing; include it in resolver_inputs, or srcs when resolver_inputs is omitted")
        graph_snapshot = json_decode(graph_content)
        if graph_snapshot.get("once_manifest") == None:
            fail(ctx["label"]["id"] + ": Swift package graph snapshot `" + graph_file + "` has no manifest binding")
        if graph_snapshot["once_manifest"] != manifest_content:
            fail(ctx["label"]["id"] + ": Swift package graph snapshot `" + graph_file + "` is stale relative to `" + manifest_file + "`")
        if graph_snapshot.get("once_resolved") == None:
            fail(ctx["label"]["id"] + ": Swift package graph snapshot `" + graph_file + "` has no resolved-file binding")
        if graph_snapshot["once_resolved"] != resolved_content:
            fail(ctx["label"]["id"] + ": Swift package graph snapshot `" + graph_file + "` is stale relative to `" + resolved_file + "`")
        expected_inputs = _resolver_snapshot_inputs(files, [graph_file])
        if graph_snapshot.get("once_inputs") != expected_inputs:
            fail(ctx["label"]["id"] + ": Swift package graph snapshot `" + graph_file + "` input binding does not match resolver_inputs")
    else:
        xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
        platform = attrs.get("platform") or "macos"
        sdk_variant = attrs.get("sdk_variant") or "simulator"
        swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
        swift = _swiftpm_swift_executable(attrs.get("swift") or "swift", xcode_developer_dir, swiftc["swiftc_path"])
        package_path = _swiftpm_absolute_package_path(ctx, attrs.get("package_path") or ".")
        argv = [
            swift,
            "package",
            "--package-path", package_path,
            "--force-resolved-versions",
        ]
        vendor_path = attrs.get("vendor_path") or ""
        allow_network = attrs.get("allow_network") or False
        remote_identities = []
        for pin in pins:
            if _swiftpm_pin_requires_network(pin):
                remote_identities.append(pin["identity"])
        if remote_identities and not vendor_path and not allow_network:
            fail(ctx["label"]["id"] + ": live graph inspection for remote Swift packages requires explicit network permission; check in graph_file or set allow_network = true")
        if vendor_path:
            fail(ctx["label"]["id"] + ": vendor_path requires a checked-in graph_file so graph loading never mutates the vendored Swift Package Manager scratch tree")
        resolver_scratch = workspace_root() + "/.once/tmp/swiftpm-resolver/" + _swiftpm_sanitize(ctx["label"]["id"])
        argv.extend(["--scratch-path", resolver_scratch])
        argv.extend(["show-dependencies", "--format", "json"])
        graph_content = host_command(argv, env = _developer_env(xcode_developer_dir))
    graph = graph_snapshot if graph_file else json_decode(graph_content)
    return _swiftpm_graph_target_specs(pins, graph)

def _swift_package_pin_impl(ctx):
    attrs = ctx["attr"]
    dependencies = []
    for dep in ctx["deps"]:
        if dep.get("swift_package_pin"):
            dependencies.append(dep.get("identity") or dep.get("label_id"))
    return {
        "label_id": ctx["label"]["id"],
        "swift_package_pin": True,
        "identity": attrs["identity"],
        "package_name": attrs.get("package_name") or attrs["identity"],
        "source_kind": attrs.get("source_kind") or "",
        "location": attrs.get("location") or "",
        "version": attrs.get("version") or "",
        "revision": attrs.get("revision") or "",
        "branch": attrs.get("branch") or "",
        "checksum": attrs.get("checksum") or "",
        "checkout_path": attrs.get("checkout_path") or "",
        "dependency_identities": dependencies,
    }

def _swift_package_dependencies_impl(ctx):
    attrs = ctx["attr"]
    platform = attrs.get("platform") or "macos"
    minimum_os = attrs.get("minimum_os") or "13.0"
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    arch = attrs.get("arch") or host_arch()
    configuration = attrs.get("configuration") or "release"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    products = attrs.get("products") or []
    if len(products) == 0:
        fail(ctx["label"]["id"] + ": products must list at least one static Swift package product")

    swiftc = _resolve_swiftc(platform, sdk_variant, xcode_developer_dir)
    swift = _swiftpm_swift_executable(attrs.get("swift") or "swift", xcode_developer_dir, swiftc["swiftc_path"])
    version = host_command([swift, "--version"], env = swiftc["env"]).strip()
    action_env = dict(swiftc["env"])
    action_path = _parent_dir(swiftc["swiftc_path"]) + ":" + _parent_dir(swift) + ":/usr/bin:/bin"
    action_env["PATH"] = action_path
    triple = _apple_triple(platform, minimum_os, sdk_variant, arch, False)
    build_triple_dir = _swiftpm_build_triple_dir(platform, sdk_variant, arch)
    package_path = _swiftpm_package_path(ctx, attrs.get("package_path") or ".")
    all_srcs = glob(ctx["srcs"])
    scratch = declare_output("swiftpm")
    bin_dir = scratch + "/" + build_triple_dir + "/" + configuration
    archives = []
    for product in products:
        archives.append(bin_dir + "/lib" + product + ".a")
    module_dir = bin_dir + "/Modules"
    vendor_path_attr = attrs.get("vendor_path") or ""
    allow_network = attrs.get("allow_network") or False
    remote_identities = attrs.get("_remote_identities") or []
    locked_pins = attrs.get("_locked_pins") or []
    if remote_identities and not vendor_path_attr and not allow_network:
        fail(ctx["label"]["id"] + ": locked remote Swift packages require vendor_path for a network-independent build or allow_network = true for explicit network access")
    build_inputs = list(all_srcs)
    clean_paths = [bin_dir]
    if vendor_path_attr:
        vendor_path = _swiftpm_package_path(ctx, vendor_path_attr)
        copy_path(
            vendor_path,
            scratch,
            kind = "tree",
            inputs = all_srcs,
            toolchain_identity = "once.swiftpm.vendor.v1",
            identifier = "swiftpm_vendor_seed:" + ctx["label"]["id"],
        )
        build_inputs.append(scratch)
        # A tree copy replaces its destination, so removed vendor checkouts cannot persist.

    argv = [
        swift,
        "build",
        "--package-path", package_path,
        "--scratch-path", scratch,
        "--configuration", configuration,
        "--triple", triple,
        "--sdk", swiftc["sdk_path"],
        "--force-resolved-versions",
        "--manifest-cache", "local",
    ]
    if vendor_path_attr or not allow_network:
        argv.append("--disable-dependency-cache")
    build_flags = attrs.get("build_flags") or []
    argv.extend(build_flags)
    run_action(
        argv = argv,
        inputs = build_inputs,
        outputs = archives + [module_dir],
        clean_paths = clean_paths,
        env = action_env,
        toolchain_identity = "once.swiftpm.build.v2\x00" + version + "\x00" + swiftc["identity"] + "\x00" + triple + "\x00" + action_path + "\x00allow_network\x00" + str(allow_network) + "\x00remote\x00" + "\x00".join(remote_identities) + "\x00pins\x00" + "\x00".join(locked_pins),
        identifier = "swiftpm_build:" + ctx["label"]["id"],
    )

    return {
        "label_id": ctx["label"]["id"],
        "swift_package_dependencies": True,
        "resolved_identities": attrs.get("resolved_identities") or [],
        "swiftmodule_dir": module_dir,
        "archive": archives[0],
        "alwayslink": attrs.get("alwayslink") or False,
        "exported_headers": [],
        "exported_header_dirs": [],
        "modulemap": "",
        "hmap": "",
        "transitive_swiftmodule_dirs": [module_dir],
        "transitive_exported_headers": [],
        "transitive_exported_header_dirs": [],
        "transitive_modulemaps": [],
        "transitive_hmaps": [],
        "transitive_archives": archives,
        "transitive_alwayslink_archives": archives if attrs.get("alwayslink") else [],
        "transitive_sdk_frameworks": attrs.get("sdk_frameworks") or [],
        "transitive_weak_sdk_frameworks": attrs.get("weak_sdk_frameworks") or [],
        "transitive_sdk_dylibs": attrs.get("sdk_dylibs") or [],
        "transitive_linkopts": attrs.get("linkopts") or [],
        "transitive_defines": [],
    }

swift_macro = target_kind(
    docs = "Compiles a macOS Swift compiler-plugin executable that consumers load at compile time.",
    impl = _swift_macro_impl,
    attrs = [
        attr("minimum_os", "string", docs = "Minimum macOS version for the host plugin"),
        attr("module_name", "string", docs = "Compiled module name. Defaults to the target name", configurable = False),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
    ],
    deps = [
        dep("deps", ["apple_linkable"], "Libraries the plugin links against (typically a swift-syntax checkout)"),
    ],
    providers = ["apple_swift_plugin"],
    capabilities = [
        capability("build", ["default", "plugin_executable", "swiftmodule"]),
    ],
    examples = [
        example(
            "swift-macro-minimal",
            name = "Minimal Swift macro plugin",
            use_when = "You want a host-loaded Swift compiler plugin target that a library can depend on.",
        ),
    ],
)

swift_package_dependencies = target_kind(
    docs = "Imports Package.resolved and a locked Swift package graph as synthetic package pins, then builds selected static products through Swift Package Manager for Apple consumers.",
    attrs = [
        attr("package_path", "string", default = ".", docs = "Package-relative directory containing Package.swift and Package.resolved.", configurable = False),
        attr("resolved_file", "string", default = "Package.resolved", docs = "Resolver source containing the Swift package lock graph, relative to package_path.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to the resolver. Defaults to srcs when empty or omitted; use this to exclude vendored source and binary files from graph loading.", configurable = False),
        attr("graph_file", "string", docs = "Optional checked-in [JavaScript Object Notation (JSON)](https://www.json.org/json-en.html) output relative to package_path from `swift package show-dependencies --format json`, with exact once_manifest, once_resolved, and once_inputs bindings. When omitted, analysis runs that locked command.", configurable = False),
        attr("vendor_path", "string", docs = "Optional package-relative Swift Package Manager scratch tree containing checkouts, repositories, and workspace state. The build copies it before use.", configurable = False),
        attr("allow_network", "bool", default = "false", docs = "Allow invoking Swift Package Manager for remote packages when vendor_path is absent. Prefer a complete vendor_path; Once does not independently sandbox Swift Package Manager network access.", configurable = False),
        attr("products", "list<string>", default = "[]", docs = "Root and transitive static library product names exposed as Apple linker inputs.", configurable = False),
        attr("platform", "string", default = "macos", docs = "Apple platform used to build the package products.", configurable = False),
        attr("minimum_os", "string", default = "13.0", docs = "Minimum Apple operating system version used in the build triple.", configurable = False),
        attr("sdk_variant", "string", default = "simulator", docs = "Simulator or device software development kit selection. Ignored for macOS.", configurable = False),
        attr("arch", "string", docs = "Single target architecture. Defaults to the execution host architecture.", configurable = False),
        attr("configuration", "string", default = "release", docs = "Swift Package Manager build configuration, either debug or release.", configurable = False),
        attr("swift", "string", default = "swift", docs = "Swift Package Manager executable or workspace-relative executable path. The default selects the executable paired with the resolved Swift compiler.", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode developer directory for Swift and the Apple software development kit.", configurable = False),
        attr("build_flags", "list<string>", default = "[]", docs = "Additional arguments appended to the locked Swift package build.", configurable = False),
        attr("alwayslink", "bool", default = "false", docs = "Force-load every selected static product in downstream Apple links."),
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple software development kit frameworks required by the selected products."),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Weakly linked Apple software development kit frameworks required by the selected products."),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple software development kit dynamic libraries required by the selected products."),
        attr("linkopts", "list<string>", default = "[]", docs = "Additional linker flags propagated with the package products."),
        attr("resolved_identities", "list<string>", default = "[]", docs = "Synthetic package identities populated by the resolver.", configurable = False),
        attr("_remote_identities", "list<string>", default = "[]", docs = "Remote package identities populated by the resolver for execution policy.", configurable = False),
        attr("_locked_pins", "list<string>", default = "[]", docs = "Resolver-owned immutable package state included in native build action identities.", configurable = False),
        attr("_lazy_resolution", "bool", default = "false", docs = "Resolver-owned marker that defers remote package inspection until the build action.", configurable = False),
    ],
    deps = [
        dep("deps", ["swift_package_pin"], "Locked direct package dependencies emitted by the resolver."),
    ],
    providers = ["swift_package_dependencies", "apple_linkable", "apple_module"],
    capabilities = [capability("build", ["default", "binary", "swiftmodule"])],
    tools = [tool("swift", ["swift", "swiftc"])],
    resolver = _swift_package_dependencies_resolver,
    impl = _swift_package_dependencies_impl,
    source_references = [
        source_reference(
            "swift-package-manager",
            "ResolvedPackagesStore",
            "https://github.com/swiftlang/swift-package-manager/blob/main/Sources/PackageGraph/ResolvedPackagesStore.swift",
            "Defines the Package.resolved schemas and locked package state imported by this target kind.",
        ),
        source_reference(
            "swift-package-manager",
            "ShowDependencies",
            "https://github.com/swiftlang/swift-package-manager/blob/main/Sources/Commands/PackageCommands/ShowDependencies.swift",
            "Defines the dependency graph command and JavaScript Object Notation output imported by the resolver.",
        ),
    ],
    examples = [
        example(
            "swift-package-dependencies-minimal",
            name = "Swift package dependencies",
            use_when = "Use this when an Apple target consumes locked Swift package products.",
        ),
    ],
)

swift_package_pin = target_kind(
    docs = "Synthetic locked Swift package identity emitted from Package.resolved. Users depend on swift_package_dependencies instead of declaring pins directly.",
    attrs = [
        attr("identity", "string", required = True, docs = "Canonical lowercase Swift package identity.", configurable = False),
        attr("package_name", "string", docs = "Package display name from the resolved graph.", configurable = False),
        attr("source_kind", "string", docs = "Swift package source kind, such as remoteSourceControl, registry, or localSourceControl.", configurable = False),
        attr("location", "string", docs = "Locked registry, source control, or local package location.", configurable = False),
        attr("version", "string", docs = "Locked semantic version when present.", configurable = False),
        attr("revision", "string", docs = "Locked source control revision when present.", configurable = False),
        attr("branch", "string", docs = "Locked source control branch when present.", configurable = False),
        attr("checksum", "string", docs = "Locked registry checksum when present.", configurable = False),
        attr("checkout_path", "string", docs = "Workspace-relative checkout path reported by Swift Package Manager.", configurable = False),
    ],
    deps = [dep("deps", ["swift_package_pin"], "Locked transitive Swift package dependencies.")],
    providers = ["swift_package_pin"],
    capabilities = [capability("build", [])],
    impl = _swift_package_pin_impl,
    source_references = [
        source_reference(
            "swift-package-manager",
            "ResolvedPackagesStore",
            "https://github.com/swiftlang/swift-package-manager/blob/main/Sources/PackageGraph/ResolvedPackagesStore.swift",
            "Defines the locked identity, version, revision, branch, and checksum represented by this synthetic target.",
        ),
    ],
    examples = [
        example(
            "swift-package-dependencies-minimal",
            name = "Resolved Swift package pin",
            use_when = "Use this when inspecting the synthetic locked package identity emitted by Swift package dependency resolution.",
            path = "examples/swift-package-dependencies-minimal",
        ),
    ],
)

apple_resource_bundle = target_kind(
    docs = "Processes files into a named Apple resource bundle and propagates that bundle to the top-level application.",
    impl = _apple_resource_bundle_target_impl,
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform such as ios, macos, tvos, watchos, or visionos", configurable = False),
        attr("minimum_os", "string", docs = "Minimum supported operating system version"),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` software development kit selection. Ignored on macOS", configurable = False),
        attr("bundle_name", "string", docs = "Bundle name. The `.bundle` suffix is added when omitted", configurable = False),
        attr("bundle_id", "string", docs = "Bundle identifier written to generated metadata", configurable = False),
        attr("resources", "list<string>", default = "[]", docs = "Files and directory roots processed into the bundle"),
        attr("structured_resources", "list<string>", default = "[]", docs = "Resource directory roots whose own basename is preserved inside the bundle"),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
    ],
    deps = [
        dep("deps", ["apple_resource"], "Resource bundles nested in this bundle or propagated alongside it"),
    ],
    providers = ["apple_resource"],
    capabilities = [capability("build", ["default", "bundle"])],
    source_references = [
        source_reference(
            "rules-apple",
            "apple_resource_bundle",
            "https://github.com/bazelbuild/rules_apple/blob/master/apple/internal/resource_rules/apple_resource_bundle.bzl",
            "Defines the resource bundle propagation model mirrored by this target kind.",
        ),
    ],
    examples = [
        example(
            "apple-resource-bundle-minimal",
            name = "Apple resource bundle",
            use_when = "You want to package resources independently and propagate them to an application.",
            path = "examples/apple-resource-bundle-minimal",
        ),
    ],
)

apple_library = target_kind(
    docs = "Compiles Swift, Objective-C, C, and C++ sources into a linkable Apple module.",
    impl = _apple_library_impl,
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform such as ios, macos, tvos, watchos, or visionos", configurable = False),
        attr("minimum_os", "string", docs = "Minimum supported OS version (deployment target)"),
        attr("target_sdk_version", "string", docs = "Build-time SDK version baked into the triple. Defaults to `minimum_os`"),
        attr("module_name", "string", docs = "Compiled module name. Defaults to the target name", configurable = False),
        attr("headers", "list<string>", default = "[]", docs = "Public or private C-family headers compiled with this target"),
        attr("exported_headers", "list<string>", default = "[]", docs = "Headers made available to dependent targets"),
        attr("exported_header_dirs", "list<string>", default = "[]", docs = "Header search directories made available to dependent targets"),
        attr("private_header_dirs", "list<string>", default = "[]", docs = "Header search directories used only while compiling this target"),
        attr("resources", "list<string>", default = "[]", docs = "Files and directory roots placed in this library's propagated resource bundle"),
        attr("structured_resources", "list<string>", default = "[]", docs = "Resource directory roots whose own basename is preserved inside the propagated bundle"),
        attr("resource_bundle_name", "string", docs = "Name of the propagated resource bundle. The `.bundle` suffix is added when omitted", configurable = False),
        attr("resource_bundle_id", "string", docs = "Bundle identifier written to the propagated resource bundle metadata", configurable = False),
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name, such as UIKit or Foundation"),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
        attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags, propagated transitively to consumers"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
        attr("clang_flags", "list<string>", default = "[]", docs = "Extra Clang compiler flags"),
        attr("per_source_clang_flags", "map<string,string>", default = "{}", docs = "[JavaScript Object Notation (JSON)](https://www.json.org/json-en.html)-encoded Clang flag lists keyed by source path", configurable = False),
        attr("defines", "list<string>", default = "[]", docs = "`-D` conditions shared by Swift and Clang for compatibility"),
        attr("swift_defines", "list<string>", default = "[]", docs = "Swift conditional compilation conditions"),
        attr("clang_defines", "list<string>", default = "[]", docs = "C-family preprocessor definitions"),
        attr("enable_testing", "bool", default = "false", docs = "Compile Swift with testability enabled for dependent tests"),
        attr("swift_testing", "bool", default = "false", docs = "Compile sources that import the Swift Testing framework"),
        attr("xctest_support", "bool", default = "false", docs = "Compile sources that import the XCTest framework"),
        attr("library_evolution", "bool", default = "false", docs = "Emit stable Swift module interfaces for binary compatibility"),
        attr("emit_dsym", "bool", default = "false", docs = "Emit DWARF debug info so downstream target kinds can extract a `.dSYM` bundle"),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` SDK selection. Ignored on macOS (always uses macosx)", configurable = False),
        attr("archs", "list<string>", default = "[]", docs = "Target architectures (`arm64`, `x86_64`, `arm64e`, `arm64_32`). Empty defaults to the host arch; multi-arch fans out per-arch compiles and combines them with `lipo`", configurable = False),
        attr("mac_catalyst", "bool", default = "false", docs = "Build the iOSMac (Mac Catalyst) variant. Requires `platform = macos`; rewrites the triple to `<arch>-apple-ios<minOS>-macabi`", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
        attr("alwayslink", "bool", default = "false", docs = "Hint to downstream linker target kinds to force-load this archive (`-Wl,-force_load`)"),
        attr("exported_deps", "list<string>", default = "[]", docs = "Target IDs from `deps` whose module interface flows through to consumers' compile path"),
        attr("bridging_header", "string", docs = "ObjC bridging header that lets Swift sources see ObjC symbols (`-import-objc-header`)"),
        attr("prefix_header", "string", docs = "Prefix header included before every C-family source"),
        attr("prebuild_actions", "list<string>", default = "[]", docs = "Ordered serialized build preparation actions that run before compilation.", configurable = False),
        attr("enable_modules", "bool", default = "false", docs = "Emit a `module.modulemap` for `exported_headers` and pass `-fmodules` to Clang so consumers can `import` the module instead of #importing each header"),
        attr("modulemap", "string", docs = "Authored Clang module map retained instead of synthesizing an umbrella module map"),
        attr("modulemap_headers", "list<string>", default = "[]", docs = "Headers named by the authored module map, including private explicit submodules"),
        attr("auxiliary_modulemaps", "list<string>", default = "[]", docs = "Additional Clang module maps referenced by this module's public Swift interface"),
    ],
    deps = [
        dep("deps", ["apple_linkable", "apple_resource", "apple_swift_plugin", "native_linkable"], "Libraries, frameworks, resources, native linkables, or Swift compiler plugins consumed by this library"),
    ],
    providers = ["apple_linkable", "apple_module"],
    capabilities = [
        capability("build", ["default", "binary", "swiftmodule", "generated_sources"]),
    ],
    examples = [
        example(
            "apple-library-minimal",
            name = "Minimal Apple library",
            use_when = "You want a Swift static library targeting iOS or macOS with no extra resources or mixed-language sources.",
        ),
        example(
            "apple-library-with-objc",
            name = "Apple library with mixed Swift and Objective-C",
            use_when = "Your library exposes Swift APIs that call into an existing Objective-C codebase through a bridging header.",
        ),
    ],
)

apple_xcframework_import = target_kind(
    docs = "Selects a platform and architecture slice from a prebuilt XCFramework and exposes its framework or static library to Apple targets.",
    impl = _apple_xcframework_import_impl,
    attrs = [
        attr("bundle", "string", required = True, docs = "Workspace-relative `.xcframework` bundle"),
        attr("platform", "string", required = True, docs = "Apple platform whose slice is imported", configurable = False),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` slice selection", configurable = False),
        attr("arch", "string", docs = "Target architecture. Defaults to the execution host architecture.", configurable = False),
        attr("module_name", "string", docs = "Framework or Clang module name. Defaults to the selected framework bundle name or static library module map", configurable = False),
    ],
    deps = [dep("deps", ["artifact"], "Optional checksum-pinned archive artifact that materializes the XCFramework before this target is analysed.", max_count = 1)],
    providers = ["apple_linkable", "apple_framework", "apple_bundle"],
    capabilities = [capability("build", ["default", "framework"])],
    examples = [
        example(
            "apple-xcframework-import-minimal",
            name = "Minimal XCFramework import",
            use_when = "You want to link a prebuilt XCFramework for a selected Apple platform and architecture.",
        ),
    ],
)

apple_framework = target_kind(
    docs = "Builds a dynamic Apple framework bundle (`Foo.framework/Foo` dylib) with module metadata and resources.",
    impl = _apple_framework_impl,
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform for the framework", configurable = False),
        attr("minimum_os", "string", docs = "Minimum supported OS version"),
        attr("target_sdk_version", "string", docs = "Build-time SDK version baked into the triple. Defaults to `minimum_os`"),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` SDK selection. Ignored on macOS", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
        attr("bundle_id", "string", docs = "Framework bundle identifier"),
        attr("product_name", "string", docs = "Framework product name. Defaults to the target name", configurable = False),
        attr("module_name", "string", docs = "Swift module name. Defaults to `product_name`"),
        attr("headers", "list<string>", default = "[]", docs = "Headers packaged with the framework"),
        attr("exported_headers", "list<string>", default = "[]", docs = "Headers exported to downstream consumers"),
        attr("exported_header_dirs", "list<string>", default = "[]", docs = "Header search directories made available to dependent targets"),
        attr("private_header_dirs", "list<string>", default = "[]", docs = "Header search directories used only while compiling this target"),
        attr("resources", "list<string>", default = "[]", docs = "Resource glob patterns bundled into the framework"),
        attr("structured_resources", "list<string>", default = "[]", docs = "Resource directory roots whose own basename is preserved inside the framework"),
        attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the framework bundle"),
        attr("privacy_manifest", "string", docs = "Privacy manifest placed in the framework bundle"),
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name"),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
        attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
        attr("clang_flags", "list<string>", default = "[]", docs = "Extra Clang compiler flags"),
        attr("per_source_clang_flags", "map<string,string>", default = "{}", docs = "[JavaScript Object Notation (JSON)](https://www.json.org/json-en.html)-encoded Clang flag lists keyed by source path", configurable = False),
        attr("defines", "list<string>", default = "[]", docs = "Conditional compilation definitions shared by Swift and Clang for compatibility"),
        attr("swift_defines", "list<string>", default = "[]", docs = "Swift conditional compilation conditions"),
        attr("clang_defines", "list<string>", default = "[]", docs = "C-family preprocessor definitions"),
        attr("enable_testing", "bool", default = "false", docs = "Compile Swift with testability enabled for dependent tests"),
        attr("swift_testing", "bool", default = "false", docs = "Compile sources that import the Swift Testing framework"),
        attr("xctest_support", "bool", default = "false", docs = "Compile sources that import the XCTest framework"),
        attr("library_evolution", "bool", default = "false", docs = "Emit stable Swift module interfaces for binary compatibility"),
        attr("emit_dsym", "bool", default = "false", docs = "Emit debug information for symbol bundles"),
        attr("archs", "list<string>", default = "[]", docs = "Target architectures. Empty defaults to the execution host architecture", configurable = False),
        attr("mac_catalyst", "bool", default = "false", docs = "Build the iOSMac variant for Mac Catalyst", configurable = False),
        attr("alwayslink", "bool", default = "false", docs = "Force-load this target's archive into the dynamic framework link"),
        attr("exported_deps", "list<string>", default = "[]", docs = "Dependency target identifiers whose module interfaces flow to consumers"),
        attr("bridging_header", "string", docs = "Objective-C bridging header used by Swift sources"),
        attr("prefix_header", "string", docs = "Prefix header included before every C-family source"),
        attr("prebuild_actions", "list<string>", default = "[]", docs = "Ordered serialized build preparation actions that run before compilation", configurable = False),
        attr("enable_modules", "bool", default = "false", docs = "Emit and consume a Clang module map for exported headers"),
        attr("modulemap", "string", docs = "Authored Clang module map retained in the framework"),
        attr("modulemap_headers", "list<string>", default = "[]", docs = "Headers named by the authored module map, including private explicit submodules"),
        attr("auxiliary_modulemaps", "list<string>", default = "[]", docs = "Additional Clang module maps referenced by this framework's public Swift interface"),
    ],
    deps = [
        dep("deps", ["apple_linkable", "apple_resource", "apple_swift_plugin", "native_linkable"], "Libraries, resources, native linkables, or Swift compiler plugins linked or embedded by the framework"),
    ],
    providers = ["apple_linkable", "apple_framework", "apple_bundle"],
    capabilities = [
        capability("build", ["default", "framework", "dsyms", "swiftmodule"]),
    ],
    examples = [
        example(
            "apple-framework-minimal",
            name = "Minimal Apple framework",
            use_when = "You want a Swift dynamic framework bundle that can be embedded by an application.",
        ),
    ],
)

apple_application = target_kind(
    docs = "Builds an Apple application bundle (`Foo.app`) with the Mach-O executable, embedded frameworks, Info.plist, and ad-hoc codesign.",
    impl = _apple_application_impl,
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform for the application", configurable = False),
        attr("bundle_id", "string", required = True, docs = "Application bundle identifier"),
        attr("minimum_os", "string", docs = "Minimum supported OS version"),
        attr("target_sdk_version", "string", docs = "Build-time SDK version baked into the triple. Defaults to `minimum_os`"),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` SDK selection. Ignored on macOS", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
        attr("families", "list<string>", default = "[]", docs = "Supported device families, such as iphone or ipad"),
        attr("product_name", "string", docs = "Application product name. Defaults to the target name", configurable = False),
        attr("resources", "list<string>", default = "[]", docs = "Resource and asset catalog glob patterns"),
        attr("structured_resources", "list<string>", default = "[]", docs = "Resource directory roots whose own basename is preserved inside the application bundle"),
        attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the application bundle"),
        attr("info_plist", "string", docs = "Info.plist template path"),
        attr("info_plist_substitutions", "map<string,string>", default = "{}", docs = "Values substituted into the generated Info.plist"),
        attr("entitlements", "string", docs = "Entitlements plist path"),
        attr("development_team", "string", docs = "Apple development team identifier used to derive simulator application identity", configurable = False),
        attr("provisioning_profile", "string", docs = "Provisioning profile label or path used for signing"),
        attr("signing_identity", "string", docs = "Local signing identity selector used for development device signing"),
        attr("signing", "string", default = "ad_hoc", docs = "Signing mode or policy name"),
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name"),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
        attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
        attr("clang_flags", "list<string>", default = "[]", docs = "Extra Clang compiler flags applied to C, C++, Objective-C, and Objective-C++ sources"),
        attr("per_source_clang_flags", "map<string,string>", default = "{}", docs = "JSON-encoded Clang compiler flag lists keyed by source path"),
        attr("defines", "list<string>", default = "[]", docs = "Conditional compilation definitions shared by Swift and Clang for compatibility"),
        attr("swift_defines", "list<string>", default = "[]", docs = "Swift conditional compilation conditions"),
        attr("clang_defines", "list<string>", default = "[]", docs = "C-family preprocessor definitions"),
        attr("exported_header_dirs", "list<string>", default = "[]", docs = "Header search directories exported by the application target"),
        attr("private_header_dirs", "list<string>", default = "[]", docs = "Private header search directories used while compiling the application"),
        attr("bridging_header", "string", docs = "ObjC bridging header imported into every Swift source (`-import-objc-header`), letting them see ObjC symbols and any frameworks the header imports"),
        attr("prefix_header", "string", docs = "Prefix header included before every C-family source"),
        attr("entitlements_substitutions", "map<string,string>", default = "{}", docs = "Build-setting values substituted into `$(NAME)` or `${NAME}` placeholders before signing", configurable = False),
        attr("prebuild_actions", "list<string>", default = "[]", docs = "Ordered serialized build preparation actions that run before compilation.", configurable = False),
        attr("application_extension", "bool", default = "false", docs = "Build as an app extension: entered through `NSExtensionMain` and compiled against the app-extension-safe API surface"),
        attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalogs (`.xcassets`) compiled into the bundle's `Assets.car` and used to generate type-safe `ImageResource`/`ColorResource` accessors"),
        attr("app_icon", "string", docs = "Asset catalog app-icon set name (`ASSETCATALOG_COMPILER_APPICON_NAME`) compiled into the app icon"),
        attr("enable_testing", "bool", default = "false", docs = "Compile Swift with testability enabled so hosted test bundles can `@testable import` the application module"),
    ],
    deps = [
        dep("deps", ["apple_linkable", "apple_framework", "apple_resource", "apple_application", "apple_swift_plugin", "native_linkable"], "Libraries, frameworks, resources, application extensions, native linkables, and Swift compiler plugins embedded in the app"),
    ],
    providers = ["apple_application", "apple_bundle"],
    capabilities = [
        capability("build", ["default", "bundle", "dsyms"]),
        capability("run", ["default"], ["bundle"]),
    ],
    examples = [
        example(
            "apple-application-minimal",
            name = "Minimal iOS application",
            use_when = "You want the smallest viable iOS app target wired into a Once workspace.",
        ),
        example(
            "native-mobile-shared-code-e2e",
            name = "Apple app with shared native code",
            use_when = "Use this when an Apple app should embed a Kotlin/Native framework and link a Rust static library.",
        ),
    ],
)

apple_thinned_package = target_kind(
    docs = "Produces an ad-hoc signed, device-specific application archive for Apple application size analysis.",
    impl = _apple_thinned_package_impl,
    attrs = [
        attr(
            "device_model",
            "string",
            required = True,
            docs = "One Apple device model identifier, such as `iPhone17,1`",
            configurable = False,
            disallowed_values = ["", "all"],
        ),
    ],
    deps = [
        dep(
            "deps",
            ["apple_application"],
            "Exactly one device application to thin and package",
            min_count = 1,
            max_count = 1,
        ),
    ],
    providers = ["apple_thinned_package"],
    capabilities = [
        capability("build", ["default", "ipas", "manifest"]),
    ],
    source_references = [
        source_reference(
            "Sentry",
            "Apple application size analysis",
            "https://docs.sentry.io/platforms/apple/guides/ios/size-analysis/#app-thinning",
            "Confirm that device-specific application archives should be created before upload.",
        ),
        source_reference(
            "Apple",
            "Reducing your app's size",
            "https://developer.apple.com/documentation/Xcode/reducing-your-app-s-size",
            "Confirm Apple application thinning behavior and size-analysis guidance.",
        ),
    ],
    examples = [
        example(
            "apple-thinned-package-minimal",
            name = "Device-specific Apple size-analysis package",
            use_when = "You want an application archive thinned for one device model before uploading it for size analysis.",
        ),
    ],
)

apple_test_bundle = target_kind(
    docs = "Builds Apple test targets and can run Swift Testing tests through the generic Once test capability.",
    impl = _apple_test_bundle_impl,
    attrs = [
        attr("platform", "string", required = True, docs = "Apple platform for the tests", configurable = False),
        attr("minimum_os", "string", docs = "Minimum supported OS version"),
        attr("target_sdk_version", "string", docs = "Build-time SDK version baked into the triple. Defaults to `minimum_os`"),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` SDK selection. Ignored on macOS", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
        attr("product_name", "string", docs = "Test bundle product name. Defaults to the target name", configurable = False),
        attr("module_name", "string", docs = "Swift module name. Defaults to product_name", configurable = False),
        attr("bundle_id", "string", docs = "Test bundle identifier. Defaults to `dev.once.tests.<product_name>`", configurable = False),
        attr("test_host", "target", docs = "Application target hosting the test bundle"),
        attr("resources", "list<string>", default = "[]", docs = "Resource glob patterns bundled into the test bundle"),
        attr("structured_resources", "list<string>", default = "[]", docs = "Resource directory roots whose own basename is preserved inside the test bundle"),
        attr("resource_bundle_name", "string", docs = "Optional resource bundle name. The `.bundle` suffix is added when omitted", configurable = False),
        attr("resource_bundle_id", "string", docs = "Bundle identifier written to generated resource bundle metadata", configurable = False),
        attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the test bundle"),
        attr("info_plist", "string", docs = "Info.plist template path"),
        attr("info_plist_substitutions", "map<string,string>", default = "{}", docs = "Build-setting values substituted into `$(NAME)` or `${NAME}` placeholders in the Info.plist template", configurable = False),
        attr("entitlements", "string", docs = "Entitlements plist path"),
        attr("destination", "string", docs = "Simulator, device, or local destination selector"),
        attr("test_plan", "string", docs = "XCTest plan path"),
        attr("test_env", "map<string,string>", default = "{}", docs = "Environment variables passed to the test runner"),
        attr("test_arguments", "list<string>", default = "[]", docs = "Arguments passed to the test process", configurable = False),
        attr("skipped_tests", "list<string>", default = "[]", docs = "Suite or case identifiers excluded from the test run", configurable = False),
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple software development kit frameworks linked by name"),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple software development kit frameworks linked weakly"),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple software development kit dynamic libraries linked by name"),
        attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
        attr("clang_flags", "list<string>", default = "[]", docs = "Extra Clang compiler flags applied to C, C++, Objective-C, and Objective-C++ test sources"),
        attr("per_source_clang_flags", "map<string,string>", default = "{}", docs = "JSON-encoded Clang compiler flag lists keyed by test source path"),
        attr("defines", "list<string>", default = "[]", docs = "Conditional compilation definitions shared by Swift and Clang for compatibility"),
        attr("swift_defines", "list<string>", default = "[]", docs = "Swift conditional compilation conditions"),
        attr("clang_defines", "list<string>", default = "[]", docs = "C-family preprocessor definitions"),
        attr("exported_header_dirs", "list<string>", default = "[]", docs = "Header search directories exported by the test target"),
        attr("private_header_dirs", "list<string>", default = "[]", docs = "Private header search directories used while compiling tests"),
        attr("bridging_header", "string", docs = "Objective-C bridging header imported into Swift test sources"),
        attr("prefix_header", "string", docs = "Prefix header included before every C-family test source"),
        attr("prebuild_actions", "list<string>", default = "[]", docs = "Ordered serialized build preparation actions that run before compilation.", configurable = False),
        attr("swift_testing", "bool", default = "false", docs = "Run sources that use Swift Testing (`import Testing`) through the generic Once test capability"),
        attr("ui_testing", "bool", default = "false", docs = "Package the test bundle in the platform test runner and launch an application under test", configurable = False),
        attr("labels", "list<string>", default = "[]", docs = "Agent-readable labels used for filtering or policy"),
    ],
    deps = [
        dep("deps", ["apple_linkable", "apple_application", "apple_swift_plugin", "native_linkable"], "Code under test, optional host application, native linkables, and Swift compiler plugins"),
    ],
    providers = ["apple_test_bundle", "apple_bundle", "once_test_info"],
    capabilities = [
        capability("build", ["default", "bundle", "dsyms"]),
        capability("test", ["default", "test_results", "coverage"]),
    ],
    examples = [
        example(
            "apple-test-bundle-minimal",
            name = "Minimal Swift Testing bundle",
            use_when = "You want a modern Swift Testing target without a host application.",
        ),
    ],
)

shellspec_test = target_kind(
    docs = "Runs ShellSpec files through the generic Once test capability and emits normalized once.test_results.v1 results.",
    attrs = [
        attr("shellspec", "string", default = "shellspec", docs = "ShellSpec executable to invoke"),
        attr("args", "list<string>", default = "[]", docs = "Additional arguments passed to ShellSpec"),
        attr("env", "map<string,string>", default = "{}", docs = "Environment variables passed to the ShellSpec process"),
        attr("data", "list<string>", default = "[]", docs = "Additional runtime files needed by the specs, such as spec helpers"),
        attr("labels", "list<string>", default = "[]", docs = "Agent-readable labels used for filtering or policy"),
        attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds"),
    ],
    deps = [
        dep("deps", ["script_action", "apple_linkable", "apple_application", "once_test_info"], "Targets whose outputs or source changes should affect this ShellSpec test target"),
    ],
    providers = ["once_test_info"],
    capabilities = [
        capability("test", ["default", "test_results", "logs"]),
    ],
    examples = [
        example(
            "shellspec-test-minimal",
            name = "Minimal ShellSpec test",
            use_when = "Use when modeling shell-based e2e tests that should run through Once's generic test capability.",
        ),
    ],
    impl = _shellspec_test_impl,
)
