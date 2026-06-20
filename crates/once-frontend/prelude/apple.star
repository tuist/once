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

def _xcrun_env(xcode_developer_dir):
    env = {}
    if xcode_developer_dir:
        env["DEVELOPER_DIR"] = xcode_developer_dir
    return env

def _xcrun_swiftc(platform, sdk_variant, xcode_developer_dir):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _xcrun_env(xcode_developer_dir)
    swiftc_path = host_command([xcrun, "--sdk", sdk, "--find", "swiftc"], env = env).strip()
    version = host_command([xcrun, "--sdk", sdk, "swiftc", "--version"], env = env).strip()
    # Identity also folds in the developer dir override so different
    # Xcode installations partition the action cache cleanly.
    identity = "once.apple.swiftc.v1\x00" + swiftc_path + "\x00" + version + "\x00" + (xcode_developer_dir or "")
    return (xcrun, sdk, identity, env)

def _filter_swift_sources(paths):
    return _filter_by_extensions(paths, [".swift"])

def _filter_objc_sources(paths):
    return _filter_by_extensions(paths, [".m", ".mm"])

def _filter_c_sources(paths):
    return _filter_by_extensions(paths, [".c"])

def _filter_cxx_sources(paths):
    return _filter_by_extensions(paths, [".cc", ".cpp", ".cxx"])

def _xcrun_clang(platform, sdk_variant, xcode_developer_dir):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _xcrun_env(xcode_developer_dir)
    clang_path = host_command([xcrun, "--sdk", sdk, "--find", "clang"], env = env).strip()
    sdk_path = host_command([xcrun, "--sdk", sdk, "--show-sdk-path"], env = env).strip()
    version = host_command([xcrun, "--sdk", sdk, "clang", "--version"], env = env).strip()
    identity = "once.apple.clang.v1\x00" + clang_path + "\x00" + version + "\x00" + (xcode_developer_dir or "")
    return (xcrun, sdk, sdk_path, identity, env)

def _xcrun_libtool(platform, sdk_variant, xcode_developer_dir):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _xcrun_env(xcode_developer_dir)
    libtool_path = host_command([xcrun, "--sdk", sdk, "--find", "libtool"], env = env).strip()
    identity = "once.apple.libtool.v1\x00" + libtool_path + "\x00" + (xcode_developer_dir or "")
    return (xcrun, sdk, identity, env)

def _xcrun_lipo(platform, sdk_variant, xcode_developer_dir):
    xcrun = host_which("xcrun")
    sdk = _apple_sdk_name(platform, sdk_variant)
    env = _xcrun_env(xcode_developer_dir)
    lipo_path = host_command([xcrun, "--sdk", sdk, "--find", "lipo"], env = env).strip()
    identity = "once.apple.lipo.v1\x00" + lipo_path + "\x00" + (xcode_developer_dir or "")
    return (xcrun, sdk, identity, env)

def _xcrun_codesign(xcode_developer_dir):
    xcrun = host_which("xcrun")
    env = _xcrun_env(xcode_developer_dir)
    codesign_path = host_command([xcrun, "--find", "codesign"], env = env).strip()
    identity = "once.apple.codesign.v1\x00" + codesign_path + "\x00" + (xcode_developer_dir or "")
    return (xcrun, codesign_path, identity, env)

def _swift_testing_macros_plugin(xcrun, xcrun_env):
    swiftc_path = host_command([xcrun, "--find", "swiftc"], env = xcrun_env).strip()
    suffix = "/usr/bin/swiftc"
    if not _ends_with(swiftc_path, suffix):
        fail("unable to derive Swift toolchain path from swiftc at " + swiftc_path)
    toolchain_dir = swiftc_path[:len(swiftc_path) - len(suffix)]
    return toolchain_dir + "/usr/lib/swift/host/plugins/testing/libTestingMacros.dylib"

def _xcrun_actool(xcode_developer_dir):
    xcrun = host_which("xcrun")
    env = _xcrun_env(xcode_developer_dir)
    actool_path = host_command([xcrun, "--find", "actool"], env = env).strip()
    identity = "once.apple.actool.v1\x00" + actool_path + "\x00" + (xcode_developer_dir or "")
    return (xcrun, actool_path, identity, env)

def _unique_dirs(paths):
    seen = {}
    out = []
    for path in paths:
        directory = _parent_dir(path)
        if directory and directory not in seen:
            seen[directory] = True
            out.append(directory)
    return out

def _basename(path):
    idx = -1
    for i in range(len(path)):
        if path[i] == "/":
            idx = i
    if idx < 0:
        return path
    return path[idx + 1:]

# --- Apple header map (.hmap) byte construction ---------------------
#
# Clang reads `.hmap` files via `-I <foo.hmap>`. The format is defined
# in LLVM's `clang/include/clang/Lex/HeaderMapTypes.h`. Layout (all
# little-endian on the platforms Once targets):
#
#   offset  size   field
#   ------  ----   -----
#     0      4     magic         = 0x68616D70
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
    out.extend(_u32_le(0x68616D70))
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

# Accumulate a transitive list of strings from a field that every dep
# provider exposes. Preserves order while removing duplicates: the
# first occurrence wins. Mirrors the Swift and Buck2 convention of
# propagating SwiftInfo / CcInfo fields up the graph.
def _collect_transitive(deps, key, own_values):
    seen = {}
    out = []
    for value in own_values:
        if value not in seen:
            seen[value] = True
            out.append(value)
    for dep in deps:
        for value in dep.get(key) or []:
            if value not in seen:
                seen[value] = True
                out.append(value)
    return out

def _validate_apple_native_deps(deps, consumer_label):
    for dep in deps:
        if dep.get("target_kind") != "rust_library":
            continue
        crate_type = dep.get("crate_type") or ""
        if crate_type == "staticlib":
            continue
        label = dep.get("label_id") or "dependency"
        fail(consumer_label + ": Rust library dep `" + label + "` has crate_type `" + crate_type + "` and does not provide an Apple static library; set crate_type = \"staticlib\" for Apple consumers")

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

def _apple_config_tokens(attrs, label_id):
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
    return tokens

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

def _resolve_attrs(attrs, label_id, non_configurable):
    tokens = _apple_config_tokens(attrs, label_id)
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
mkdir -p {test_dir}
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
    run_action(
        argv = ["/bin/sh", "-c", script],
        inputs = inputs,
        outputs = [test_dir, results, log, native_results],
        env = action_env,
        toolchain_identity = "once.shellspec_test.v1\x00" + shellspec,
        identifier = "shellspec_test:" + ctx["label"]["id"],
    )
    return provider

def _swift_testing_cases_script(swift_srcs, cases_file, target, runner_type):
    specs = _shell_words(swift_srcs)
    return """total=0
cases_file={cases_file}
: > "$cases_file"
for spec in {specs}; do
  [ -f "$spec" ] || continue
  suite=${{spec%.swift}}
  suite=${{suite##*/}}
  while IFS= read -r line; do
    case "$line" in
      *"@Test func "*)
        name=${{line#*"@Test func "}}
        name=${{name%%"("*}}
        total=$((total + 1))
        case_id="{target}::$name"
        if [ "$status" -eq 0 ]; then case_status=passed; else case_status=unknown; fi
        if [ "$total" -gt 1 ]; then printf ',\n' >> "$cases_file"; fi
        printf '{{"id":"%s","name":"%s","suite":"%s","file":"%s","status":"%s","attempts":[{{"status":"%s"}}],"runner_metadata":{{"runner":"%s"}}}}' "$case_id" "$name" "$suite" "$spec" "$case_status" "$case_status" "{runner_type}" >> "$cases_file"
        ;;
    esac
  done < "$spec"
done
""".format(
        cases_file = _shell_literal(cases_file),
        specs = specs,
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
        "listing": {
            "supported": runner_type == "swift_testing",
            "strategy": "parse_swift_testing_functions" if runner_type == "swift_testing" else "external_runner",
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
            "run_from_workspace_root": True,
        },
        "labels": labels,
        "metadata": {},
    }

def _apple_library_impl(ctx):
    attrs = _resolve_attrs(ctx["attr"], ctx["label"]["id"], ["module_name"])
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    module_name = attrs.get("module_name") or ctx["label"]["name"]
    sdk_frameworks = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    swift_flags = attrs.get("swift_flags") or []
    clang_flags = attrs.get("clang_flags") or []
    defines = attrs.get("defines") or []
    enable_testing = attrs.get("enable_testing") or False
    library_evolution = attrs.get("library_evolution") or False
    emit_dsym = attrs.get("emit_dsym") or False
    alwayslink = attrs.get("alwayslink") or False
    exported_deps = attrs.get("exported_deps") or []
    bridging_header = attrs.get("bridging_header") or ""
    exported_headers = attrs.get("exported_headers") or []
    enable_modules = attrs.get("enable_modules") or False

    all_srcs = glob(ctx["srcs"])
    swift_srcs = _filter_swift_sources(all_srcs)
    objc_srcs = _filter_objc_sources(all_srcs)
    c_srcs = _filter_c_sources(all_srcs)
    cxx_srcs = _filter_cxx_sources(all_srcs)
    if len(swift_srcs) == 0 and len(objc_srcs) == 0 and len(c_srcs) == 0 and len(cxx_srcs) == 0:
        fail("apple_library " + ctx["label"]["id"] + " has no compilable sources (.swift/.m/.mm/.c/.cc)")

    archs_attr = attrs.get("archs") or []
    archs = archs_attr if len(archs_attr) > 0 else [host_arch()]
    _reject_multi_arch_selects(ctx["attr"], ctx["label"]["id"], archs)
    mac_catalyst = attrs.get("mac_catalyst") or False
    if mac_catalyst and platform != "macos" and platform != "macosx":
        fail("apple_library " + ctx["label"]["id"] + " sets mac_catalyst = true but platform = `" + platform + "`; mac_catalyst requires platform = macos")
    is_universal = len(archs) > 1

    xcrun, sdk, swiftc_identity, xcrun_env = _xcrun_swiftc(platform, sdk_variant, xcode_developer_dir)
    archive = declare_output(module_name + ".a")

    deps = ctx["deps"]
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
    # Swift macro deps surface a `plugin_dylib` field. Pass each as a
    # `-load-plugin-library` to swiftc so the host loads the macro
    # implementation at compile time.
    plugin_dylibs = []
    for dep in deps:
        dylib = dep.get("plugin_dylib")
        if dylib and dylib not in plugin_dylibs:
            plugin_dylibs.append(dylib)

    # Own exported headers as workspace-relative paths, plus the
    # dirs we expose to consumers.
    own_exported_headers = [_package_relative(ctx, h) for h in exported_headers]
    own_exported_header_dirs = _unique_dirs(own_exported_headers)

    # Modulemap generation: if the target exports headers AND opts into
    # clang modules, write a modulemap so consumers can `import` the
    # module without listing each header on the command line. This is
    # the minimum Buck2 and Bazel Apple implementations do; framework modules and umbrella
    # headers can layer on later.
    modulemap_path = ""
    if enable_modules and len(own_exported_headers) > 0:
        modulemap_path = declare_output("module.modulemap")
        modulemap_lines = ["module " + module_name + " {"]
        for header in own_exported_headers:
            # Header paths in modulemaps are relative to the modulemap's
            # location. We write the modulemap into `build_dir/` and
            # reference each header by its workspace-relative path
            # prefixed with the relative escape back to workspace root.
            depth = len(ctx["build_dir"].split("/"))
            relative = ("../" * depth) + header
            modulemap_lines.append("    header \"" + relative + "\"")
        modulemap_lines.append("    export *")
        modulemap_lines.append("}")
        modulemap_lines.append("")
        write_path(modulemap_path, "\n".join(modulemap_lines))

    # Header map generation: cover the `#include "Foo.h"` and
    # `#include <Module/Foo.h>` lookup styles that a pure modulemap
    # doesn't help with. Each entry maps to the header's workspace-
    # relative path so consumers resolve them without listing
    # individual `-I` flags per source tree. Gated behind the same
    # `enable_modules` switch as modulemap generation so the two
    # surfaces toggle together.
    hmap_path = ""
    if enable_modules and len(own_exported_headers) > 0:
        hmap_path = declare_output(module_name + ".hmap")
        hmap_entries = {}
        for header in own_exported_headers:
            base = _basename(header)
            hmap_entries[base] = header
            hmap_entries[module_name + "/" + base] = header
        _write_hmap(hmap_path, hmap_entries)

    exported_deps_records = [deps[i] for i in exported_dep_indices]
    transitive_swiftmodule_dirs = _collect_transitive(
        exported_deps_records,
        "transitive_swiftmodule_dirs",
        [ctx["build_dir"]],
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
    transitive_modulemaps = _collect_transitive(
        exported_deps_records,
        "transitive_modulemaps",
        [modulemap_path] if modulemap_path else [],
    )
    transitive_hmaps = _collect_transitive(
        exported_deps_records,
        "transitive_hmaps",
        [hmap_path] if hmap_path else [],
    )
    transitive_archives = _collect_transitive(deps, "transitive_archives", [archive])
    transitive_sdk_frameworks = _collect_transitive(deps, "transitive_sdk_frameworks", sdk_frameworks)
    transitive_weak_sdk_frameworks = _collect_transitive(deps, "transitive_weak_sdk_frameworks", weak_sdk_frameworks)
    transitive_sdk_dylibs = _collect_transitive(deps, "transitive_sdk_dylibs", sdk_dylibs)
    transitive_linkopts = _collect_transitive(deps, "transitive_linkopts", linkopts)
    transitive_defines = _collect_transitive(deps, "transitive_defines", defines)
    transitive_alwayslink_archives = _collect_transitive(deps, "transitive_alwayslink_archives", [archive] if alwayslink else [])

    # --- Per-arch compile pipeline -----------------------------------
    # When a target requests a single architecture (the default,
    # `host_arch()`), the per-arch archive is the final archive
    # directly and no lipo step runs. With more than one arch each
    # compile emits a per-arch archive and a final `lipo -create`
    # action combines them.
    swift_only = len(objc_srcs) == 0 and len(c_srcs) == 0 and len(cxx_srcs) == 0
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
        swift_objc_header = declare_output(module_name + arch_suffix + "-Swift.h") if len(swift_srcs) > 0 else ""
        swift_objc_header_holder[0] = swift_objc_header

        # Swift output: per_arch_archive for swift-only, else
        # an intermediate that libtool merges with the clang objects.
        swift_archive = per_arch_archive if swift_only else (declare_output(module_name + "-swift" + arch_suffix + ".a") if len(swift_srcs) > 0 else "")

        if len(swift_srcs) > 0:
            swift_base_argv = [
                xcrun,
                "--sdk",
                sdk,
                "swiftc",
                "-module-name",
                module_name,
                "-target",
                triple,
                "-parse-as-library",
            ]
            if emit_dsym:
                swift_base_argv.append("-g")
            if enable_testing:
                swift_base_argv.append("-enable-testing")
            if library_evolution:
                swift_base_argv.append("-enable-library-evolution")
            if bridging_header:
                swift_base_argv.extend(["-import-objc-header", _package_relative(ctx, bridging_header)])
            for framework in sdk_frameworks:
                swift_base_argv.extend(["-framework", framework])
            for framework in weak_sdk_frameworks:
                swift_base_argv.extend(["-weak_framework", framework])
            for dep_dir in compile_swiftmodule_dirs:
                swift_base_argv.extend(["-I", dep_dir])
            # Header search paths flow through `-Xcc -I` so swiftc's
            # underlying Clang invocation (for bridging headers + ObjC
            # interop) can locate dep headers.
            for hdir in compile_header_dirs:
                swift_base_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
            # Feed each dep's modulemap to swiftc's underlying Clang so
            # `import` of a clang-module dep resolves without manual
            # `-fmodule-map-file` from the user.
            for mmap in dep_modulemaps:
                swift_base_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
            # Header maps flow through Clang's `-I` search, so the bridging
            # header (and any dep ObjC interop) can resolve `#include "Foo.h"`
            # without enumerating include directories.
            if hmap_path:
                swift_base_argv.extend(["-Xcc", "-I", "-Xcc", hmap_path])
            for hmap in dep_hmaps:
                swift_base_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
            if enable_modules:
                swift_base_argv.extend(["-Xcc", "-fmodules"])
            for dylib in plugin_dylibs:
                swift_base_argv.extend(["-load-plugin-library", dylib])
            for define in defines:
                swift_base_argv.extend(["-D", define])
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
            # Plugin dylibs participate in the action's input digest so
            # rebuilding the macro invalidates the consumer compile.
            for dylib in plugin_dylibs:
                if dylib not in swift_inputs:
                    swift_inputs.append(dylib)

            swift_module_argv = list(swift_base_argv)
            swift_module_argv.extend([
                "-static",
                "-emit-module",
                "-emit-module-path",
                swiftmodule,
                "-emit-objc-header",
                "-emit-objc-header-path",
                swift_objc_header,
            ])
            for src in swift_srcs:
                swift_module_argv.append(src)

            run_action(
                argv = swift_module_argv,
                inputs = swift_inputs,
                outputs = [swiftmodule, swiftdoc, swift_objc_header],
                env = xcrun_env,
                toolchain_identity = swiftc_identity,
                identifier = "swift_module_compile_" + module_name + arch_suffix,
            )

            swift_archive_argv = list(swift_base_argv)
            swift_archive_argv.extend(["-emit-library", "-static", "-o", swift_archive])
            for src in swift_srcs:
                swift_archive_argv.append(src)

            run_action(
                argv = swift_archive_argv,
                inputs = swift_inputs,
                outputs = [swift_archive],
                env = xcrun_env,
                toolchain_identity = swiftc_identity,
                identifier = "swift_archive_compile_" + module_name + arch_suffix,
            )

        arch_clang_objects = []
        if len(objc_srcs) > 0 or len(c_srcs) > 0 or len(cxx_srcs) > 0:
            clang_xcrun, clang_sdk, sdk_path, clang_identity, clang_env = _xcrun_clang(platform, sdk_variant, xcode_developer_dir)

            def compile_with_clang(src, language):
                # Sanitise the source path into a stable .o filename
                # under the build dir: `apps/ios/AppCore/Sources/A.m`
                # → `apps_ios_AppCore_Sources_A.m.o` (with `-<arch>`
                # appended for universal builds).
                sanitised = src.replace("/", "_")
                obj = declare_output(sanitised + arch_suffix + ".o")
                argv = [
                    clang_xcrun,
                    "--sdk",
                    clang_sdk,
                    "clang" if language != "c++" else "clang++",
                    "-c",
                    "-x",
                    language,
                    "-arch",
                    arch,
                    "-isysroot",
                    sdk_path,
                    "-target",
                    triple,
                    "-o",
                    obj,
                ]
                if language == "objective-c" or language == "objective-c++":
                    argv.append("-fobjc-arc")
                if emit_dsym:
                    argv.append("-g")
                if enable_modules:
                    argv.append("-fmodules")
                for framework in sdk_frameworks:
                    argv.extend(["-framework", framework])
                for hdir in own_exported_header_dirs:
                    argv.extend(["-I", hdir])
                for hdir in compile_header_dirs:
                    argv.extend(["-I", hdir])
                for mmap in dep_modulemaps:
                    argv.append("-fmodule-map-file=" + mmap)
                if hmap_path:
                    argv.extend(["-I", hmap_path])
                for hmap in dep_hmaps:
                    argv.extend(["-I", hmap])
                for define in defines:
                    argv.append("-D" + define)
                for flag in clang_flags:
                    argv.append(flag)
                argv.append(src)
                inputs = [src]
                for h in own_exported_headers:
                    if h not in inputs:
                        inputs.append(h)
                run_action(
                    argv = argv,
                    inputs = inputs,
                    outputs = [obj],
                    env = clang_env,
                    toolchain_identity = clang_identity,
                    identifier = "clang_compile_" + module_name + arch_suffix + "_" + sanitised,
                )
                arch_clang_objects.append(obj)

            for src in objc_srcs:
                compile_with_clang(src, "objective-c")
            for src in c_srcs:
                compile_with_clang(src, "c")
            for src in cxx_srcs:
                compile_with_clang(src, "c++")

        # Libtool merge into per_arch_archive. Only needed when there
        # is at least one non-Swift input alongside Swift; Swift-only
        # and C-only libraries already wrote into per_arch_archive.
        if not swift_only and len(swift_srcs) > 0:
            libtool_xcrun, libtool_sdk, libtool_identity, libtool_env = _xcrun_libtool(platform, sdk_variant, xcode_developer_dir)
            libtool_argv = [
                libtool_xcrun,
                "--sdk",
                libtool_sdk,
                "libtool",
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
                env = libtool_env,
                toolchain_identity = libtool_identity,
                identifier = "libtool_merge_" + module_name + arch_suffix,
            )
        elif len(swift_srcs) == 0 and len(arch_clang_objects) > 0:
            libtool_xcrun, libtool_sdk, libtool_identity, libtool_env = _xcrun_libtool(platform, sdk_variant, xcode_developer_dir)
            libtool_argv = [libtool_xcrun, "--sdk", libtool_sdk, "libtool", "-static", "-o", per_arch_archive]
            libtool_argv.extend(arch_clang_objects)
            run_action(
                argv = libtool_argv,
                inputs = list(arch_clang_objects),
                outputs = [per_arch_archive],
                env = libtool_env,
                toolchain_identity = libtool_identity,
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
        lipo_xcrun, lipo_sdk, lipo_identity, lipo_env = _xcrun_lipo(platform, sdk_variant, xcode_developer_dir)
        lipo_argv = [lipo_xcrun, "--sdk", lipo_sdk, "lipo", "-create", "-output", archive]
        lipo_argv.extend(per_arch_archives)
        run_action(
            argv = lipo_argv,
            inputs = list(per_arch_archives),
            outputs = [archive],
            env = lipo_env,
            toolchain_identity = lipo_identity,
            identifier = "lipo_" + module_name,
        )

    swift_objc_header = swift_objc_header_holder[0]

    return {
        "label_id": ctx["label"]["id"],
        "swiftmodule_dir": ctx["build_dir"] if len(swift_srcs) > 0 else "",
        "archive": archive,
        "objc_header": swift_objc_header,
        "alwayslink": alwayslink,
        "exported_headers": own_exported_headers,
        "exported_header_dirs": own_exported_header_dirs,
        "modulemap": modulemap_path,
        "hmap": hmap_path,
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_exported_headers": transitive_exported_headers,
        "transitive_exported_header_dirs": transitive_exported_header_dirs,
        "transitive_modulemaps": transitive_modulemaps,
        "transitive_hmaps": transitive_hmaps,
        "transitive_archives": transitive_archives,
        "transitive_alwayslink_archives": transitive_alwayslink_archives,
        "transitive_sdk_frameworks": transitive_sdk_frameworks,
        "transitive_weak_sdk_frameworks": transitive_weak_sdk_frameworks,
        "transitive_sdk_dylibs": transitive_sdk_dylibs,
        "transitive_linkopts": transitive_linkopts,
        "transitive_defines": transitive_defines,
    }

def _swift_macro_impl(ctx):
    attrs = _resolve_attrs(ctx["attr"], ctx["label"]["id"], ["module_name"])
    minimum_os = attrs.get("minimum_os") or "13.0"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    module_name = attrs.get("module_name") or ctx["label"]["name"]
    swift_flags = attrs.get("swift_flags") or []

    all_srcs = glob(ctx["srcs"])
    swift_srcs = _filter_swift_sources(all_srcs)
    if len(swift_srcs) == 0:
        fail("swift_macro " + ctx["label"]["id"] + " has no Swift sources (.swift)")

    # Swift macros are host-loaded compiler plugins. They always build
    # for macOS in the simulator-equivalent SDK; macOS ignores the
    # variant anyway.
    xcrun, sdk, swiftc_identity, xcrun_env = _xcrun_swiftc("macos", "simulator", xcode_developer_dir)
    triple = _apple_triple("macos", minimum_os, "simulator", host_arch(), False)

    plugin_dylib = declare_output("lib" + module_name + ".dylib")
    plugin_swiftmodule = declare_output(module_name + ".swiftmodule")

    deps = ctx["deps"]
    _validate_apple_native_deps(deps, ctx["label"]["id"])

    # Aggregate dep archives, swiftmodule search paths, frameworks, and
    # linkopts so the plugin links against a real swift-syntax
    # checkout the user provides via `deps`.
    dep_swiftmodule_dirs = []
    for dep in deps:
        for d in dep.get("transitive_swiftmodule_dirs") or []:
            if d and d != ctx["build_dir"] and d not in dep_swiftmodule_dirs:
                dep_swiftmodule_dirs.append(d)
    dep_archives = []
    for dep in deps:
        for ar in dep.get("transitive_archives") or []:
            if ar and ar not in dep_archives:
                dep_archives.append(ar)
    dep_sdk_frameworks = []
    for dep in deps:
        for fw in dep.get("transitive_sdk_frameworks") or []:
            if fw and fw not in dep_sdk_frameworks:
                dep_sdk_frameworks.append(fw)
    dep_linkopts = []
    for dep in deps:
        for opt in dep.get("transitive_linkopts") or []:
            if opt and opt not in dep_linkopts:
                dep_linkopts.append(opt)

    swift_argv = [
        xcrun,
        "--sdk",
        sdk,
        "swiftc",
        "-emit-library",
        "-emit-module",
        "-module-name",
        module_name,
        "-emit-module-path",
        plugin_swiftmodule,
        "-target",
        triple,
        "-parse-as-library",
        "-o",
        plugin_dylib,
    ]
    for d in dep_swiftmodule_dirs:
        swift_argv.extend(["-I", d])
    for fw in dep_sdk_frameworks:
        swift_argv.extend(["-framework", fw])
    for flag in swift_flags:
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    # Dep archives appear as positional inputs; swiftc forwards
    # unknown-extension inputs to the linker.
    for ar in dep_archives:
        swift_argv.append(ar)
    for opt in dep_linkopts:
        swift_argv.extend(["-Xlinker", opt])

    swift_inputs = list(swift_srcs)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [plugin_dylib, plugin_swiftmodule],
        env = xcrun_env,
        toolchain_identity = swiftc_identity,
        identifier = "swift_macro_compile_" + module_name,
    )

    return {
        "label_id": ctx["label"]["id"],
        "plugin_dylib": plugin_dylib,
        "plugin_module_name": module_name,
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

def _collect_dep_compile_inputs(deps, build_dir):
    """Aggregate compile-visible inputs from dep providers.

    Returns (swiftmodule_dirs, header_dirs, modulemaps, hmaps, archives,
    framework_search_dirs, framework_module_names, framework_files, sdk_frameworks,
    sdk_dylibs, linkopts, plugin_dylibs).
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
    plugin_dylibs = []
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
        framework_path = dep.get("framework_path")
        if framework_path:
            for f in dep.get("framework_files") or []:
                if f and f not in framework_files:
                    framework_files.append(f)
            framework_parent = _parent_dir(framework_path)
            if framework_parent and framework_parent not in framework_search_dirs:
                framework_search_dirs.append(framework_parent)
            module_name = dep.get("framework_module_name")
            if module_name and module_name not in framework_module_names:
                framework_module_names.append(module_name)
        for fw in dep.get("transitive_sdk_frameworks") or []:
            if fw and fw not in sdk_frameworks:
                sdk_frameworks.append(fw)
        for dy in dep.get("transitive_sdk_dylibs") or []:
            if dy and dy not in sdk_dylibs:
                sdk_dylibs.append(dy)
        for opt in dep.get("transitive_linkopts") or []:
            if opt and opt not in linkopts:
                linkopts.append(opt)
        plugin_dylib = dep.get("plugin_dylib")
        if plugin_dylib and plugin_dylib not in plugin_dylibs:
            plugin_dylibs.append(plugin_dylib)
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
        plugin_dylibs,
    )

def _apple_framework_impl(ctx):
    attrs = _resolve_attrs(ctx["attr"], ctx["label"]["id"], ["product_name"])
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
    xcrun, sdk, swiftc_identity, xcrun_env = _xcrun_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant, arch, False)

    framework_dir = product_name + ".framework"
    dylib = declare_output(framework_dir + "/" + product_name)
    swiftmodule = declare_output(framework_dir + "/Modules/" + module_name + ".swiftmodule")
    swiftdoc = declare_output(framework_dir + "/Modules/" + module_name + ".swiftdoc")
    modulemap = declare_output(framework_dir + "/Modules/module.modulemap")
    info_plist = declare_output(framework_dir + "/Info.plist")

    deps = ctx["deps"]
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
        plugin_dylibs,
    ) = _collect_dep_compile_inputs(deps, ctx["build_dir"])

    swift_argv = [
        xcrun,
        "--sdk",
        sdk,
        "swiftc",
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
    for d in framework_search_dirs:
        swift_argv.extend(["-F", d])
    for fw in framework_module_names:
        swift_argv.extend(["-framework", fw])
    for fw in sdk_frameworks_attr:
        swift_argv.extend(["-framework", fw])
    for fw in dep_sdk_frameworks:
        if fw not in sdk_frameworks_attr:
            swift_argv.extend(["-framework", fw])
    for fw in weak_sdk_frameworks:
        swift_argv.extend(["-weak_framework", fw])
    for dy in sdk_dylibs_attr:
        swift_argv.extend(["-l" + dy])
    for dy in dep_sdk_dylibs:
        if dy not in sdk_dylibs_attr:
            swift_argv.extend(["-l" + dy])
    for opt in linkopts:
        swift_argv.append(opt)
    for opt in dep_linkopts:
        if opt not in linkopts:
            swift_argv.append(opt)
    for dylib_path in plugin_dylibs:
        swift_argv.extend(["-load-plugin-library", dylib_path])
    for flag in swift_flags:
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    for ar in dep_archives:
        swift_argv.append(ar)

    swift_inputs = list(swift_srcs)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)
    for f in dep_framework_files:
        if f not in swift_inputs:
            swift_inputs.append(f)
    for d in plugin_dylibs:
        if d not in swift_inputs:
            swift_inputs.append(d)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [dylib, swiftmodule, swiftdoc],
        env = xcrun_env,
        toolchain_identity = swiftc_identity,
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
    cs_xcrun, codesign_path, cs_identity, cs_env = _xcrun_codesign(xcode_developer_dir)
    cs_stamp = declare_output(framework_dir + "/_CodeSignature/CodeResources")
    run_action(
        argv = [codesign_path, "--force", "--sign", "-", "--timestamp=none", ctx["build_dir"] + "/" + framework_dir],
        inputs = [dylib, info_plist, modulemap, swiftmodule],
        outputs = [cs_stamp],
        env = cs_env,
        toolchain_identity = cs_identity,
        identifier = "apple_framework_codesign_" + module_name,
    )

    own_swiftmodule_dir = ctx["build_dir"] + "/" + framework_dir + "/Modules"
    transitive_archives = _collect_transitive(deps, "transitive_archives", [])
    transitive_swiftmodule_dirs = _collect_transitive(deps, "transitive_swiftmodule_dirs", [own_swiftmodule_dir])
    transitive_sdk_frameworks = _collect_transitive(deps, "transitive_sdk_frameworks", sdk_frameworks_attr)
    transitive_weak_sdk_frameworks = _collect_transitive(deps, "transitive_weak_sdk_frameworks", weak_sdk_frameworks)
    transitive_sdk_dylibs = _collect_transitive(deps, "transitive_sdk_dylibs", sdk_dylibs_attr)
    transitive_linkopts = _collect_transitive(deps, "transitive_linkopts", linkopts)
    transitive_frameworks = _collect_transitive(deps, "transitive_frameworks", [ctx["build_dir"] + "/" + framework_dir])

    framework_files = [dylib, swiftmodule, swiftdoc, modulemap, info_plist, cs_stamp]

    return {
        "label_id": ctx["label"]["id"],
        "framework_path": ctx["build_dir"] + "/" + framework_dir,
        "framework_module_name": module_name,
        "framework_files": framework_files,
        "swiftmodule_dir": own_swiftmodule_dir,
        "transitive_swiftmodule_dirs": transitive_swiftmodule_dirs,
        "transitive_archives": transitive_archives,
        "transitive_frameworks": transitive_frameworks,
        "transitive_sdk_frameworks": transitive_sdk_frameworks,
        "transitive_weak_sdk_frameworks": transitive_weak_sdk_frameworks,
        "transitive_sdk_dylibs": transitive_sdk_dylibs,
        "transitive_linkopts": transitive_linkopts,
    }

def shell_quote_for_action(path):
    # Single-quote the path and escape any embedded single quotes by
    # closing, escaping with double-quoted apostrophe, and reopening.
    escaped = path.replace("'", "'\"'\"'")
    return "'" + escaped + "'"

def _apple_application_run_script(label_id, platform, sdk_variant, xcrun, app_path, bundle_id, run_dir, run_record, run_log):
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
        command = _ios_simulator_selection_script(xcrun) + """
{xcrun} simctl boot "$simulator_id" >> {log} 2>&1 || true
{xcrun} simctl bootstatus "$simulator_id" -b >> {log} 2>&1
{xcrun} simctl install "$simulator_id" {app} >> {log} 2>&1
{xcrun} simctl launch "$simulator_id" {bundle_id} >> {log} 2>&1
printf '%s%s%s\\n' {record_prefix} "$simulator_id" {record_suffix} > {record}
""".format(
            xcrun = _shell_literal(xcrun),
            log = _shell_literal(run_log),
            app = _shell_literal(app_path),
            bundle_id = _shell_literal(bundle_id),
            record_prefix = _shell_literal(record_prefix),
            record_suffix = _shell_literal(record_suffix),
            record = _shell_literal(run_record),
        )
    else:
        fail(label_id + ": apple_application run supports macos and ios simulator targets")
    return """set -eu
mkdir -p {run_dir}
: > {log}
{command}
""".format(
        run_dir = _shell_literal(run_dir),
        log = _shell_literal(run_log),
        command = command,
    )

def _apple_application_impl(ctx):
    attrs = _resolve_attrs(ctx["attr"], ctx["label"]["id"], ["product_name"])
    _reject_unsupported_attrs(attrs, ctx["label"]["id"], ["resources", "asset_catalogs", "info_plist", "info_plist_substitutions", "entitlements", "provisioning_profile", "signing_identity"])
    if attrs.get("signing") and attrs.get("signing") != "ad_hoc":
        fail(ctx["label"]["id"] + ": attribute `signing` only supports `ad_hoc` today")
    platform = attrs["platform"]
    bundle_id = attrs["bundle_id"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    product_name = attrs.get("product_name") or ctx["label"]["name"]
    families = attrs.get("families") or ["iphone"]
    sdk_frameworks_attr = attrs.get("sdk_frameworks") or []
    weak_sdk_frameworks = attrs.get("weak_sdk_frameworks") or []
    sdk_dylibs_attr = attrs.get("sdk_dylibs") or []
    linkopts = attrs.get("linkopts") or []
    swift_flags = attrs.get("swift_flags") or []

    all_srcs = glob(ctx["srcs"])
    swift_srcs = _filter_swift_sources(all_srcs)
    if len(swift_srcs) == 0:
        fail("apple_application " + ctx["label"]["id"] + " has no Swift sources (.swift)")

    arch = host_arch()
    xcrun, sdk, swiftc_identity, xcrun_env = _xcrun_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant, arch, False)

    app_dir = product_name + ".app"
    app_path = ctx["build_dir"] + "/" + app_dir
    if ctx["capability"] == "run":
        run_dir = ctx["build_dir"] + "/run"
        run_record = run_dir + "/run.json"
        run_log = run_dir + "/run.log"
        run_action(
            argv = ["/bin/sh", "-c", _apple_application_run_script(ctx["label"]["id"], platform, sdk_variant, xcrun, app_path, bundle_id, run_dir, run_record, run_log)],
            outputs = [run_dir, run_record, run_log],
            env = xcrun_env,
            cacheable = False,
            toolchain_identity = "once.apple.application.run.v1\x00" + swiftc_identity,
            identifier = "apple_application_run_" + product_name,
        )
        return {
            "label_id": ctx["label"]["id"],
            "app_path": app_path,
            "bundle_id": bundle_id,
        }

    executable = declare_output(app_dir + "/" + product_name)
    info_plist = declare_output(app_dir + "/Info.plist")

    deps = ctx["deps"]
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
        plugin_dylibs,
    ) = _collect_dep_compile_inputs(deps, ctx["build_dir"])

    swift_argv = [
        xcrun,
        "--sdk",
        sdk,
        "swiftc",
        "-module-name",
        product_name,
        "-target",
        triple,
        "-parse-as-library",
        "-Xlinker",
        "-rpath",
        "-Xlinker",
        "@executable_path/Frameworks",
        "-o",
        executable,
    ]
    for d in compile_swiftmodule_dirs:
        swift_argv.extend(["-I", d])
    for hdir in compile_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for mmap in dep_modulemaps:
        swift_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
    for hmap in dep_hmaps:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
    for d in framework_search_dirs:
        swift_argv.extend(["-F", d])
    for fw in framework_module_names:
        swift_argv.extend(["-framework", fw])
    for fw in sdk_frameworks_attr:
        swift_argv.extend(["-framework", fw])
    for fw in dep_sdk_frameworks:
        if fw not in sdk_frameworks_attr:
            swift_argv.extend(["-framework", fw])
    for fw in weak_sdk_frameworks:
        swift_argv.extend(["-weak_framework", fw])
    for dy in sdk_dylibs_attr:
        swift_argv.extend(["-l" + dy])
    for dy in dep_sdk_dylibs:
        if dy not in sdk_dylibs_attr:
            swift_argv.extend(["-l" + dy])
    for opt in linkopts:
        swift_argv.append(opt)
    for opt in dep_linkopts:
        if opt not in linkopts:
            swift_argv.append(opt)
    for dylib_path in plugin_dylibs:
        swift_argv.extend(["-load-plugin-library", dylib_path])
    for flag in swift_flags:
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    for ar in dep_archives:
        swift_argv.append(ar)

    swift_inputs = list(swift_srcs)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)
    for f in dep_framework_files:
        if f not in swift_inputs:
            swift_inputs.append(f)
    for d in plugin_dylibs:
        if d not in swift_inputs:
            swift_inputs.append(d)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [executable],
        env = xcrun_env,
        toolchain_identity = swiftc_identity,
        identifier = "apple_application_compile_" + product_name,
    )

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
        "DTPlatformName": sdk,
    }
    bool_entries = {"LSRequiresIPhoneOS": True}
    device_family_codes = []
    for family in families:
        if family == "iphone":
            device_family_codes.append({"integer": 1})
        elif family == "ipad":
            device_family_codes.append({"integer": 2})
    array_entries = {"UIDeviceFamily": device_family_codes}
    write_path(info_plist, _render_plist(plist_entries, bool_entries, array_entries))

    # Embed each transitive dep framework into App.app/Frameworks/.
    # Each framework is copied as a whole bundle directory and
    # individually ad-hoc codesigned so the app's dyld loads them
    # without rejecting the signature.
    cs_xcrun, codesign_path, cs_identity, cs_env = _xcrun_codesign(xcode_developer_dir)
    embedded_stamps = []
    dep_framework_paths = []
    dep_framework_files = {}
    for dep in deps:
        framework_path = dep.get("framework_path")
        if framework_path and framework_path not in dep_framework_paths:
            dep_framework_paths.append(framework_path)
            dep_framework_files[framework_path] = dep.get("framework_files") or []
    for framework_path in dep_framework_paths:
        framework_basename = _basename(framework_path)
        source_files = dep_framework_files.get(framework_path) or []
        # Declare every file that lands in the embedded framework so
        # the Once runner preserves them. Map each source file
        # (`<build_dir>/<fw>/<sub>`) to its embedded path
        # (`<app>/Frameworks/<fw>/<sub>`) by stripping the framework
        # path prefix.
        framework_prefix = framework_path + "/"
        embed_outputs = []
        for source in source_files:
            if source == framework_path:
                embed_outputs.append(declare_output(app_dir + "/Frameworks/" + framework_basename))
                continue
            if source.startswith(framework_prefix):
                rel = source[len(framework_prefix):]
                embed_outputs.append(declare_output(app_dir + "/Frameworks/" + framework_basename + "/" + rel))
        embedded_stamp = declare_output(app_dir + "/Frameworks/" + framework_basename + "/_CodeSignature/CodeResources")
        if embedded_stamp not in embed_outputs:
            embed_outputs.append(embedded_stamp)
        embed_inputs = list(source_files)
        embedded_framework_path = ctx["build_dir"] + "/" + app_dir + "/Frameworks/" + framework_basename
        prepare_path(embedded_framework_path, kind = "remove", identifier = "apple_application_embed_clean_" + framework_basename)
        copy_path(
            framework_path,
            embedded_framework_path,
            kind = "tree",
            inputs = embed_inputs,
            identifier = "apple_application_embed_copy_" + framework_basename,
        )
        run_action(
            argv = [codesign_path, "--force", "--sign", "-", "--timestamp=none", embedded_framework_path],
            inputs = embed_inputs,
            outputs = embed_outputs,
            env = cs_env,
            toolchain_identity = cs_identity,
            identifier = "apple_application_embed_" + framework_basename,
        )
        embedded_stamps.append(embedded_stamp)

    # Ad-hoc codesign the .app bundle itself. Must run after embedded
    # frameworks land so their signature is included in the bundle's
    # resource envelope.
    app_cs_stamp = declare_output(app_dir + "/_CodeSignature/CodeResources")
    cs_inputs = [executable, info_plist]
    for stamp in embedded_stamps:
        cs_inputs.append(stamp)
    run_action(
        argv = [codesign_path, "--force", "--sign", "-", "--timestamp=none", ctx["build_dir"] + "/" + app_dir],
        inputs = cs_inputs,
        outputs = [app_cs_stamp],
        env = cs_env,
        toolchain_identity = cs_identity,
        identifier = "apple_application_codesign_" + product_name,
    )

    return {
        "label_id": ctx["label"]["id"],
        "app_path": app_path,
        "bundle_id": bundle_id,
    }

def _apple_test_bundle_impl(ctx):
    attrs = _resolve_attrs(ctx["attr"], ctx["label"]["id"], ["product_name"])
    _reject_unsupported_attrs(attrs, ctx["label"]["id"], ["test_host", "resources", "asset_catalogs", "info_plist", "entitlements", "destination", "test_plan"])
    platform = attrs["platform"]
    minimum_os = attrs.get("minimum_os") or "13.0"
    target_sdk_version = attrs.get("target_sdk_version") or minimum_os
    sdk_variant = attrs.get("sdk_variant") or "simulator"
    xcode_developer_dir = attrs.get("xcode_developer_dir") or ""
    product_name = attrs.get("product_name") or ctx["label"]["name"]
    module_name = product_name
    swift_flags = attrs.get("swift_flags") or []
    swift_testing = attrs.get("swift_testing") or False
    test_env = attrs.get("test_env") or {}
    labels = attrs.get("labels") or []

    all_srcs = glob(ctx["srcs"])
    swift_srcs = _filter_swift_sources(all_srcs)
    if len(swift_srcs) == 0:
        fail("apple_test_bundle " + ctx["label"]["id"] + " has no Swift sources (.swift)")

    test_dir = ctx["build_dir"] + "/test"
    results = test_dir + "/test_results.json"
    log = test_dir + "/swift-testing.log" if swift_testing else test_dir + "/xctest.log"
    native_results = test_dir + "/native_results.txt"
    action_env = {"HOME": test_dir + "/home"}
    for key in test_env:
        action_env[key] = test_env[key]
    arch = host_arch()
    xcrun, sdk, swiftc_identity, xcrun_env = _xcrun_swiftc(platform, sdk_variant, xcode_developer_dir)
    triple = _apple_triple(platform, target_sdk_version, sdk_variant, arch, False)
    for key in xcrun_env:
        action_env[key] = xcrun_env[key]

    # XCTest lives in the platform's developer-frameworks tree, not
    # the SDK's default search path. Resolve `<platform>/Developer/
    # Library/Frameworks` via xcrun and add it as `-F`/`-rpath` so the
    # linker finds the framework and dyld locates it at runtime.
    platform_path = host_command([xcrun, "--sdk", sdk, "--show-sdk-platform-path"], env = xcrun_env).strip()
    xctest_framework_dir = platform_path + "/Developer/Library/Frameworks"
    xctest_usr_lib_dir = platform_path + "/Developer/usr/lib"
    testing_macros_plugin = _swift_testing_macros_plugin(xcrun, xcrun_env)

    bundle_dir = product_name + ".xctest"
    if platform == "macos" or platform == "macosx":
        test_binary = declare_output(bundle_dir + "/Contents/MacOS/" + product_name)
        info_plist = declare_output(bundle_dir + "/Contents/Info.plist")
    else:
        test_binary = declare_output(bundle_dir + "/" + product_name)
        info_plist = declare_output(bundle_dir + "/Info.plist")
    test_bundle_path = ctx["build_dir"] + "/" + bundle_dir
    runner_type = "swift_testing" if swift_testing else "xctest"
    command_argv = [xcrun, "xctest", test_bundle_path]
    provider = {
        "label_id": ctx["label"]["id"],
        "test_bundle_path": test_bundle_path,
        "affected_inputs": all_srcs,
        "test_info": _apple_test_info(ctx, runner_type, command_argv, action_env, labels, results, log, native_results),
    }

    deps = ctx["deps"]
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
        plugin_dylibs,
    ) = _collect_dep_compile_inputs(deps, ctx["build_dir"])

    # An XCTest bundle is a Mach-O loadable bundle; swiftc takes
    # `-emit-library` and the linker `-bundle` flag is plumbed through
    # `-Xlinker`. The XCTest framework lives under the platform's
    # `Developer/Library/Frameworks`; add it to both the framework
    # search path (`-F`) and the dyld rpath so the test runner can
    # load it at simulator launch time.
    swift_argv = [
        xcrun,
        "--sdk",
        sdk,
        "swiftc",
        "-emit-library",
        "-module-name",
        module_name,
        "-target",
        triple,
        "-parse-as-library",
        "-Xlinker",
        "-bundle",
        "-F",
        xctest_framework_dir,
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
        "-framework",
        "XCTest",
        "-o",
        test_binary,
    ]
    if swift_testing:
        swift_argv.extend([
            "-framework",
            "Testing",
            "-lXCTestSwiftSupport",
            "-load-plugin-library",
            testing_macros_plugin,
        ])
    for d in compile_swiftmodule_dirs:
        swift_argv.extend(["-I", d])
    for hdir in compile_header_dirs:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hdir])
    for mmap in dep_modulemaps:
        swift_argv.extend(["-Xcc", "-fmodule-map-file=" + mmap])
    for hmap in dep_hmaps:
        swift_argv.extend(["-Xcc", "-I", "-Xcc", hmap])
    for d in framework_search_dirs:
        swift_argv.extend(["-F", d])
    for fw in framework_module_names:
        swift_argv.extend(["-framework", fw])
    for fw in dep_sdk_frameworks:
        swift_argv.extend(["-framework", fw])
    for dy in dep_sdk_dylibs:
        swift_argv.extend(["-l" + dy])
    for opt in dep_linkopts:
        swift_argv.append(opt)
    for dylib_path in plugin_dylibs:
        swift_argv.extend(["-load-plugin-library", dylib_path])
    for flag in swift_flags:
        swift_argv.append(flag)
    for src in swift_srcs:
        swift_argv.append(src)
    for ar in dep_archives:
        swift_argv.append(ar)

    swift_inputs = list(swift_srcs)
    for ar in dep_archives:
        if ar not in swift_inputs:
            swift_inputs.append(ar)
    for f in dep_framework_files:
        if f not in swift_inputs:
            swift_inputs.append(f)
    for dylib_path in plugin_dylibs:
        if dylib_path not in swift_inputs:
            swift_inputs.append(dylib_path)

    run_action(
        argv = swift_argv,
        inputs = swift_inputs,
        outputs = [test_binary],
        env = xcrun_env,
        toolchain_identity = swiftc_identity,
        identifier = "apple_test_bundle_compile_" + module_name,
    )

    plist_entries = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": product_name,
        "CFBundleIdentifier": "dev.once.tests." + product_name,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": product_name,
        "CFBundlePackageType": "BNDL",
        "CFBundleShortVersionString": "1.0",
        "CFBundleVersion": "1",
        "MinimumOSVersion": minimum_os,
    }
    write_path(info_plist, _render_plist(plist_entries))

    cs_xcrun, codesign_path, cs_identity, cs_env = _xcrun_codesign(xcode_developer_dir)
    if platform == "macos" or platform == "macosx":
        test_cs_stamp = declare_output(bundle_dir + "/Contents/_CodeSignature/CodeResources")
    else:
        test_cs_stamp = declare_output(bundle_dir + "/_CodeSignature/CodeResources")
    run_action(
        argv = [codesign_path, "--force", "--sign", "-", "--timestamp=none", test_bundle_path],
        inputs = [test_binary, info_plist],
        outputs = [test_cs_stamp],
        env = cs_env,
        toolchain_identity = cs_identity,
        identifier = "apple_test_bundle_codesign_" + module_name,
    )

    if ctx["capability"] == "test":
        cases_file = test_dir + "/cases.jsonl"
        if platform == "macos" or platform == "macosx":
            runner_command = """DYLD_LIBRARY_PATH={usr_lib}${{DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}} DYLD_FALLBACK_FRAMEWORK_PATH={frameworks}${{DYLD_FALLBACK_FRAMEWORK_PATH:+:$DYLD_FALLBACK_FRAMEWORK_PATH}} {command}""".format(
                usr_lib = _shell_literal(xctest_usr_lib_dir),
                frameworks = _shell_literal(xctest_framework_dir),
                command = _shell_words([xcrun, "xctest", test_bundle_path]),
            )
        elif sdk_variant == "simulator":
            runner_command = _ios_simulator_selection_script(xcrun) + """
{xcrun} simctl boot "$simulator_id" >/dev/null 2>&1 || true
{xcrun} simctl bootstatus "$simulator_id" -b
tmpdir=$(mktemp -d "${{TMPDIR:-/tmp}}/once-xctest.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT
cp -R {bundle} "$tmpdir/"
find "$tmpdir/{bundle_name}" -type d -exec chmod 755 {{}} +
find "$tmpdir/{bundle_name}" -type f -exec chmod 644 {{}} +
chmod 755 "$tmpdir/{bundle_name}/{binary_name}"
SIMCTL_CHILD_DYLD_LIBRARY_PATH={usr_lib} SIMCTL_CHILD_DYLD_FALLBACK_FRAMEWORK_PATH={frameworks} {xcrun} simctl spawn "$simulator_id" {xctest_agent} -XCTest All "$tmpdir/{bundle_name}"
""".format(
                xcrun = _shell_literal(xcrun),
                bundle = _shell_literal(test_bundle_path),
                bundle_name = bundle_dir,
                binary_name = product_name,
                usr_lib = _shell_literal(xctest_usr_lib_dir),
                frameworks = _shell_literal(xctest_framework_dir),
                xctest_agent = _shell_literal(platform_path + "/Developer/Library/Xcode/Agents/xctest"),
            )
        else:
            fail(ctx["label"]["id"] + ": apple_test_bundle execution supports macos and simulator targets; device runners need xctestrun support")
        script = """set -eu
mkdir -p {test_dir}
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
            cases_script = _swift_testing_cases_script(swift_srcs, cases_file, ctx["label"]["id"], runner_type),
            target = ctx["label"]["id"],
            runner_type = runner_type,
        )
        test_inputs = [test_binary, info_plist, test_cs_stamp]
        for src in swift_srcs:
            if src not in test_inputs:
                test_inputs.append(src)
        run_action(
            argv = ["/bin/sh", "-c", script],
            inputs = test_inputs,
            outputs = [test_dir, results, log, native_results],
            env = action_env,
            cacheable = False,
            toolchain_identity = "once.apple." + runner_type + ".runner.v1\x00" + swiftc_identity,
            identifier = "apple_" + runner_type + ":" + ctx["label"]["id"],
        )

    return provider

swift_macro = target_kind(
    docs = "Compiles a Swift compiler-plugin dylib that consumers load via `-load-plugin-library` at compile time.",
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
        capability("build", ["default", "plugin_dylib", "swiftmodule"]),
    ],
    examples = [
        example(
            "swift-macro-minimal",
            name = "Minimal Swift macro plugin",
            use_when = "You want a host-loaded Swift compiler plugin target that a library can depend on.",
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
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name, such as UIKit or Foundation"),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
        attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags, propagated transitively to consumers"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
        attr("clang_flags", "list<string>", default = "[]", docs = "Extra Clang compiler flags"),
        attr("defines", "list<string>", default = "[]", docs = "`-D` preprocessor / Swift conditional compilation flags, propagated transitively"),
        attr("enable_testing", "bool", default = "false", docs = "Compile Swift with testability enabled for dependent tests"),
        attr("library_evolution", "bool", default = "false", docs = "Emit stable Swift module interfaces for binary compatibility"),
        attr("emit_dsym", "bool", default = "false", docs = "Emit DWARF debug info so downstream target kinds can extract a `.dSYM` bundle"),
        attr("sdk_variant", "string", default = "\"simulator\"", docs = "`simulator` or `device` SDK selection. Ignored on macOS (always uses macosx)", configurable = False),
        attr("archs", "list<string>", default = "[]", docs = "Target architectures (`arm64`, `x86_64`, `arm64e`, `arm64_32`). Empty defaults to the host arch; multi-arch fans out per-arch compiles and combines them with `lipo`", configurable = False),
        attr("mac_catalyst", "bool", default = "false", docs = "Build the iOSMac (Mac Catalyst) variant. Requires `platform = macos`; rewrites the triple to `<arch>-apple-ios<minOS>-macabi`", configurable = False),
        attr("xcode_developer_dir", "string", docs = "Pin a specific Xcode by overriding `DEVELOPER_DIR`. Folded into the action cache key"),
        attr("alwayslink", "bool", default = "false", docs = "Hint to downstream linker target kinds to force-load this archive (`-Wl,-force_load`)"),
        attr("exported_deps", "list<string>", default = "[]", docs = "Target IDs from `deps` whose module interface flows through to consumers' compile path"),
        attr("bridging_header", "string", docs = "ObjC bridging header that lets Swift sources see ObjC symbols (`-import-objc-header`)"),
        attr("enable_modules", "bool", default = "false", docs = "Emit a `module.modulemap` for `exported_headers` and pass `-fmodules` to Clang so consumers can `import` the module instead of #importing each header"),
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
        attr("resources", "list<string>", default = "[]", docs = "Resource glob patterns bundled into the framework"),
        attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the framework bundle"),
        attr("privacy_manifest", "string", docs = "Privacy manifest placed in the framework bundle"),
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name"),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
        attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
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
        attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the application bundle"),
        attr("info_plist", "string", docs = "Info.plist template path"),
        attr("info_plist_substitutions", "map<string,string>", default = "{}", docs = "Values substituted into the generated Info.plist"),
        attr("entitlements", "string", docs = "Entitlements plist path"),
        attr("provisioning_profile", "string", docs = "Provisioning profile label or path used for signing"),
        attr("signing_identity", "string", docs = "Local signing identity selector used for development device signing"),
        attr("signing", "string", default = "ad_hoc", docs = "Signing mode or policy name"),
        attr("sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked by name"),
        attr("weak_sdk_frameworks", "list<string>", default = "[]", docs = "Apple SDK frameworks linked weakly"),
        attr("sdk_dylibs", "list<string>", default = "[]", docs = "Apple SDK dynamic libraries linked by name"),
        attr("linkopts", "list<string>", default = "[]", docs = "Extra linker flags"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
    ],
    deps = [
        dep("deps", ["apple_linkable", "apple_framework", "apple_resource", "apple_swift_plugin", "native_linkable"], "Libraries, frameworks, resources, native linkables, and Swift compiler plugins embedded in the app"),
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
        attr("test_host", "target", docs = "Application target hosting the test bundle"),
        attr("resources", "list<string>", default = "[]", docs = "Resource glob patterns bundled into the test bundle"),
        attr("asset_catalogs", "list<string>", default = "[]", docs = "Asset catalog paths compiled into the test bundle"),
        attr("info_plist", "string", docs = "Info.plist template path"),
        attr("entitlements", "string", docs = "Entitlements plist path"),
        attr("destination", "string", docs = "Simulator, device, or local destination selector"),
        attr("test_plan", "string", docs = "XCTest plan path"),
        attr("test_env", "map<string,string>", default = "{}", docs = "Environment variables passed to the test runner"),
        attr("swift_flags", "list<string>", default = "[]", docs = "Extra Swift compiler flags"),
        attr("swift_testing", "bool", default = "false", docs = "Run sources that use Swift Testing (`import Testing`) through the generic Once test capability"),
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
