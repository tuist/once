def _bazel_test_adapter():
    return '''#!/bin/sh
set +e
bazel="$1"
results="$2"
native_results="$3"
target="$4"
"$bazel" test --repo_contents_cache= //... > "$native_results" 2>&1
exit_code=$?
if [ "$exit_code" -eq 0 ]; then
  test_status=passed
  passed=1
  failed=0
else
  test_status=failed
  passed=0
  failed=1
fi
printf '{"schema":"once.test_results.v1","target":"%s","runner":{"type":"bazel","metadata":{}},"status":"%s","summary":{"total":1,"passed":%s,"failed":%s,"skipped":0,"flaky":0},"cases":[{"id":"%s","name":"//...","suite":"Bazel","status":"%s","attempts":[{"status":"%s"}],"runner_metadata":{}}],"artifacts":{"logs":["%s"],"native_results":["%s"]}}\n' "$target" "$test_status" "$passed" "$failed" "$target" "$test_status" "$test_status" "$native_results" "$native_results" > "$results"
exit "$exit_code"
'''

def _bazel_workspace_resolver(ctx):
    return {
        "targets": [{
            "name": "bazel_all",
            "kind": "bazel_command",
            "srcs": ["**/*"],
        }],
        "roots": ["bazel_all"],
        "attrs": {"_default_test_roots": ["bazel_all"]},
    }

def _bazel_workspace_impl(ctx):
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "bazel_workspace",
        "targets": ctx["deps"],
    }

def _bazel_command_impl(ctx):
    bazel = host_which_optional("bazelisk") or _resolve_host_executable("bazel")
    version = host_command([bazel, "--version"]).strip()
    inputs = glob(ctx["srcs"])
    home = host_env("HOME")
    path = host_env("PATH")
    env = {}
    if home:
        env["HOME"] = home
    if path:
        env["PATH"] = path
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "bazel_command",
        "affected_inputs": inputs,
    }
    if ctx["capability"] == "build":
        run_action(
            argv = [bazel, "build", "--repo_contents_cache=", "//..."],
            inputs = inputs,
            outputs = [],
            env = env,
            cacheable = False,
            toolchain_identity = "once.bazel.v1\x00" + version,
            identifier = ctx["label"]["id"] + ":build",
        )
        return provider
    test_dir = _test_output_dir(ctx)
    results = test_dir + "/test_results.json"
    native_results = test_dir + "/bazel.log"
    adapter = test_dir + "/once_bazel_test_adapter.sh"
    shell = _resolve_host_executable("sh")
    test_argv = [shell, adapter, bazel, results, native_results, ctx["label"]["id"]]
    provider["test_info"] = {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {"type": "bazel", "display_name": "Bazel", "metadata": {}},
        "command": {"argv": test_argv, "env": env, "cwd": "."},
        "outputs": {"results": results, "logs": [native_results], "native_results": [native_results], "coverage": []},
        "listing": {"supported": False, "strategy": "none"},
        "filtering": {"case_filtering": "unsupported"},
        "sharding": {"supported": False},
        "retries": {"supported": False, "default_attempts": 1},
        "execution": {"cacheable": False, "run_from_workspace_root": True},
        "labels": [],
        "metadata": {},
    }
    if ctx["capability"] != "test":
        return provider
    write_path(adapter, _bazel_test_adapter())
    run_action(
        argv = test_argv,
        inputs = inputs + [adapter],
        outputs = [test_dir, results, native_results],
        clean_paths = [results, native_results],
        create_dirs = [test_dir],
        env = env,
        cacheable = False,
        toolchain_identity = "once.bazel.test.v1\x00" + version,
        identifier = ctx["label"]["id"] + ":test",
    )
    return provider

bazel_command = target_kind(
    docs = "Opaque Bazel workspace command target. Bazel retains ownership of native graph evaluation and caching while Once schedules and reports the complete workspace build or test.",
    providers = ["bazel_workspace_command", "once_test_info"],
    capabilities = [capability("build", []), capability("test", ["test_results", "test_logs"])],
    tools = [tool("bazel", ["bazelisk", "bazel"]), tool("shell", ["sh"])],
    examples = [example("bazel-command-minimal", name = "Bazel workspace command", use_when = "Use this when declaring an explicit opaque Bazel workspace command target.")],
    impl = _bazel_command_impl,
)

bazel_workspace = target_kind(
    docs = "Native Bazel workspace seed. Its resolver exposes the complete native workspace through ordinary Once build and test capabilities.",
    attrs = [attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative Bazel module, workspace, and build definition files supplied to native integration resolution.", configurable = False), attr("_default_test_roots", "list<string>", default = "[]", docs = "Resolver-owned first-party test target names used by targetless test selection.", configurable = False)],
    resolver = _bazel_workspace_resolver,
    deps = [dep("deps", ["bazel_workspace_command"], "Complete native Bazel workspace command target.")],
    providers = ["bazel_workspace"],
    capabilities = [capability("build", [])],
    tools = [tool("bazel", ["bazelisk", "bazel"])],
    examples = [example("bazel-workspace-native-project", name = "Bazel native integration seed", use_when = "Use this when a Bazel workspace should build and test without a Once manifest.")],
    impl = _bazel_workspace_impl,
)

bazel = native_project(
    target_kind = "bazel_workspace",
    name = "bazel_module",
    target_name = "bazel",
    docs = "Recognizes a native Bazel workspace from MODULE.bazel, WORKSPACE.bazel, or WORKSPACE.",
    markers = ["MODULE.bazel"],
    inputs = ["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel", ".bazelrc", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs"],
    input_exclude = [".git", "bazel-bin", "bazel-out", "bazel-testlogs"],
    on_match = "stop",
    requires_tools = [],
    owns_descendants = True,
)

bazel_workspace_file = native_project(
    target_kind = "bazel_workspace",
    name = "bazel_workspace_file",
    target_name = "bazel",
    docs = "Recognizes a native Bazel workspace that predates MODULE.bazel.",
    markers = ["WORKSPACE"],
    inputs = ["WORKSPACE.bazel", ".bazelrc", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs"],
    input_exclude = [".git", "bazel-bin", "bazel-out", "bazel-testlogs"],
    on_match = "stop",
    requires_tools = [],
    owns_descendants = True,
)

bazel_workspace_bazel_file = native_project(
    target_kind = "bazel_workspace",
    name = "bazel_workspace_bazel_file",
    target_name = "bazel",
    docs = "Recognizes a native Bazel workspace from WORKSPACE.bazel.",
    markers = ["WORKSPACE.bazel"],
    inputs = ["WORKSPACE", ".bazelrc", "**/BUILD", "**/BUILD.bazel", "**/*.bzl"],
    exclude = _native_project_generated_dirs() + ["bazel-bin", "bazel-out", "bazel-testlogs"],
    input_exclude = [".git", "bazel-bin", "bazel-out", "bazel-testlogs"],
    on_match = "stop",
    requires_tools = [],
    owns_descendants = True,
)
