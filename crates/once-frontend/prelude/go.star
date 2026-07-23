_GO_TOOL = tool("go", executables = ["go"])

_GO_SOURCE_EXTENSIONS = [
    ".go",
    ".s",
    ".S",
    ".syso",
    ".c",
    ".cc",
    ".cpp",
    ".cxx",
    ".h",
    ".hh",
    ".hpp",
    ".hxx",
    ".inc",
    ".m",
    ".mm",
]

def _go_attr(ctx, key, default):
    value = ctx["attr"].get(key)
    if value == None:
        return default
    return _go_resolve_select(value, _go_config_tokens(ctx), ctx["label"]["id"], key)

def _go_is_select(value):
    return type(value) == type({}) and len(value) == 1 and type(value.get("select")) == type({})

def _go_resolve_select(value, tokens, label_id, attr_name):
    if _go_is_select(value):
        branches = value["select"]
        for token in tokens:
            if token in branches:
                return _go_resolve_select(branches[token], tokens, label_id, attr_name)
        fail(label_id + ": select() on `" + attr_name + "` has no matching Go configuration and no `default`")
    if type(value) == type([]):
        return [_go_resolve_select(item, tokens, label_id, attr_name) for item in value]
    if type(value) == type({}):
        return {key: _go_resolve_select(item, tokens, label_id, attr_name) for key, item in value.items()}
    return value

def _go_host_os():
    value = host_os()
    if value == "macos":
        return "darwin"
    return value

def _go_host_arch():
    value = host_arch()
    if value == "arm64":
        return "arm64"
    return value

def _go_target_os(ctx):
    value = ctx["attr"].get("goos")
    if value and value != "auto":
        return value
    return _go_host_os()

def _go_target_arch(ctx):
    value = ctx["attr"].get("goarch")
    if value and value != "auto":
        return value
    return _go_host_arch()

def _go_config_tokens(ctx):
    goos = _go_target_os(ctx)
    goarch = _go_target_arch(ctx)
    return [goos + "-" + goarch, goos, goarch, "default"]

def _go_trim_slashes(value):
    out = value.replace("\\", "/")
    while_guard = len(out) + 1
    for _ in range(while_guard):
        if not out.startswith("./"):
            break
        out = out[2:]
    for _ in range(while_guard):
        if not out.endswith("/"):
            break
        out = out[:len(out) - 1]
    return out

def _go_validate_workspace_path(path, description):
    normalized = _go_trim_slashes(path)
    if normalized.startswith("/") or (len(normalized) > 2 and normalized[1] == ":"):
        fail(description + " must be workspace-relative")
    for part in normalized.split("/"):
        if part == "..":
            fail(description + " must not escape the workspace")
    return normalized

def _go_default_module_root(ctx):
    return ctx["label"]["package"]

def _go_dependency_sets(deps):
    return [dep for dep in deps if dep.get("go_dependency_set")]

def _go_module_context(ctx):
    configured = ctx["attr"].get("module_root")
    configured = _go_validate_workspace_path(configured, "module_root") if configured else None
    sets = _go_dependency_sets(_go_all_deps(ctx))
    if len(sets) > 1:
        fail(ctx["label"]["id"] + ": Go targets accept at most one go_dependencies provider")
    if sets:
        dependency = sets[0]
        root = dependency.get("module_root") or ""
        if configured != None and configured != root:
            fail(ctx["label"]["id"] + ": module_root does not match the go_dependencies provider")
        return {
            "root": root,
            "manifest": dependency.get("manifest") or "",
            "sum_files": dependency.get("sum_files") or [],
            "vendor_manifest": dependency.get("vendor_manifest") or "",
            "vendor_dir": dependency.get("vendor_dir") or "",
            "workspace_file": dependency.get("workspace_file") or "",
            "inputs": dependency.get("transitive_sources") or [],
        }
    root = configured if configured != None else _go_default_module_root(ctx)
    return {
        "root": root,
        "manifest": _go_join(root, "go.mod"),
        "sum_files": [_go_join(root, "go.sum")],
        "vendor_manifest": _go_join(root, "vendor/modules.txt"),
        "vendor_dir": _go_join(root, "vendor"),
        "workspace_file": _go_attr(ctx, "go_work", ""),
        "inputs": [],
    }

def _go_join(root, path):
    if not root or root == ".":
        return path
    if not path:
        return root
    return root + "/" + path

def _go_cwd(module):
    return module["root"] or "."

def _go_relative_to_module(module, path):
    root = module["root"]
    if not root or root == ".":
        return path
    prefix = ""
    for part in root.split("/"):
        if part:
            prefix += "../"
    return prefix + path

def _go_package_arg(ctx, module):
    configured = _go_attr(ctx, "package", "") or _go_attr(ctx, "package_root", "")
    if configured:
        return configured
    package = ctx["label"]["package"]
    root = module["root"]
    if package == root:
        return "."
    if root and package.startswith(root + "/"):
        return "./" + package[len(root) + 1:]
    if not root and package:
        return "./" + package
    return "."

def _go_resolved_deps(ctx):
    out = []
    seen = {}
    roles = ctx.get("deps_by_role") or {}
    for role in ["deps", "embed", "cdeps", "target_under_test"]:
        values = roles.get(role)
        if values == None and role == "deps":
            values = ctx.get("deps") or []
        for dep in values or []:
            identity = dep.get("label_id") or dep.get("importpath") or str(dep)
            if identity not in seen:
                seen[identity] = True
                out.append(dep)
    return out

def _go_all_deps(ctx):
    return _go_resolved_deps(ctx)

def _go_embed_deps(ctx):
    return (ctx.get("deps_by_role") or {}).get("embed") or []

def _go_collect(deps, key, own = []):
    out = []
    out.extend(own)
    for dep in deps:
        out.extend(dep.get(key) or [])
    return _unique(out)

def _go_source_inputs(ctx):
    return _filter_by_extensions(glob(ctx["srcs"]) + _file_globs(_go_attr(ctx, "headers", [])), _GO_SOURCE_EXTENSIONS)

def _go_embed_inputs(ctx):
    patterns = _go_attr(ctx, "embedsrcs", []) + _go_attr(ctx, "embed_srcs", [])
    return _file_globs(patterns)

def _go_data_inputs(ctx):
    return _file_globs(_go_attr(ctx, "data", []) + _go_attr(ctx, "resources", []))

def _go_existing_module_inputs(module):
    out = []
    for path in [module["manifest"], module["vendor_manifest"], module["workspace_file"]] + module["sum_files"]:
        if path and host_file_exists(workspace_root() + "/" + path):
            out.append(path)
    return out

def _go_build_inputs(ctx, module):
    deps = _go_all_deps(ctx)
    own = _go_source_inputs(ctx) + _go_embed_inputs(ctx)
    inputs = own + _go_collect(deps, "transitive_sources") + module["inputs"]
    inputs.extend(_go_existing_module_inputs(module))
    profile = _go_attr(ctx, "pgoprofile", "")
    if profile:
        inputs.append(_package_relative(ctx, profile))
    return _unique(inputs)

def _go_runtime_data(ctx):
    return _go_collect(_go_all_deps(ctx), "transitive_data", _go_data_inputs(ctx))

def _go_native_info(ctx):
    deps = [dep for dep in _go_all_deps(ctx) if dep.get("c_provider")]
    return {
        "headers": _go_collect(deps, "transitive_headers"),
        "include_dirs": _go_collect(deps, "transitive_include_dirs"),
        "quote_include_dirs": _go_collect(deps, "transitive_quote_include_dirs"),
        "system_include_dirs": _go_collect(deps, "transitive_system_include_dirs"),
        "framework_include_dirs": _go_collect(deps, "transitive_framework_include_dirs"),
        "defines": _go_collect(deps, "transitive_defines"),
        "static_libraries": _go_collect(deps, "transitive_static_libraries"),
        "dynamic_libraries": _go_collect(deps, "transitive_dynamic_libraries"),
        "linkopts": _go_collect(deps, "transitive_linkopts"),
        "data": _go_collect(deps, "transitive_data"),
    }

def _go_cgo_enabled(ctx):
    pure = _go_attr(ctx, "pure", "auto")
    if pure not in ["auto", "on", "off"]:
        fail(ctx["label"]["id"] + ": pure must be auto, on, or off")
    enabled = _go_attr(ctx, "cgo", False) or pure == "off"
    if pure == "on" and _go_attr(ctx, "cgo", False):
        fail(ctx["label"]["id"] + ": cgo cannot be true when pure is on")
    return enabled

def _go_toolchain(ctx):
    requested = _go_attr(ctx, "go", "")
    executable = _resolve_host_executable(requested) if requested else host_which("go")
    if not executable:
        fail(ctx["label"]["id"] + ": could not resolve the Go command")
    version = host_command([executable, "version"]).strip()
    values = json_decode(host_command([executable, "env", "-json", "GOOS", "GOARCH", "GOROOT"]))
    identity = "once.go.v1\x00" + executable + "\x00" + version + "\x00" + (values.get("GOROOT") or "")
    return {
        "go": executable,
        "version": version,
        "host_os": values.get("GOOS") or _go_host_os(),
        "host_arch": values.get("GOARCH") or _go_host_arch(),
        "goroot": values.get("GOROOT") or "",
        "identity": identity,
    }

def _go_action_env(ctx, module, scratch):
    env = {}
    for key, value in _go_attr(ctx, "env", {}).items():
        env[key] = value
    env["GOENV"] = "off"
    env["GOTOOLCHAIN"] = "local"
    env["GOPROXY"] = "off"
    env["GOSUMDB"] = "off"
    env["GOOS"] = _go_target_os(ctx)
    env["GOARCH"] = _go_target_arch(ctx)
    env["GOCACHE"] = execution_path(scratch + "/cache")
    env["GOMODCACHE"] = execution_path(scratch + "/modules")
    env["TMPDIR"] = execution_path(scratch + "/tmp")
    env["HOME"] = execution_path(scratch + "/home")
    workspace_file = module["workspace_file"]
    env["GOWORK"] = execution_path(workspace_file) if workspace_file else "off"
    cgo = _go_cgo_enabled(ctx)
    env["CGO_ENABLED"] = "1" if cgo else "0"
    if cgo:
        cc = _go_attr(ctx, "cc", "") or host_which("cc")
        cxx = _go_attr(ctx, "cxx", "") or host_which("c++")
        env["CC"] = cc
        env["CXX"] = cxx
        native = _go_native_info(ctx)
        cflags = []
        for include in native["include_dirs"] + native["quote_include_dirs"]:
            cflags.append("-I" + _go_relative_to_module(module, include))
        for include in native["system_include_dirs"]:
            cflags.append("-isystem " + _go_relative_to_module(module, include))
        for include in native["framework_include_dirs"]:
            cflags.append("-F" + _go_relative_to_module(module, include))
        for define in native["defines"]:
            cflags.append("-D" + define)
        cflags.extend(_go_attr(ctx, "cppopts", []))
        cflags.extend(_go_attr(ctx, "copts", []))
        cxxflags = cflags + _go_attr(ctx, "cxxopts", [])
        ldflags = []
        for library in native["static_libraries"] + native["dynamic_libraries"]:
            ldflags.append(_go_relative_to_module(module, library))
        ldflags.extend(native["linkopts"])
        ldflags.extend(_go_attr(ctx, "clinkopts", []))
        if cflags:
            env["CGO_CFLAGS"] = " ".join(cflags)
        if cxxflags:
            env["CGO_CXXFLAGS"] = " ".join(cxxflags)
        if ldflags:
            env["CGO_LDFLAGS"] = " ".join(ldflags)
    return env

def _go_mod_flag(ctx, module):
    mode = _go_attr(ctx, "mod_mode", "vendor" if module["vendor_manifest"] else "readonly")
    if mode not in ["vendor", "readonly", "mod"]:
        fail(ctx["label"]["id"] + ": mod_mode must be vendor, readonly, or mod")
    if mode == "mod":
        fail(ctx["label"]["id"] + ": mod_mode = mod may fetch undeclared inputs; use vendor or readonly")
    return "-mod=" + mode

def _go_mode_enabled(ctx, name):
    value = _go_attr(ctx, name, "auto")
    if value not in ["auto", "on", "off"]:
        fail(ctx["label"]["id"] + ": " + name + " must be auto, on, or off")
    return value == "on"

def _go_common_build_args(ctx, module):
    args = [_go_mod_flag(ctx, module), "-buildvcs=false"]
    if _go_attr(ctx, "trimpath", True):
        args.append("-trimpath")
    tags = _unique(_go_attr(ctx, "tags", []) + _go_attr(ctx, "gotags", []) + _go_attr(ctx, "build_tags", []))
    if tags:
        args.append("-tags=" + ",".join(tags))
    gcflags = _go_attr(ctx, "gc_goopts", []) + _go_attr(ctx, "compiler_flags", [])
    if gcflags:
        args.append("-gcflags=all=" + " ".join(gcflags))
    asmflags = _go_attr(ctx, "assembler_flags", [])
    if asmflags:
        args.append("-asmflags=all=" + " ".join(asmflags))
    if _go_mode_enabled(ctx, "race"):
        args.append("-race")
    if _go_mode_enabled(ctx, "msan"):
        args.append("-msan")
    if _go_mode_enabled(ctx, "asan"):
        args.append("-asan")
    profile = _go_attr(ctx, "pgoprofile", "")
    if profile:
        args.append("-pgo=" + _go_relative_to_module(module, _package_relative(ctx, profile)))
    if _go_attr(ctx, "coverage", False) or _go_attr(ctx, "coverage_enabled", False):
        args.append("-cover")
        mode = _go_attr(ctx, "coverage_mode", "")
        if mode:
            args.append("-covermode=" + mode)
    return args

def _go_link_flags(ctx):
    flags = []
    flags.extend(_go_attr(ctx, "gc_linkopts", []))
    flags.extend(_go_attr(ctx, "linker_flags", []))
    flags.extend(_go_attr(ctx, "external_linker_flags", []))
    for key, value in sorted(_go_attr(ctx, "x_defs", {}).items()):
        flags.extend(["-X", key + "=" + value])
    if _go_attr(ctx, "strip", False):
        flags.extend(["-s", "-w"])
    link_mode = _go_attr(ctx, "link_mode", "")
    if link_mode and link_mode not in ["auto", "internal", "external"]:
        fail(ctx["label"]["id"] + ": link_mode must be auto, internal, or external")
    if link_mode and link_mode != "auto":
        flags.extend(["-linkmode", link_mode])
    static = _go_attr(ctx, "static", "auto")
    if static not in ["auto", "on", "off"]:
        fail(ctx["label"]["id"] + ": static must be auto, on, or off")
    if static == "on":
        flags.extend(["-linkmode", "external", "-extldflags", "-static"])
    link_style = _go_attr(ctx, "link_style", "static")
    if link_style not in ["static", "shared", "static_pic"]:
        fail(ctx["label"]["id"] + ": link_style must be static, static_pic, or shared")
    return flags

def _go_build_mode(ctx, default):
    mode = _go_attr(ctx, "build_mode", "") or _go_attr(ctx, "linkmode", "") or default
    if mode == "auto" or mode == "normal":
        mode = "exe"
    if mode not in ["archive", "exe", "pie", "plugin", "c-archive", "c-shared", "shared"]:
        fail(ctx["label"]["id"] + ": unsupported Go build mode `" + mode + "`")
    return mode

def _go_output_extension(ctx, mode):
    goos = _go_target_os(ctx)
    if mode in ["exe", "pie"]:
        return ".exe" if goos == "windows" else ""
    if mode in ["archive", "c-archive"]:
        return ".lib" if goos == "windows" else ".a"
    if mode == "plugin":
        return ".so"
    if mode in ["c-shared", "shared"]:
        if goos == "windows":
            return ".dll"
        if goos == "darwin":
            return ".dylib"
        return ".so"
    return ""

def _go_output_name(ctx, mode):
    explicit = _go_attr(ctx, "out", "")
    if explicit:
        return explicit
    name = _go_attr(ctx, "basename", "") or _go_attr(ctx, "output_name", "") or ctx["label"]["name"]
    extension = _go_output_extension(ctx, mode)
    if mode in ["archive", "c-archive", "c-shared", "shared"] and _go_target_os(ctx) != "windows" and not name.startswith("lib"):
        name = "lib" + name
    if extension and not name.endswith(extension):
        name += extension
    return name

def _go_replace_extension(path, extension):
    slash = -1
    dot = -1
    for index in range(len(path)):
        if path[index] == "/":
            slash = index
            dot = -1
        elif path[index] == ".":
            dot = index
    if dot > slash:
        return path[:dot] + extension
    return path + extension

def _go_action_identifier(ctx, name):
    return ctx["label"]["id"] + ":go-" + name

def _go_declared_build(ctx, mode, explicit_mode = True):
    module = _go_module_context(ctx)
    toolchain = _go_toolchain(ctx)
    output = declare_output(_go_output_name(ctx, mode))
    relative_output = _go_relative_to_module(module, output)
    scratch = ctx["scratch_dir"] + "/go-build"
    args = [toolchain["go"], "build"] + _go_common_build_args(ctx, module)
    if explicit_mode and mode != "exe":
        args.append("-buildmode=" + mode)
    if _go_attr(ctx, "link_style", "static") == "shared":
        args.append("-linkshared")
    link_flags = _go_link_flags(ctx)
    if link_flags:
        args.append("-ldflags=" + " ".join(link_flags))
    args.extend(["-o", relative_output, _go_package_arg(ctx, module)])
    inputs = _go_build_inputs(ctx, module)
    native = _go_native_info(ctx)
    inputs.extend(native["headers"] + native["static_libraries"] + native["dynamic_libraries"])
    outputs = [output]
    header = ""
    if mode in ["c-archive", "c-shared"]:
        header = _go_replace_extension(output, ".h")
        outputs.append(header)
    run_action(
        argv = args,
        inputs = _unique(inputs),
        outputs = outputs,
        clean_paths = [scratch],
        create_dirs = [scratch + "/cache", scratch + "/modules", scratch + "/tmp", scratch + "/home"],
        cwd = _go_cwd(module),
        env = _go_action_env(ctx, module, scratch),
        toolchain_identity = toolchain["identity"] + "\x00" + _go_target_os(ctx) + "\x00" + _go_target_arch(ctx),
        identifier = _go_action_identifier(ctx, "build"),
    )
    return (output, header, inputs, native, toolchain, module)

def _go_base_provider(ctx, kind, output, source_inputs, native):
    sources = _unique(_go_source_inputs(ctx) + _go_embed_inputs(ctx))
    transitive_sources = _go_collect(_go_all_deps(ctx), "transitive_sources", sources)
    data = _go_runtime_data(ctx)
    importpath = _go_attr(ctx, "importpath", "") or _go_attr(ctx, "package_name", "")
    return {
        "go_package": True,
        "label_id": ctx["label"]["id"],
        "target_kind": kind,
        "importpath": importpath,
        "importmap": _go_attr(ctx, "importmap", "") or importpath,
        "package": _go_package_arg(ctx, _go_module_context(ctx)),
        "archive": output if kind == "go_library" else "",
        "binary": output if kind in ["go_binary", "go_test"] else "",
        "sources": sources,
        "transitive_sources": transitive_sources,
        "data": _go_data_inputs(ctx),
        "transitive_data": data,
        "affected_inputs": _unique(source_inputs + data + native["data"]),
        "cgo": _go_cgo_enabled(ctx),
        "default_output": output,
    }

def _go_source_impl(ctx):
    module = _go_module_context(ctx)
    sources = _unique(_go_source_inputs(ctx) + _go_embed_inputs(ctx))
    provider = _go_base_provider(ctx, "go_source", "", sources, _go_native_info(ctx))
    provider["package"] = _go_package_arg(ctx, module)
    return provider

def _go_library_impl(ctx):
    output, _, inputs, native, _, _ = _go_declared_build(ctx, "archive", explicit_mode = False)
    return _go_base_provider(ctx, "go_library", output, inputs, native)

def _go_host_env(names):
    out = {}
    for name in names:
        value = host_env(name)
        if value:
            out[name] = value
    return out

def _go_runtime_env(ctx, run_dir):
    env = {"HOME": execution_path(run_dir + "/home")}
    for key, value in _go_host_env(_go_attr(ctx, "env_inherit", [])).items():
        env[key] = value
    for key, value in _go_attr(ctx, "env", {}).items():
        env[key] = value
    for key, value in _go_attr(ctx, "run_env", {}).items():
        env[key] = value
    return env

def _go_native_provider_fields(ctx, provider, mode, output, header, native):
    is_static = mode == "c-archive"
    is_dynamic = mode == "c-shared"
    if not is_static and not is_dynamic:
        return provider
    static_libraries = [output] if is_static else []
    dynamic_libraries = [output] if is_dynamic else []
    linkopts = list(native["linkopts"])
    if _go_target_os(ctx) not in ["darwin", "windows"]:
        linkopts.append("-pthread")
    provider.update({
        "c_provider": True,
        "headers": [header] if header else [],
        "include_dirs": [_parent_dir(header)] if header else [],
        "static_libraries": static_libraries,
        "dynamic_libraries": dynamic_libraries,
        "transitive_headers": _unique(([header] if header else []) + native["headers"]),
        "transitive_include_dirs": _unique(([_parent_dir(header)] if header else []) + native["include_dirs"]),
        "transitive_quote_include_dirs": native["quote_include_dirs"],
        "transitive_system_include_dirs": native["system_include_dirs"],
        "transitive_framework_include_dirs": native["framework_include_dirs"],
        "transitive_defines": native["defines"],
        "transitive_static_libraries": _unique(static_libraries + native["static_libraries"]),
        "transitive_dynamic_libraries": _unique(dynamic_libraries + native["dynamic_libraries"]),
        "transitive_linkopts": linkopts,
        "transitive_archives": _unique(static_libraries + native["static_libraries"]),
        "archive": output if is_static else "",
        "dylib": output if is_dynamic else "",
    })
    abi = _go_attr(ctx, "android_abi", "")
    if not abi and _go_target_os(ctx) == "android":
        abi = {
            "arm64": "arm64-v8a",
            "arm": "armeabi-v7a",
            "386": "x86",
            "amd64": "x86_64",
        }.get(_go_target_arch(ctx)) or ""
    android_libraries = [{"abi": abi, "path": output}] if is_dynamic and abi else []
    provider["android_native_libraries"] = android_libraries
    provider["transitive_android_native_libraries"] = android_libraries
    return provider

def _go_binary_impl(ctx):
    mode = _go_build_mode(ctx, "exe")
    if _go_attr(ctx, "generate_exported_header", False) and mode not in ["c-archive", "c-shared"]:
        fail(ctx["label"]["id"] + ": generate_exported_header requires c-archive or c-shared build mode")
    output, header, inputs, native, toolchain, _ = _go_declared_build(ctx, mode)
    provider = _go_base_provider(ctx, "go_binary", output, inputs, native)
    provider = _go_native_provider_fields(ctx, provider, mode, output, header, native)
    if ctx["capability"] != "run":
        return provider
    if mode not in ["exe", "pie"]:
        fail(ctx["label"]["id"] + ": only executable and position-independent executable build modes can run")
    run_dir = ctx["build_dir"] + "/run"
    log = run_dir + "/stdout.log"
    marker = run_dir + "/run.json"
    prepare_path(run_dir, kind = "directory", identifier = _go_action_identifier(ctx, "run-prepare"))
    run_action(
        argv = [output] + _go_attr(ctx, "args", []),
        inputs = _unique([output] + _go_runtime_data(ctx) + native["dynamic_libraries"]),
        outputs = [run_dir, log],
        stdout = log,
        stderr = log,
        env = _go_runtime_env(ctx, run_dir),
        cacheable = False,
        toolchain_identity = toolchain["identity"] + "\x00once.go.run.v1",
        identifier = _go_action_identifier(ctx, "run"),
    )
    write_path(marker, _run_result_json(ctx["label"]["id"]))
    return provider

def _go_test_runner_source():
    return r'''package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

type event struct {
	Action string `json:"Action"`
	Test string `json:"Test"`
	Output string `json:"Output"`
}

type attempt struct {
	Status string `json:"status"`
}

type testCase struct {
	ID string `json:"id"`
	Name string `json:"name"`
	Suite string `json:"suite"`
	Status string `json:"status"`
	Attempts []attempt `json:"attempts"`
	RunnerMetadata map[string]string `json:"runner_metadata"`
}

type summary struct {
	Total int `json:"total"`
	Passed int `json:"passed"`
	Failed int `json:"failed"`
	Skipped int `json:"skipped"`
	Flaky int `json:"flaky"`
}

type report struct {
	Schema string `json:"schema"`
	Target string `json:"target"`
	Runner map[string]any `json:"runner"`
	Status string `json:"status"`
	Summary summary `json:"summary"`
	Cases []testCase `json:"cases"`
	Artifacts map[string][]string `json:"artifacts"`
}

func main() {
	if len(os.Args) < 9 {
		fmt.Fprintln(os.Stderr, "usage: once-go-test-runner <go> <package> <binary> <results> <log> <native-results> <coverage> <target> [--once-rundir dir] [--once-filter name]... -- [args...]")
		os.Exit(2)
	}
	goTool, packagePath, binary := os.Args[1], os.Args[2], os.Args[3]
	results, logPath, nativePath := os.Args[4], os.Args[5], os.Args[6]
	coverage, target := os.Args[7], os.Args[8]
	filters, runDir, testArgs := parseArgs(os.Args[9:])
	for _, path := range []string{results, logPath, nativePath, coverage} {
		if path != "" {
			if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
				fmt.Fprintln(os.Stderr, err)
				os.Exit(2)
			}
		}
	}
	absBinary, _ := filepath.Abs(binary)
	listed := listTests(absBinary, runDir)
	runArgs := append([]string{}, testArgs...)
	runArgs = append(runArgs, "-test.v=test2json")
	if coverage != "" {
		absCoverage, _ := filepath.Abs(coverage)
		runArgs = append(runArgs, "-test.coverprofile="+absCoverage)
	}
	if len(filters) > 0 {
		tests, benchmarks := splitFilters(filters)
		if len(tests) > 0 {
			runArgs = append(runArgs, "-test.run="+exactPattern(tests))
		} else {
			runArgs = append(runArgs, "-test.run=^$")
		}
		if len(benchmarks) > 0 {
			runArgs = append(runArgs, "-test.bench="+exactPattern(benchmarks))
		}
	}
	commandArgs := []string{"tool", "test2json", "-p", packagePath, absBinary}
	commandArgs = append(commandArgs, runArgs...)
	command := exec.Command(goTool, commandArgs...)
	if runDir != "" {
		command.Dir = runDir
	}
	native, runErr := command.CombinedOutput()
	statuses, humanLog, packageFailed := parseEvents(native)
	if len(filters) == 0 {
		for _, name := range listed {
			if _, ok := statuses[name]; !ok && runErr == nil && !packageFailed {
				if strings.HasPrefix(name, "Benchmark") {
					statuses[name] = "skipped"
				} else {
					statuses[name] = "passed"
				}
			}
		}
	} else {
		for _, name := range filters {
			if _, ok := statuses[name]; !ok && runErr == nil && !packageFailed {
				statuses[name] = "passed"
			}
		}
	}
	if err := os.WriteFile(logPath, []byte(humanLog), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	if err := os.WriteFile(nativePath, native, 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	record := buildReport(target, packagePath, statuses, runErr == nil && !packageFailed, logPath, nativePath, coverage)
	encoded, _ := json.Marshal(record)
	encoded = append(encoded, '\n')
	if err := os.WriteFile(results, encoded, 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	if runErr != nil || packageFailed {
		os.Exit(1)
	}
}

func parseArgs(args []string) ([]string, string, []string) {
	filters := []string{}
	runDir := ""
	for index := 0; index < len(args); index++ {
		if args[index] == "--" {
			return filters, runDir, args[index+1:]
		}
		if args[index] == "--once-filter" && index+1 < len(args) {
			filters = append(filters, args[index+1])
			index++
			continue
		}
		if args[index] == "--once-rundir" && index+1 < len(args) {
			runDir = args[index+1]
			index++
		}
	}
	return filters, runDir, nil
}

func listTests(binary, runDir string) []string {
	command := exec.Command(binary, "-test.list=.")
	if runDir != "" {
		command.Dir = runDir
	}
	output, _ := command.Output()
	lines := strings.Split(string(output), "\n")
	out := []string{}
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "Test") || strings.HasPrefix(line, "Benchmark") || strings.HasPrefix(line, "Example") || strings.HasPrefix(line, "Fuzz") {
			out = append(out, line)
		}
	}
	return out
}

func splitFilters(filters []string) ([]string, []string) {
	tests, benchmarks := []string{}, []string{}
	for _, name := range filters {
		if strings.HasPrefix(name, "Benchmark") {
			benchmarks = append(benchmarks, name)
		} else {
			tests = append(tests, name)
		}
	}
	return tests, benchmarks
}

func exactPattern(names []string) string {
	patterns := make([]string, 0, len(names))
	for _, name := range names {
		parts := strings.Split(name, "/")
		for index, part := range parts {
			parts[index] = regexp.QuoteMeta(part)
		}
		patterns = append(patterns, "^"+strings.Join(parts, "$/^")+"$")
	}
	return strings.Join(patterns, "|")
}

func parseEvents(native []byte) (map[string]string, string, bool) {
	statuses := map[string]string{}
	log := strings.Builder{}
	packageFailed := false
	scanner := bufio.NewScanner(strings.NewReader(string(native)))
	buffer := make([]byte, 0, 64*1024)
	scanner.Buffer(buffer, 16*1024*1024)
	for scanner.Scan() {
		var value event
		if json.Unmarshal(scanner.Bytes(), &value) != nil {
			continue
		}
		log.WriteString(value.Output)
		if value.Test != "" {
			switch value.Action {
			case "pass":
				statuses[value.Test] = "passed"
			case "fail":
				statuses[value.Test] = "failed"
			case "skip":
				statuses[value.Test] = "skipped"
			}
		} else if value.Action == "fail" {
			packageFailed = true
		}
	}
	return statuses, log.String(), packageFailed
}

func buildReport(target, packagePath string, statuses map[string]string, passedRun bool, logPath, nativePath, coveragePath string) report {
	names := make([]string, 0, len(statuses))
	for name := range statuses {
		names = append(names, name)
	}
	sort.Strings(names)
	cases := make([]testCase, 0, len(names))
	summaryValue := summary{}
	for _, name := range names {
		status := statuses[name]
		switch status {
		case "passed":
			summaryValue.Passed++
		case "failed":
			summaryValue.Failed++
		case "skipped":
			summaryValue.Skipped++
		}
		cases = append(cases, testCase{
			ID: target + "::" + name,
			Name: name,
			Suite: packagePath,
			Status: status,
			Attempts: []attempt{{Status: status}},
			RunnerMetadata: map[string]string{"package": packagePath},
		})
	}
	summaryValue.Total = len(cases)
	if !passedRun && summaryValue.Total == 0 {
		summaryValue.Total = 1
		summaryValue.Failed = 1
	}
	status := "passed"
	if !passedRun {
		status = "failed"
	}
	artifacts := map[string][]string{"logs": {logPath}, "native_results": {nativePath}}
	if coveragePath != "" {
		artifacts["coverage"] = []string{coveragePath}
	}
	return report{
		Schema: "once.test_results.v1",
		Target: target,
		Runner: map[string]any{"type": "go_test", "metadata": map[string]any{"package": packagePath}},
		Status: status,
		Summary: summaryValue,
		Cases: cases,
		Artifacts: artifacts,
	}
}
'''

def _go_test_filter_args(ctx):
    out = []
    for value in (ctx.get("test") or {}).get("filters") or []:
        out.extend(["--once-filter", _test_unit_suffix(ctx, value)])
    return out

def _go_test_runtime_args(ctx):
    args = []
    if _go_attr(ctx, "fail_fast", False):
        args.append("-test.failfast")
    if _go_attr(ctx, "short", False):
        args.append("-test.short")
    count = _go_attr(ctx, "count", 0)
    if count > 0:
        args.append("-test.count=" + str(count))
    parallel = _go_attr(ctx, "parallel", 0)
    if parallel > 0:
        args.append("-test.parallel=" + str(parallel))
    shuffle = _go_attr(ctx, "shuffle", "")
    if shuffle:
        args.append("-test.shuffle=" + shuffle)
    args.extend(_go_attr(ctx, "args", []))
    return args

def _go_test_env(ctx, module, scratch, test_dir):
    env = _go_action_env(ctx, module, scratch)
    for key, value in _go_host_env(_go_attr(ctx, "env_inherit", [])).items():
        env[key] = value
    for key, value in _go_attr(ctx, "test_env", {}).items():
        env[key] = value
    env["HOME"] = execution_path(test_dir + "/home")
    return env

def _go_test_info(ctx, toolchain, module, binary, runner, results, log, native, coverage, test_dir):
    package = _go_package_arg(ctx, module)
    argv = [toolchain["go"], "run", runner, toolchain["go"], package, binary, results, log, native, coverage, ctx["label"]["id"]]
    rundir = _go_attr(ctx, "rundir", "")
    if rundir:
        argv.extend(["--once-rundir", _package_relative(ctx, rundir)])
    argv.extend(_go_test_filter_args(ctx))
    argv.append("--")
    argv.extend(_go_test_runtime_args(ctx))
    return {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {
            "type": "go_test",
            "display_name": "Go test",
            "metadata": {"package": package},
        },
        "command": {"argv": argv, "env": _go_test_env(ctx, module, ctx["scratch_dir"] + "/go-test-runner", test_dir), "cwd": "."},
        "outputs": {
            "results": results,
            "logs": [log],
            "native_results": [native],
            "coverage": [coverage] if coverage else [],
        },
        "listing": {"supported": True, "strategy": "go_test_list"},
        "filtering": {"case_filtering": "runner_args"},
        "sharding": {"supported": False},
        "retries": {"supported": False, "default_attempts": 1},
        "execution": {
            "cacheable": True,
            "timeout_ms": _go_attr(ctx, "timeout_ms", None),
            "run_from_workspace_root": True,
        },
        "labels": _go_attr(ctx, "labels", []),
        "metadata": {"package": package},
    }

def _go_test_impl(ctx):
    module = _go_module_context(ctx)
    toolchain = _go_toolchain(ctx)
    binary = declare_output(_go_output_name(ctx, "exe") + ".test" + (".exe" if _go_target_os(ctx) == "windows" else ""))
    scratch = ctx["scratch_dir"] + "/go-test-build"
    args = [toolchain["go"], "test", "-c"] + _go_common_build_args(ctx, module)
    cover_packages = _go_attr(ctx, "cover_packages", [])
    if cover_packages:
        args.append("-coverpkg=" + ",".join(cover_packages))
    link_flags = _go_link_flags(ctx)
    if link_flags:
        args.append("-ldflags=" + " ".join(link_flags))
    args.extend(["-o", _go_relative_to_module(module, binary), _go_package_arg(ctx, module)])
    inputs = _go_build_inputs(ctx, module)
    native_info = _go_native_info(ctx)
    inputs.extend(native_info["headers"] + native_info["static_libraries"] + native_info["dynamic_libraries"])
    run_action(
        argv = args,
        inputs = _unique(inputs),
        outputs = [binary],
        clean_paths = [scratch],
        create_dirs = [scratch + "/cache", scratch + "/modules", scratch + "/tmp", scratch + "/home"],
        cwd = _go_cwd(module),
        env = _go_action_env(ctx, module, scratch),
        toolchain_identity = toolchain["identity"] + "\x00once.go.test-build.v1",
        identifier = _go_action_identifier(ctx, "test-build"),
    )
    provider = _go_base_provider(ctx, "go_test", binary, inputs, native_info)
    provider["test_binary"] = binary
    test_dir = _test_output_dir(ctx)
    results = test_dir + "/test_results.json"
    log = test_dir + "/go-test.log"
    native = test_dir + "/native_results.jsonl"
    coverage_enabled = _go_attr(ctx, "coverage", False) or _go_attr(ctx, "coverage_enabled", False)
    coverage = test_dir + "/coverage.out" if coverage_enabled else ""
    runner = declare_output("once_go_test_runner.go")
    write_path(runner, _go_test_runner_source())
    provider["test_discovery_inputs"] = _go_source_inputs(ctx)
    provider["test_info"] = _go_test_info(ctx, toolchain, module, binary, runner, results, log, native, coverage, test_dir)
    if ctx["capability"] != "test":
        return provider
    if _go_target_os(ctx) != toolchain["host_os"] or _go_target_arch(ctx) != toolchain["host_arch"]:
        fail(ctx["label"]["id"] + ": Go test execution requires the host operating system and architecture")
    runner_scratch = ctx["scratch_dir"] + "/go-test-runner"
    outputs = [test_dir, results, log, native] + ([coverage] if coverage else [])
    command = provider["test_info"]["command"]
    run_action(
        argv = command["argv"],
        inputs = _unique([runner, binary] + _go_runtime_data(ctx) + native_info["dynamic_libraries"]),
        outputs = outputs,
        clean_paths = [runner_scratch],
        create_dirs = [runner_scratch + "/cache", runner_scratch + "/modules", runner_scratch + "/tmp", runner_scratch + "/home"],
        env = command["env"],
        toolchain_identity = toolchain["identity"] + "\x00once.go.test-runner.v1",
        identifier = _go_action_identifier(ctx, "test"),
    )
    return provider

def _go_unquote(value):
    if len(value) >= 2 and ((value.startswith("\"") and value.endswith("\"")) or (value.startswith("`") and value.endswith("`"))):
        return value[1:len(value) - 1]
    return value

def _go_mod_tokens(raw):
    line = raw.replace("\t", " ").replace("\r", " ").strip()
    if not line or line.startswith("//"):
        return []
    comment = -1
    for index in range(len(line) - 1):
        if line[index:index + 2] == "//" and (index == 0 or line[index - 1] == " "):
            comment = index
            break
    if comment >= 0:
        line = line[:comment].strip()
    return [_go_unquote(value) for value in line.split(" ") if value]

def _go_parse_replace(tokens):
    arrow = -1
    for index in range(len(tokens)):
        if tokens[index] == "=>":
            arrow = index
            break
    if arrow < 1 or arrow + 1 >= len(tokens):
        fail("invalid replace directive in Go module manifest")
    old = tokens[:arrow]
    new = tokens[arrow + 1:]
    return {
        "path": old[0],
        "version": old[1] if len(old) > 1 else "",
        "replacement_path": new[0],
        "replacement_version": new[1] if len(new) > 1 else "",
    }

def _go_parse_manifest(content, path):
    module_path = ""
    go_version = ""
    requires = {}
    replacements = {}
    block = ""
    for raw in content.split("\n"):
        tokens = _go_mod_tokens(raw)
        if not tokens:
            continue
        if block:
            if tokens[0] == ")":
                block = ""
                continue
            directive = block
            values = tokens
        else:
            directive = tokens[0]
            values = tokens[1:]
            if values and values[0] == "(":
                block = directive
                continue
        if directive == "module" and values:
            module_path = values[0]
        elif directive == "go" and values:
            go_version = values[0]
        elif directive == "require" and len(values) >= 2:
            requires[values[0]] = values[1]
        elif directive == "replace":
            replacement = _go_parse_replace(values)
            replacements[replacement["path"] + "\x00" + replacement["version"]] = replacement
    if not module_path:
        fail(path + ": expected a module directive")
    if not go_version:
        go_version = "1.16"
    version_parts = go_version.split(".")
    if int(version_parts[0]) < 1 or (int(version_parts[0]) == 1 and int(version_parts[1]) < 17):
        fail(path + ": go_dependencies requires a Go 1.17 or newer module manifest so transitive requirements are complete")
    return {
        "module": module_path,
        "go": go_version,
        "requires": requires,
        "replacements": replacements,
    }

def _go_parse_sums(contents):
    sums = {}
    for content in contents:
        for raw in content.split("\n"):
            values = [value for value in raw.strip().split(" ") if value]
            if len(values) != 3:
                continue
            version = values[1]
            if version.endswith("/go.mod"):
                continue
            sums[values[0] + "\x00" + version] = values[2]
    return sums

def _go_vendor_modules(content):
    records = {}
    order = []
    current = None
    for raw in content.split("\n"):
        line = raw.strip()
        if not line:
            continue
        if line.startswith("# ") and not line.startswith("##"):
            header = line[2:].strip()
            values = [value for value in header.split(" ") if value]
            replacement = None
            if "=>" in values:
                replacement = _go_parse_replace(values)
                left = []
                for value in values:
                    if value == "=>":
                        break
                    left.append(value)
                values = left
            path = values[0]
            version = values[1] if len(values) > 1 else ""
            key = path + "\x00" + version
            current = records.get(key)
            if current == None:
                current = {
                    "path": path,
                    "version": version,
                    "replacement_path": "",
                    "replacement_version": "",
                    "explicit": False,
                    "packages": [],
                }
                records[key] = current
                order.append(key)
            if replacement != None:
                current["replacement_path"] = replacement["replacement_path"]
                current["replacement_version"] = replacement["replacement_version"]
        elif line.startswith("## "):
            if current != None and "explicit" in line:
                current["explicit"] = True
        elif not line.startswith("#") and current != None:
            current["packages"].append(line)
    return [records[key] for key in order if records[key]["packages"]]

def _go_resolver_file(ctx, path, description):
    key = path[2:] if path.startswith("./") else path
    value = (ctx.get("files") or {}).get(key)
    if value == None:
        fail(ctx["label"]["id"] + ": " + description + " `" + path + "` must be included in resolver_inputs, or srcs when resolver_inputs is omitted")
    return value

def _go_safe_name(value):
    out = []
    for ch in value.elems():
        code = ord(ch)
        if (code >= ord("a") and code <= ord("z")) or (code >= ord("A") and code <= ord("Z")) or (code >= ord("0") and code <= ord("9")) or ch == "-" or ch == ".":
            out.append(ch)
        elif ch == "_":
            out.append("_u")
        else:
            out.append("_x" + str(code) + "_")
    return "".join(out)

def _go_module_target_name(module):
    version = module["version"] or "local"
    return "go-module-" + _go_safe_name(module["path"] + "@" + version)

def _go_manifest_replacement(manifest, path, version):
    return manifest["replacements"].get(path + "\x00" + version) or manifest["replacements"].get(path + "\x00")

def _go_dependencies_resolver(ctx):
    manifest_path = (ctx.get("attrs") or {}).get("manifest") or "go.mod"
    sum_files = (ctx.get("attrs") or {}).get("sum_files") or ["go.sum"]
    workspace_file = (ctx.get("attrs") or {}).get("workspace_file") or ""
    vendor_dir = _go_trim_slashes((ctx.get("attrs") or {}).get("vendor_dir") or "vendor")
    vendor_manifest = vendor_dir + "/modules.txt"
    manifest = None
    if not workspace_file:
        manifest = _go_parse_manifest(_go_resolver_file(ctx, manifest_path, "Go module manifest"), manifest_path)
    else:
        _go_resolver_file(ctx, workspace_file, "Go workspace file")
    sums = _go_parse_sums([_go_resolver_file(ctx, path, "Go checksum file") for path in sum_files])
    modules = _go_vendor_modules(_go_resolver_file(ctx, vendor_manifest, "Go vendor manifest"))
    targets = []
    names = []
    for module in modules:
        if manifest != None:
            required = manifest["requires"].get(module["path"])
            if required == None:
                fail(ctx["label"]["id"] + ": vendored module `" + module["path"] + "` is absent from the complete Go module requirement graph")
            if required != module["version"]:
                fail(ctx["label"]["id"] + ": vendored module `" + module["path"] + "` version `" + module["version"] + "` does not match go.mod version `" + required + "`")
            replacement = _go_manifest_replacement(manifest, module["path"], module["version"])
            if module["replacement_path"]:
                if replacement == None:
                    fail(ctx["label"]["id"] + ": vendored replacement for `" + module["path"] + "` is absent from go.mod")
                if replacement["replacement_path"] != module["replacement_path"] or replacement["replacement_version"] != module["replacement_version"]:
                    fail(ctx["label"]["id"] + ": vendored replacement for `" + module["path"] + "` does not match go.mod")
            elif replacement != None:
                fail(ctx["label"]["id"] + ": go.mod replacement for `" + module["path"] + "` is absent from vendor/modules.txt")
        checksum_path = module["replacement_path"] or module["path"]
        checksum_version = module["replacement_version"] or module["version"]
        local_replacement = module["replacement_path"].startswith(".") or module["replacement_path"].startswith("/")
        checksum = "" if local_replacement else sums.get(checksum_path + "\x00" + checksum_version)
        if checksum_version and not local_replacement and not checksum:
            fail(ctx["label"]["id"] + ": vendored module `" + checksum_path + "` version `" + checksum_version + "` has no source checksum in the declared Go checksum files")
        name = _go_module_target_name(module)
        names.append(name)
        source_root = vendor_dir + "/" + module["path"]
        targets.append({
            "name": name,
            "kind": "go_module",
            "deps": [],
            "srcs": [source_root + "/**/*"],
            "attrs": {
                "module_path": module["path"],
                "version": module["version"],
                "checksum": checksum or "",
                "replacement_path": module["replacement_path"],
                "replacement_version": module["replacement_version"],
                "source_root": source_root,
                "packages": module["packages"],
                "_go_locked": True,
            },
        })
    return {
        "targets": targets,
        "roots": names,
        "attrs": {"_go_resolved": True, "_go_module_targets": names},
    }

def _go_module_impl(ctx):
    if not _go_attr(ctx, "_go_locked", False):
        fail(ctx["label"]["id"] + ": go_module targets are generated by go_dependencies")
    sources = glob(ctx["srcs"])
    return {
        "go_module": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "go_module",
        "module_path": _go_attr(ctx, "module_path", ""),
        "version": _go_attr(ctx, "version", ""),
        "checksum": _go_attr(ctx, "checksum", ""),
        "replacement_path": _go_attr(ctx, "replacement_path", ""),
        "replacement_version": _go_attr(ctx, "replacement_version", ""),
        "source_root": _package_relative(ctx, _go_attr(ctx, "source_root", "")),
        "packages": _go_attr(ctx, "packages", []),
        "sources": sources,
        "transitive_sources": sources,
        "affected_inputs": sources,
    }

def _go_dependencies_impl(ctx):
    if not _go_attr(ctx, "_go_resolved", False):
        fail(ctx["label"]["id"] + ": go_dependencies must be expanded by its graph resolver")
    module_root = ctx["label"]["package"]
    manifest = _package_relative(ctx, _go_attr(ctx, "manifest", "go.mod"))
    sum_files = [_package_relative(ctx, path) for path in _go_attr(ctx, "sum_files", ["go.sum"])]
    vendor_dir = _package_relative(ctx, _go_attr(ctx, "vendor_dir", "vendor"))
    vendor_manifest = vendor_dir + "/modules.txt"
    workspace_file = _package_relative(ctx, _go_attr(ctx, "workspace_file", ""))
    modules = [dep for dep in ctx["deps"] if dep.get("go_module")]
    sources = _go_collect(modules, "transitive_sources", [manifest, vendor_manifest] + sum_files + ([workspace_file] if workspace_file else []))
    return {
        "go_dependency_set": True,
        "label_id": ctx["label"]["id"],
        "target_kind": "go_dependencies",
        "module_root": module_root,
        "manifest": manifest,
        "sum_files": sum_files,
        "vendor_dir": vendor_dir,
        "vendor_manifest": vendor_manifest,
        "workspace_file": workspace_file,
        "modules": modules,
        "transitive_sources": sources,
        "affected_inputs": sources,
    }

_GO_MODULE_ATTRS = [
    attr("module_root", "string", docs = "Workspace-relative Go module root. Defaults to the package containing once.toml.", configurable = False),
    attr("package", "string", docs = "Go package pattern built relative to module_root. Defaults to the target package.", configurable = False),
    attr("package_root", "string", docs = "Buck-compatible alias for package.", configurable = False),
    attr("go_work", "string", docs = "Workspace-relative go.work file used for the action. Defaults to GOWORK=off.", configurable = False),
    attr("mod_mode", "string", default = "\"vendor\"", docs = "Go module loading mode. vendor is the reproducible default; readonly is supported for pre-populated module caches.", configurable = False),
]

_GO_TOOLCHAIN_ATTRS = [
    attr("go", "string", docs = "Override path to the Go command.", configurable = False),
    attr("goos", "string", default = "\"auto\"", docs = "Destination Go operating system. Defaults to the host.", configurable = False),
    attr("goarch", "string", default = "\"auto\"", docs = "Destination Go architecture. Defaults to the host.", configurable = False),
    attr("cc", "string", docs = "C compiler used when cgo is enabled.", configurable = False),
    attr("cxx", "string", docs = "C++ compiler used when cgo is enabled.", configurable = False),
]

_GO_COMPILE_ATTRS = _GO_MODULE_ATTRS + _GO_TOOLCHAIN_ATTRS + [
    attr("importpath", "string", docs = "Import path exposed by this package.", configurable = False),
    attr("package_name", "string", docs = "Buck-compatible import path exposed by this package.", configurable = False),
    attr("importmap", "string", docs = "Compiler import identity. Defaults to importpath.", configurable = False),
    attr("importpath_aliases", "list<string>", default = "[]", docs = "Alternate import paths accepted while migrating Bazel targets.", configurable = False),
    attr("embedsrcs", "list<string>", default = "[]", docs = "Files consumed by //go:embed directives.", configurable = False),
    attr("embed_srcs", "list<string>", default = "[]", docs = "Buck-compatible alias for embedsrcs.", configurable = False),
    attr("headers", "list<string>", default = "[]", docs = "Buck-compatible cgo header inputs.", configurable = False),
    attr("header_namespace", "string", docs = "Buck-compatible logical namespace retained for native dependency migrations.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Runtime data file globs propagated to binaries and tests.", configurable = False),
    attr("resources", "list<string>", default = "[]", docs = "Buck-compatible runtime resource file globs.", configurable = False),
    attr("gc_goopts", "list<string>", default = "[]", docs = "Options passed to the Go compiler for every package.", configurable = False),
    attr("compiler_flags", "list<string>", default = "[]", docs = "Buck-compatible alias for Go compiler options.", configurable = False),
    attr("assembler_flags", "list<string>", default = "[]", docs = "Options passed to the Go assembler.", configurable = False),
    attr("tags", "list<string>", default = "[]", docs = "Go build constraint tags.", configurable = True),
    attr("gotags", "list<string>", default = "[]", docs = "Bazel-compatible alias for Go build constraint tags.", configurable = True),
    attr("build_tags", "list<string>", default = "[]", docs = "Buck-compatible alias for Go build constraint tags.", configurable = True),
    attr("cgo", "bool", default = "false", docs = "Enable cgo compilation for this target.", configurable = False),
    attr("pure", "string", default = "\"auto\"", docs = "Bazel-compatible cgo policy: on disables cgo, off enables it, and auto follows cgo.", configurable = False),
    attr("copts", "list<string>", default = "[]", docs = "C compiler options used by cgo.", configurable = False),
    attr("cppopts", "list<string>", default = "[]", docs = "C preprocessor options used by cgo.", configurable = False),
    attr("cxxopts", "list<string>", default = "[]", docs = "C++ compiler options used by cgo.", configurable = False),
    attr("clinkopts", "list<string>", default = "[]", docs = "C linker options used by cgo.", configurable = False),
    attr("env", "map<string,string>", default = "{}", docs = "Environment variables passed to Go build actions.", configurable = False),
    attr("trimpath", "bool", default = "true", docs = "Remove host filesystem paths from compiled artifacts.", configurable = False),
    attr("race", "string", default = "\"auto\"", docs = "Race detector mode: auto, on, or off.", configurable = False),
    attr("msan", "string", default = "\"auto\"", docs = "Memory sanitizer mode: auto, on, or off.", configurable = False),
    attr("asan", "string", default = "\"auto\"", docs = "Address sanitizer mode: auto, on, or off.", configurable = False),
    attr("pgoprofile", "string", docs = "Package-relative profile used for profile-guided optimization.", configurable = False),
    attr("coverage", "bool", default = "false", docs = "Instrument the target for coverage.", configurable = False),
    attr("coverage_enabled", "bool", default = "false", docs = "Buck-compatible alias for coverage.", configurable = False),
    attr("coverage_mode", "string", docs = "Coverage mode: set, count, or atomic.", configurable = False),
]

_GO_LINK_ATTRS = [
    attr("out", "string", docs = "Exact output file name.", configurable = False),
    attr("basename", "string", docs = "Output base name before the platform extension.", configurable = False),
    attr("output_name", "string", docs = "Buck-compatible alias for basename.", configurable = False),
    attr("build_mode", "string", docs = "Go build mode: exe, pie, plugin, c-archive, c-shared, shared, or archive.", configurable = False),
    attr("linkmode", "string", default = "\"auto\"", docs = "Bazel-compatible build mode alias.", configurable = False),
    attr("link_style", "string", default = "\"static\"", docs = "Buck-compatible link style: static, static_pic, or shared.", configurable = False),
    attr("link_mode", "string", default = "\"auto\"", docs = "Buck-compatible Go linker mode: auto, internal, or external.", configurable = False),
    attr("gc_linkopts", "list<string>", default = "[]", docs = "Options passed to the Go linker.", configurable = False),
    attr("linker_flags", "list<string>", default = "[]", docs = "Buck-compatible Go linker options.", configurable = False),
    attr("external_linker_flags", "list<string>", default = "[]", docs = "Options passed through the Go linker to the external linker.", configurable = False),
    attr("x_defs", "map<string,string>", default = "{}", docs = "Link-time string variable definitions lowered to -X options.", configurable = False),
    attr("strip", "bool", default = "false", docs = "Strip symbol and debugging tables from the final binary.", configurable = False),
    attr("static", "string", default = "\"auto\"", docs = "Static linking policy: auto, on, or off.", configurable = False),
    attr("android_abi", "string", docs = "Android Application Binary Interface directory exposed by c-shared outputs.", configurable = False),
    attr("generate_exported_header", "bool", default = "false", docs = "Buck-compatible request for the header emitted by C archive and C shared modes.", configurable = False),
]

_GO_DEP_EDGES = [
    dep("deps", ["go_package", "go_dependency_set", "c_provider"], "Go packages, locked module sets, and native providers consumed by the target."),
    dep("embed", ["go_package"], "Go source or library providers compiled into the same package."),
    dep("cdeps", ["c_provider"], "Native dependencies consumed through cgo."),
]

_GO_SOURCE_REFERENCES = [
    source_reference(
        "Bazel rules_go",
        "go_source",
        "https://raw.githubusercontent.com/bazel-contrib/rules_go/c6b35c2367d164f27af4825422d5c70c3365f6fb/go/private/rules/source.bzl",
        "Preserve reusable source, data, dependency, and embedding roles.",
        content_digest = "74cceee5ed74660b14d3ef08bb35010bf5e6ca817fe1d0c11e50affe69dfa965",
    ),
]

_GO_LIBRARY_REFERENCES = [
    source_reference(
        "Bazel rules_go",
        "go_library",
        "https://raw.githubusercontent.com/bazel-contrib/rules_go/c6b35c2367d164f27af4825422d5c70c3365f6fb/go/private/rules/library.bzl",
        "Preserve package identity, source embedding, cgo, archive, and coverage behavior.",
        content_digest = "5ce524ff9cdea71a0a463b92295c4f124e629535406295d9e9a92f6aa1e47a46",
    ),
    source_reference(
        "Buck2",
        "go_library",
        "https://raw.githubusercontent.com/facebook/buck2/afaea83d9dfcd27d28fa5101b0e885a066a10737/prelude/go/go_library.bzl",
        "Preserve package archive, compiler flag, coverage, cgo, and native propagation roles.",
        content_digest = "c00863d71d94c5a9bbc8dcf0c499d3679a17ec05a8019a870f661e6c9ecddfe4",
    ),
]

_GO_BINARY_REFERENCES = [
    source_reference(
        "Bazel rules_go",
        "go_binary",
        "https://raw.githubusercontent.com/bazel-contrib/rules_go/c6b35c2367d164f27af4825422d5c70c3365f6fb/go/private/rules/binary.bzl",
        "Preserve executable, cross-compilation, build-mode, cgo export, and run behavior.",
        content_digest = "6159d3417e29a525ac90163ee53610a05dd1485ffd29d23b5a353aadeffd4ead",
    ),
    source_reference(
        "Bazel rules_go",
        "go_cross_binary",
        "https://raw.githubusercontent.com/bazel-contrib/rules_go/c6b35c2367d164f27af4825422d5c70c3365f6fb/go/private/rules/cross.bzl",
        "Express cross-compilation on the typed Go operating-system and architecture attributes instead of a wrapper target.",
        content_digest = "ae116d7cba68433ad3f5a9693dc1f821d3f9e2d38924ca4d416e50fbe1545d31",
    ),
    source_reference(
        "Buck2",
        "go_binary",
        "https://raw.githubusercontent.com/facebook/buck2/afaea83d9dfcd27d28fa5101b0e885a066a10737/prelude/go/go_binary.bzl",
        "Preserve compile, link, runtime resource, and distribution output roles.",
        content_digest = "3c2c2c269e45afbb4cc86f953e23ac891e21f7e3c12248acf01cd04b7875ffe2",
    ),
    source_reference(
        "Buck2",
        "go_exported_library",
        "https://raw.githubusercontent.com/facebook/buck2/afaea83d9dfcd27d28fa5101b0e885a066a10737/prelude/go/go_exported_library.bzl",
        "Expose C archive and C shared build modes through generic native providers.",
        content_digest = "f8ae2f0ff3229c99ce9cea1d075eb4977fbcd18b057ea0927a7e5662036641c7",
    ),
]

_GO_TEST_REFERENCES = [
    source_reference(
        "Bazel rules_go",
        "go_test",
        "https://raw.githubusercontent.com/bazel-contrib/rules_go/c6b35c2367d164f27af4825422d5c70c3365f6fb/go/private/rules/test.bzl",
        "Preserve internal and external test execution, filtering, coverage, and test environment behavior.",
        content_digest = "69f8e2b3c6b70b12b8012feb75cf7f0679c075c73e0623ba3d44a2b349d66c00",
    ),
    source_reference(
        "Buck2",
        "go_test",
        "https://raw.githubusercontent.com/facebook/buck2/afaea83d9dfcd27d28fa5101b0e885a066a10737/prelude/go/go_test.bzl",
        "Preserve target-under-test composition, exact runner behavior, coverage, and resource roles.",
        content_digest = "442667c0055a396eaaac9953022dee796e591fa1e2e315f41c7fb312128e461a",
    ),
]

_GO_DEPENDENCY_REFERENCES = [
    source_reference(
        "Bazel Gazelle",
        "go_deps.from_file",
        "https://raw.githubusercontent.com/bazelbuild/bazel-gazelle/a1b7ff21027be138c00498d328fcd2af698e259a/internal/bzlmod/go_mod.bzl",
        "Import the complete Go 1.17 or newer module graph and bind source checksums from go.sum.",
        content_digest = "dc29c4b2ab5e158b5926213cac5e4297ec1cb735f9b1f3dc524696ff48610995",
    ),
    source_reference(
        "Bazel Gazelle",
        "go_deps",
        "https://raw.githubusercontent.com/bazelbuild/bazel-gazelle/a1b7ff21027be138c00498d328fcd2af698e259a/internal/bzlmod/go_deps.bzl",
        "Lower selected Go modules into independently queryable external dependency nodes.",
        content_digest = "79e495feb25ad4b893780bcf8ae54142148c6b15034e34366b37881561a1c874",
    ),
]

_GO_EXAMPLE = [
    example(
        "go-comprehensive",
        name = "Go library, binary, tests, and locked module",
        use_when = "Use this when a Go module needs first-party packages, a runnable command, normalized tests, and an offline vendored dependency.",
    ),
]

go_dependencies = target_kind(
    docs = "Locked Go module graph read from go.mod or go.work checksum files and materialized from vendor sources.",
    attrs = [
        attr("manifest", "string", default = "\"go.mod\"", docs = "Package-relative Go module manifest.", configurable = False),
        attr("workspace_file", "string", docs = "Package-relative go.work file for a multi-module workspace.", configurable = False),
        attr("sum_files", "list<string>", default = "[\"go.sum\"]", docs = "Package-relative checksum files that authenticate vendored module source versions.", configurable = False),
        attr("vendor_dir", "string", default = "\"vendor\"", docs = "Package-relative directory created by go mod vendor or go work vendor.", configurable = False),
        attr("resolver_inputs", "list<string>", default = "[]", docs = "Manifest, checksum, workspace, and vendor/modules.txt files supplied to the resolver.", configurable = False),
        attr("_go_resolved", "bool", default = "false", docs = "Resolver-owned marker for an expanded module graph.", configurable = False),
        attr("_go_module_targets", "list<string>", default = "[]", docs = "Resolver-owned generated module target names.", configurable = False),
    ],
    deps = [dep("deps", ["go_module"], "Locked vendored modules emitted by the resolver.")],
    providers = ["go_dependency_set"],
    capabilities = [capability("build", [])],
    examples = _GO_EXAMPLE,
    source_references = _GO_DEPENDENCY_REFERENCES,
    resolver = _go_dependencies_resolver,
    impl = _go_dependencies_impl,
)

go_module = target_kind(
    docs = "One checksum-bound vendored Go module generated from go_dependencies.",
    attrs = [
        attr("module_path", "string", required = True, docs = "Canonical Go module path.", configurable = False),
        attr("version", "string", docs = "Selected Go module version.", configurable = False),
        attr("checksum", "string", docs = "go.sum source checksum for the selected version.", configurable = False),
        attr("replacement_path", "string", docs = "Replacement module or local path from the vendor manifest.", configurable = False),
        attr("replacement_version", "string", docs = "Replacement module version from the vendor manifest.", configurable = False),
        attr("source_root", "string", required = True, docs = "Package-relative vendored module source root.", configurable = False),
        attr("packages", "list<string>", default = "[]", docs = "Vendored import paths owned by this module.", configurable = False),
        attr("_go_locked", "bool", default = "false", docs = "Resolver-owned marker proving locked generation.", configurable = False),
    ],
    providers = ["go_module"],
    capabilities = [capability("build", [])],
    examples = _GO_EXAMPLE,
    source_references = _GO_DEPENDENCY_REFERENCES,
    impl = _go_module_impl,
)

go_source = target_kind(
    docs = "Reusable Go sources, embedded files, data, and dependencies compiled by another Go target.",
    attrs = _GO_COMPILE_ATTRS,
    deps = _GO_DEP_EDGES,
    providers = ["go_package"],
    capabilities = [capability("build", [])],
    examples = _GO_EXAMPLE,
    source_references = _GO_SOURCE_REFERENCES,
    tools = [_GO_TOOL],
    impl = _go_source_impl,
)

go_library = target_kind(
    docs = "Go package compiled into an archive with build constraints, embedding, cgo, coverage, sanitizer, and cross-compilation controls.",
    attrs = _GO_COMPILE_ATTRS + [
        attr("x_defs", "map<string,string>", default = "{}", docs = "Bazel-compatible link-time definitions retained for embedded binary migrations.", configurable = False),
    ],
    deps = _GO_DEP_EDGES,
    providers = ["go_package"],
    capabilities = [capability("build", ["library"])],
    examples = _GO_EXAMPLE,
    source_references = _GO_LIBRARY_REFERENCES,
    tools = [_GO_TOOL],
    impl = _go_library_impl,
)

go_binary = target_kind(
    docs = "Go executable, position-independent executable, plugin, C archive, or C shared library with build and run capabilities.",
    attrs = _GO_COMPILE_ATTRS + _GO_LINK_ATTRS + [
        attr("args", "list<string>", default = "[]", docs = "Arguments passed by once run.", configurable = False),
        attr("run_env", "map<string,string>", default = "{}", docs = "Environment variables passed by once run.", configurable = False),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited by once run.", configurable = False),
    ],
    deps = _GO_DEP_EDGES,
    providers = ["go_package", "go_binary", "c_provider", "native_linkable", "apple_linkable", "android_native_library"],
    capabilities = [capability("build", ["binary"]), capability("run", ["default"], ["binary"])],
    examples = _GO_EXAMPLE,
    source_references = _GO_BINARY_REFERENCES,
    tools = [_GO_TOOL],
    impl = _go_binary_impl,
)

go_test = target_kind(
    docs = "Go test package compiled once and run with exact filters, normalized results, native event records, and optional coverage.",
    attrs = _GO_COMPILE_ATTRS + _GO_LINK_ATTRS + [
        attr("args", "list<string>", default = "[]", docs = "Arguments passed to the generated Go test binary.", configurable = False),
        attr("test_env", "map<string,string>", default = "{}", docs = "Environment variables passed to the test runner.", configurable = False),
        attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited by the test runner.", configurable = False),
        attr("rundir", "string", docs = "Package-relative working directory for the test binary.", configurable = False),
        attr("labels", "list<string>", default = "[]", docs = "Labels exposed through generic test discovery.", configurable = True),
        attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
        attr("cover_packages", "list<string>", default = "[]", docs = "Import path patterns included in coverage instrumentation.", configurable = False),
        attr("fail_fast", "bool", default = "false", docs = "Stop after the first failing test.", configurable = False),
        attr("short", "bool", default = "false", docs = "Request short test mode.", configurable = False),
        attr("count", "int", default = "0", docs = "Run each selected test this many times when greater than zero.", configurable = False),
        attr("parallel", "int", default = "0", docs = "Maximum parallel tests inside the generated test binary when greater than zero.", configurable = False),
        attr("shuffle", "string", docs = "Go test shuffle setting, such as on, off, or a seed.", configurable = False),
    ],
    deps = _GO_DEP_EDGES + [dep("target_under_test", ["go_package"], "Buck-compatible library or binary sources composed into this test package.")],
    providers = ["go_package", "once_test_info"],
    capabilities = [capability("build", ["binary"]), capability("test", ["default", "test_results", "logs", "coverage"])],
    examples = _GO_EXAMPLE,
    source_references = _GO_TEST_REFERENCES,
    tools = [_GO_TOOL],
    impl = _go_test_impl,
)
