def _apple_default_arch(platform, sdk_variant):
    if host_os() == "linux" and platform == "ios" and sdk_variant == "device":
        return "arm64"
    return host_arch()

def _apple_linux_host_bin():
    swift = host_which("swift")
    info = json_decode(host_command([swift, "-print-target-info"]))
    return _parent_dir(_parent_dir(info["paths"]["runtimeResourcePath"])) + "/bin"

def _apple_linux_config_value(config, key):
    prefix = key + ": "
    for line in config.split("\n"):
        value = line.strip()
        if value.startswith(prefix):
            return value[len(prefix):].strip()
    return ""

def _apple_linux_sdk(platform, sdk_variant, developer_dir):
    sdk_name = _apple_sdk_name(platform, sdk_variant)
    triple = _apple_triple(platform, "", sdk_variant, _apple_default_arch(platform, sdk_variant), False)
    if developer_dir:
        bundle = _parent_dir(developer_dir)
        metadata_path = bundle + "/swift-sdk.json"
        if not host_file_exists(metadata_path):
            fail("Linux Apple builds require an xtool Darwin SDK, not Xcode's macOS executables; set xcode_developer_dir to the SDK bundle's Developer directory or omit it to use `swift sdk configure darwin`")
        definition = json_decode(host_file_read(metadata_path))
        target = (definition.get("targetTriples") or {}).get(triple)
        if not target:
            fail("Darwin SDK does not provide " + triple + "; select a platform and sdk_variant listed in swift-sdk.json")
        sdk_path = target["sdkRootPath"]
        resource_dir = target["swiftResourcesPath"]
        if not sdk_path.startswith("/"):
            sdk_path = bundle + "/" + sdk_path
        if not resource_dir.startswith("/"):
            resource_dir = bundle + "/" + resource_dir
    else:
        swift = host_which("swift")
        if "darwin" not in [line.strip() for line in host_command([swift, "sdk", "list"]).split("\n")]:
            fail("Linux Apple builds require the Darwin Swift SDK; install matching Swift and xtool, then run `xtool sdk install /path/to/Xcode.xip` and verify `swift sdk list` includes darwin")
        config = host_command([swift, "sdk", "configure", "darwin", triple, "--show-configuration"])
        sdk_path = _apple_linux_config_value(config, "sdkRootPath")
        resource_dir = _apple_linux_config_value(config, "swiftResourcesPath")
        marker = "/Developer/Toolchains/"
        if not sdk_path or marker not in resource_dir:
            fail("Darwin SDK configuration is incomplete; re-register the SDK with xtool or run omarchy-apple-dev's `install-toolchain.sh --repair`")
        bundle = resource_dir.split(marker)[0]
        developer_dir = bundle + "/Developer"
    tools = bundle + "/toolset/bin"
    for path in [sdk_path, resource_dir, tools]:
        if not host_path_exists(path):
            fail("Darwin SDK path is missing: " + path + "; re-install or repair the SDK with xtool")
    identities = [sdk_path, resource_dir, tools, host_tree_sha256(sdk_path), host_tree_sha256(resource_dir), host_tree_sha256(tools)]
    platform_developer = _parent_dir(_parent_dir(sdk_path))
    for path in [platform_developer + "/usr/lib/swift/host", bundle + "/OpenAppleMacrosServer"]:
        if host_path_exists(path):
            identities.append(host_tree_sha256(path) if path.endswith("/host") else host_file_sha256(path))
    identity = "once.apple.linux.sdk.v1\x00" + content_sha256(_json_encode(identities))
    return {
        "sdk_name": sdk_name,
        "sdk_path": sdk_path,
        "resource_dir": resource_dir,
        "tools": tools,
        "developer_dir": developer_dir,
        "identity": identity,
    }

def _apple_linux_env(sdk, host_bin):
    env = _developer_env(sdk["developer_dir"])
    env["PATH"] = sdk["tools"] + ":" + host_bin + ":/usr/bin:/bin"
    library_path = host_env("LD_LIBRARY_PATH")
    if library_path:
        env["LD_LIBRARY_PATH"] = library_path
    return env

def _apple_linux_swiftc(platform, sdk_variant, developer_dir):
    sdk = _apple_linux_sdk(platform, sdk_variant, developer_dir)
    host_bin = _apple_linux_host_bin()
    compiler = host_bin + "/swiftc"
    env = _apple_linux_env(sdk, host_bin)
    argv = [compiler, "-disable-sandbox", "-sdk", sdk["sdk_path"], "-resource-dir", sdk["resource_dir"], "-tools-directory", sdk["tools"], "-use-ld=lld", "-L" + sdk["sdk_path"] + "/usr/lib/swift", "-Xfrontend", "-enable-cross-import-overlays"]
    platform_developer = _parent_dir(_parent_dir(sdk["sdk_path"]))
    plugins = platform_developer + "/usr/lib/swift/host/plugins"
    server = platform_developer + "/usr/bin/swift-plugin-server"
    if host_path_exists(plugins) and host_file_exists(server):
        argv.extend(["-external-plugin-path", plugins + "#" + server])
    return {
        "argv": argv,
        "swiftc_path": compiler,
        "sdk_name": sdk["sdk_name"],
        "sdk_path": sdk["sdk_path"],
        "resource_dir": sdk["resource_dir"],
        "identity": sdk["identity"] + "\x00" + host_file_sha256(compiler) + "\x00" + host_file_sha256(host_bin + "/swift-frontend") + "\x00" + host_command([compiler, "--version"], env = env).strip() + "\x00" + _json_encode(env),
        "env": env,
    }

def _apple_linux_clang(platform, sdk_variant, developer_dir):
    sdk = _apple_linux_sdk(platform, sdk_variant, developer_dir)
    host_bin = _apple_linux_host_bin()
    compiler = host_bin + "/clang"
    env = _apple_linux_env(sdk, host_bin)
    return {
        "clang_path": compiler,
        "clangxx_path": host_bin + "/clang++",
        "sdk_name": sdk["sdk_name"],
        "sdk_path": sdk["sdk_path"],
        "identity": sdk["identity"] + "\x00" + host_file_sha256(compiler) + "\x00" + host_command([compiler, "--version"]).strip() + "\x00" + _json_encode(env),
        "env": env,
    }

def _apple_linux_tool(name, platform, sdk_variant, developer_dir):
    sdk = _apple_linux_sdk(platform, sdk_variant, developer_dir)
    path = sdk["tools"] + "/" + name
    if name == "lipo" and not host_file_exists(path):
        path = host_which_optional("llvm-lipo")
        if not path:
            fail("Universal Apple archives on Linux require llvm-lipo on PATH; install LLVM tools or set archs = [\"arm64\"] for a single-architecture device library")
    elif not host_file_exists(path):
        fail("Darwin SDK tool is missing: " + path + "; repair the SDK with xtool")
    env = _apple_linux_env(sdk, _apple_linux_host_bin())
    return {
        "argv": [path],
        name + "_path": path,
        "identity": sdk["identity"] + "\x00" + path + "\x00" + host_file_sha256(path) + "\x00" + _json_encode(env),
        "env": env,
    }

def _apple_linux_resource_tool(name, developer_dir):
    sdk = _apple_linux_sdk("ios", "device", developer_dir)
    path = _parent_dir(_parent_dir(sdk["sdk_path"])) + "/usr/bin/" + name
    companion_identity = host_tree_sha256(_parent_dir(path)) if host_file_exists(path) else ""
    if not host_file_exists(path):
        path = host_which_optional(name)
    if not path:
        fail("Linux Apple resources require " + name + "; install the compatibility tools with omarchy-apple-dev's `install-toolchain.sh`")
    env = _apple_linux_env(sdk, _apple_linux_host_bin())
    return {
        "path": path,
        "actool_path": path,
        "identity": sdk["identity"] + "\x00" + path + "\x00" + host_file_sha256(path) + "\x00" + companion_identity + "\x00" + _json_encode(env),
        "env": env,
    }

def _apple_codesign_argv(codesign, path, entitlements = ""):
    if codesign.get("backend") == "rcodesign":
        argv = [codesign["codesign_path"], "sign", "--shallow", "--timestamp-url", "none"]
        if entitlements:
            argv.extend(["--entitlements-xml-file", entitlements])
    else:
        argv = [codesign["codesign_path"], "--force", "--sign", "-", "--timestamp=none"]
        if entitlements:
            argv.extend(["--entitlements", entitlements])
    return argv + [path]
