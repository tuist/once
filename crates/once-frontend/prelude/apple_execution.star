def _apple_execution_toolchain():
    execution = execution_context()
    if not execution or (execution.get("executor") or execution.get("provider")) == "local":
        return None
    toolchain = (execution.get("properties") or {}).get("apple")
    if type(toolchain) != "dict" or toolchain.get("schema_version") != 1:
        fail("Remote Apple builds require infrastructure.execution.properties.apple with schema_version = 1 and an image-bound Linux toolchain description")
    if not execution.get("environment") or toolchain.get("environment") != execution["environment"]:
        fail("Apple execution toolchain environment must match the selected execution environment; select the matching provider or update its toolchain description")
    if toolchain.get("os") != "linux":
        fail("The Apple execution toolchain description currently supports Linux workers only")
    if toolchain.get("arch") not in ["arm64", "x86_64"]:
        fail("Apple execution toolchain arch must be arm64 or x86_64")
    for name in ["bin", "developer_dir", "tools"]:
        _apple_worker_path(toolchain.get(name), name)
    if type(toolchain.get("sdks")) != "dict":
        fail("Apple execution toolchain sdks must describe the supported SDKs")
    return toolchain

def _apple_worker_path(path, name):
    if type(path) != "string" or not path.startswith("/") or ".." in path.split("/") or "\x00" in path:
        fail("Apple execution toolchain " + name + " must be an absolute worker path without parent traversal")
    return path

def _apple_execution_os():
    toolchain = _apple_execution_toolchain()
    return toolchain["os"] if toolchain else host_os()

def _apple_execution_arch():
    toolchain = _apple_execution_toolchain()
    return toolchain["arch"] if toolchain else host_arch()

def _apple_worker_identity():
    return "once.apple.execution.v1\x00" + content_sha256(_json_encode(execution_context()))

def _apple_worker_sdk(platform, sdk_variant, developer_dir):
    toolchain = _apple_execution_toolchain()
    name = _apple_sdk_name(platform, sdk_variant)
    sdk = toolchain["sdks"].get(name)
    if type(sdk) != "dict":
        fail("Apple execution toolchain does not provide " + name + "; configure that SDK in infrastructure.execution.properties.apple.sdks")
    if developer_dir and developer_dir != toolchain["developer_dir"]:
        fail("xcode_developer_dir must match the selected Apple worker developer_dir; omit the coordinator Xcode override for remote builds")
    for key in ["sdk_path", "resource_dir"]:
        _apple_worker_path(sdk.get(key), "sdks." + name + "." + key)
    for key in ["plugins", "plugin_server"]:
        if sdk.get(key):
            _apple_worker_path(sdk[key], "sdks." + name + "." + key)
    if bool(sdk.get("plugins")) != bool(sdk.get("plugin_server")):
        fail("Apple worker SDK plugins and plugin_server must be configured together")
    return dict(sdk, sdk_name = name, tools = toolchain["tools"], developer_dir = toolchain["developer_dir"], identity = _apple_worker_identity())

def _apple_worker_program(name):
    toolchain = _apple_execution_toolchain()
    path = (toolchain.get("programs") or {}).get(name)
    if not path:
        fail("Apple execution toolchain requires programs." + name + "; install the Linux-compatible tool in the selected image and describe its absolute path")
    return _apple_worker_path(path, "programs." + name)
