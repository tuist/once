_RUFF_TOOL = tool("ruff", executables = ["ruff"])
_ESLINT_TOOL = tool("eslint", executables = ["eslint"])
_GOLANGCI_LINT_TOOL = tool("golangci-lint", executables = ["golangci-lint"])
_SWIFTLINT_TOOL = tool("swiftlint", executables = ["swiftlint"])
_DETEKT_TOOL = tool("detekt", executables = ["detekt"])
_CREDO_TOOL = tool("mix", executables = ["mix"])
_RUBOCOP_TOOL = tool("rubocop", executables = ["rubocop"])
_LINT_NODE_TOOL = tool("node", executables = ["node"])

def _lint_attr(ctx, name, default):
    return _configured_attr(ctx, name, default)

def _lint_sources(ctx):
    return _unique(glob(ctx["srcs"]) + _file_globs(_lint_attr(ctx, "config", [])) + _file_globs(_lint_attr(ctx, "data", [])))

def _lint_executable(ctx, name, default, workspace_candidates = []):
    requested = _lint_attr(ctx, name, "")
    if requested:
        resolved = _resolve_host_executable(requested)
        if resolved:
            return resolved
        fail(ctx["label"]["id"] + ": lint executable `" + requested + "` was not found")
    for candidate in workspace_candidates:
        resolved = _resolve_host_executable(_package_relative(ctx, candidate))
        if resolved:
            return resolved
    resolved = _resolve_host_executable(default)
    if resolved:
        return resolved
    fail(ctx["label"]["id"] + ": lint executable `" + default + "` was not found")

def _lint_paths(ctx):
    directory = ctx["build_dir"] + "/lint"
    return {
        "directory": directory,
        "sarif": directory + "/report.sarif",
        "native": directory + "/native-results.json",
        "results": directory + "/lint_results.json",
    }

def _lint_provider(ctx, analyzer, paths, inputs, command, native_results = [], target_kind = None):
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": target_kind or (analyzer + "_lint"),
        "affected_inputs": inputs,
        "lint_info": {
            "schema": "once.lint_info.v1",
            "target": ctx["label"]["id"],
            "analyzer": {
                "type": analyzer,
                "display_name": analyzer,
                "metadata": {},
            },
            "command": command,
            "outputs": {
                "sarif": paths["sarif"],
                "results": paths["results"],
                "native_results": native_results,
                "logs": [],
            },
            "scope": {
                "requested": glob(ctx["srcs"]),
            },
            "execution": {
                "cacheable": True,
                "run_from_workspace_root": True,
            },
            "metadata": {},
        },
    }

def _lint_identity(name, executable, version):
    return "once.lint.v1\x00" + name + "\x00" + executable + "\x00" + version

def _ruff_lint_impl(ctx):
    executable = _lint_executable(ctx, "ruff", "ruff")
    paths = _lint_paths(ctx)
    inputs = _lint_sources(ctx)
    sources = glob(ctx["srcs"])
    argv = [executable, "check", "--output-format", "sarif", "--output-file", paths["sarif"]] + _lint_attr(ctx, "args", []) + sources
    provider = _lint_provider(ctx, "ruff", paths, inputs, {"argv": argv, "env": {}, "cwd": "."})
    if ctx["capability"] == "lint":
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [paths["sarif"]],
            clean_paths = [paths["sarif"]],
            create_dirs = [paths["directory"]],
            success_exit_codes = [0, 1],
            toolchain_identity = _lint_identity("ruff", executable, host_command([executable, "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":ruff",
        )
    return provider

def _golangci_lint_impl(ctx):
    executable = _lint_executable(ctx, "golangci_lint", "golangci-lint")
    paths = _lint_paths(ctx)
    inputs = _lint_sources(ctx)
    packages = _lint_attr(ctx, "packages", ["./..."])
    argv = [executable, "run", "--output.sarif.path=" + execution_path(paths["sarif"]), "--issues-exit-code=1"] + _lint_attr(ctx, "args", []) + packages
    cwd = _lint_attr(ctx, "cwd", ctx["label"]["package"] or ".")
    provider = _lint_provider(ctx, "golangci-lint", paths, inputs, {"argv": argv, "env": {}, "cwd": cwd}, target_kind = "golangci_lint")
    if ctx["capability"] == "lint":
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [paths["sarif"]],
            clean_paths = [paths["sarif"]],
            create_dirs = [paths["directory"]],
            cwd = cwd,
            success_exit_codes = [0, 1],
            toolchain_identity = _lint_identity("golangci-lint", executable, host_command([executable, "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":golangci-lint",
        )
    return provider

def _swiftlint_impl(ctx):
    executable = _lint_executable(ctx, "swiftlint", "swiftlint")
    paths = _lint_paths(ctx)
    inputs = _lint_sources(ctx)
    argv = [executable, "lint", "--reporter", "sarif"] + _lint_attr(ctx, "args", []) + glob(ctx["srcs"])
    provider = _lint_provider(ctx, "swiftlint", paths, inputs, {"argv": argv, "env": {}, "cwd": "."})
    if ctx["capability"] == "lint":
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [paths["sarif"]],
            clean_paths = [paths["sarif"]],
            create_dirs = [paths["directory"]],
            stdout = paths["sarif"],
            success_exit_codes = [0, 2],
            toolchain_identity = _lint_identity("swiftlint", executable, host_command([executable, "version"]).strip()),
            identifier = ctx["label"]["id"] + ":swiftlint",
        )
    return provider

def _detekt_impl(ctx):
    executable = _lint_executable(ctx, "detekt", "detekt")
    paths = _lint_paths(ctx)
    inputs = _lint_sources(ctx)
    sources = glob(ctx["srcs"])
    argv = [executable, "--input", ",".join(sources), "--report", "sarif:" + paths["sarif"]] + _lint_attr(ctx, "args", [])
    provider = _lint_provider(ctx, "detekt", paths, inputs, {"argv": argv, "env": {}, "cwd": "."})
    if ctx["capability"] == "lint":
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [paths["sarif"]],
            clean_paths = [paths["sarif"]],
            create_dirs = [paths["directory"]],
            success_exit_codes = [0, 2],
            toolchain_identity = _lint_identity("detekt", executable, host_command([executable, "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":detekt",
        )
    return provider

def _credo_impl(ctx):
    executable = _lint_executable(ctx, "mix", "mix")
    paths = _lint_paths(ctx)
    inputs = _lint_sources(ctx)
    argv = [executable, "credo", "--format", "sarif", "--mute-exit-status"] + _lint_attr(ctx, "args", [])
    cwd = _lint_attr(ctx, "cwd", ctx["label"]["package"] or ".")
    provider = _lint_provider(ctx, "credo", paths, inputs, {"argv": argv, "env": {}, "cwd": cwd})
    if ctx["capability"] == "lint":
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [paths["sarif"]],
            clean_paths = [paths["sarif"]],
            create_dirs = [paths["directory"]],
            cwd = cwd,
            stdout = paths["sarif"],
            toolchain_identity = _lint_identity("credo", executable, host_command([executable, "credo", "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":credo",
        )
    return provider

def _eslint_adapter():
    return '''import fs from "node:fs"

const [input, output] = process.argv.slice(2)
const files = JSON.parse(fs.readFileSync(input, "utf8"))
const results = []
for (const file of files) {
  for (const message of file.messages || []) {
    results.push({
      ruleId: message.ruleId || null,
      level: message.severity === 2 ? "error" : "warning",
      message: {text: message.message},
      locations: [{physicalLocation: {
        artifactLocation: {uri: file.filePath},
        region: {
          startLine: message.line,
          startColumn: message.column,
          endLine: message.endLine || message.line,
          endColumn: message.endColumn || message.column,
        },
      }}],
    })
  }
}
fs.writeFileSync(output, JSON.stringify({
  version: "2.1.0",
  runs: [{tool: {driver: {name: "ESLint", rules: []}}, results}],
}, null, 2))
'''

def _eslint_impl(ctx):
    executable = _lint_executable(ctx, "eslint", "eslint", ["node_modules/.bin/eslint"])
    node = _lint_executable(ctx, "node", "node")
    paths = _lint_paths(ctx)
    inputs = _lint_sources(ctx)
    adapter = paths["directory"] + "/eslint-to-sarif.mjs"
    sources = glob(ctx["srcs"])
    argv = [executable, "--format", "json", "--output-file", paths["native"]] + _lint_attr(ctx, "args", []) + sources
    provider = _lint_provider(ctx, "eslint", paths, inputs, {"argv": argv, "env": {}, "cwd": "."}, [paths["native"]])
    if ctx["capability"] == "lint":
        write_path(adapter, _eslint_adapter())
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [paths["native"]],
            clean_paths = [paths["native"]],
            create_dirs = [paths["directory"]],
            success_exit_codes = [0, 1],
            toolchain_identity = _lint_identity("eslint", executable, host_command([executable, "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":eslint",
        )
        run_action(
            argv = [node, adapter, paths["native"], paths["sarif"]],
            inputs = [adapter, paths["native"]],
            outputs = [paths["sarif"]],
            clean_paths = [paths["sarif"]],
            toolchain_identity = _lint_identity("eslint-adapter", node, host_command([node, "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":eslint-adapter",
        )
    return provider

def _rubocop_adapter():
    return '''require "json"

input, output = ARGV
document = JSON.parse(File.read(input))
results = document.fetch("files", []).flat_map do |file|
  file.fetch("offenses", []).map do |offense|
    location = offense.fetch("location", {})
    severity = offense["severity"]
    level = ["fatal", "error"].include?(severity) ? "error" : severity == "info" ? "note" : "warning"
    {
      "ruleId" => offense["cop_name"],
      "level" => level,
      "message" => {"text" => offense["message"]},
      "locations" => [{"physicalLocation" => {
        "artifactLocation" => {"uri" => file["path"]},
        "region" => {
          "startLine" => location["start_line"],
          "startColumn" => location["start_column"],
          "endLine" => location["last_line"],
          "endColumn" => location["last_column"],
        },
      }}],
    }
  end
end
File.write(output, JSON.pretty_generate({
  "version" => "2.1.0",
  "runs" => [{"tool" => {"driver" => {"name" => "RuboCop", "rules" => []}}, "results" => results}],
}))
'''

def _rubocop_impl(ctx):
    executable = _lint_executable(ctx, "rubocop", "rubocop")
    ruby = _lint_executable(ctx, "ruby", "ruby")
    paths = _lint_paths(ctx)
    inputs = _lint_sources(ctx)
    adapter = paths["directory"] + "/rubocop-to-sarif.rb"
    sources = glob(ctx["srcs"])
    argv = [executable, "--format", "json", "--out", paths["native"]] + _lint_attr(ctx, "args", []) + sources
    provider = _lint_provider(ctx, "rubocop", paths, inputs, {"argv": argv, "env": {}, "cwd": "."}, [paths["native"]])
    if ctx["capability"] == "lint":
        write_path(adapter, _rubocop_adapter())
        run_action(
            argv = argv,
            inputs = inputs,
            outputs = [paths["native"]],
            clean_paths = [paths["native"]],
            create_dirs = [paths["directory"]],
            success_exit_codes = [0, 1],
            toolchain_identity = _lint_identity("rubocop", executable, host_command([executable, "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":rubocop",
        )
        run_action(
            argv = [ruby, adapter, paths["native"], paths["sarif"]],
            inputs = [adapter, paths["native"]],
            outputs = [paths["sarif"]],
            clean_paths = [paths["sarif"]],
            toolchain_identity = _lint_identity("rubocop-adapter", ruby, host_command([ruby, "--version"]).strip()),
            identifier = ctx["label"]["id"] + ":rubocop-adapter",
        )
    return provider

def _lint_common_attrs(executable_name, config_default):
    return [
        attr(executable_name, "string", docs = "Executable name, absolute path, or workspace-relative path.", configurable = False),
        attr("config", "list<string>", default = config_default, docs = "Configuration files that affect analysis.", configurable = False),
        attr("data", "list<string>", default = "[]", docs = "Additional files read during analysis.", configurable = False),
        attr("args", "list<string>", default = "[]", docs = "Additional native linter arguments.", configurable = False),
    ]

ruff_lint = target_kind(
    docs = "Runs Ruff and exposes normalized Python lint findings.",
    attrs = _lint_common_attrs("ruff", "[\"pyproject.toml\", \"ruff.toml\", \".ruff.toml\"]"),
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [_RUFF_TOOL],
    source_references = [source_reference("Ruff", "output-format", "https://docs.astral.sh/ruff/settings/#output-format", "Confirm native report and command-line behavior.")],
    examples = [example("ruff-lint-minimal", name = "Minimal Ruff lint target", use_when = "You want cacheable Python lint findings.")],
    impl = _ruff_lint_impl,
)

golangci_lint = target_kind(
    docs = "Runs golangci-lint and exposes normalized Go lint findings.",
    attrs = _lint_common_attrs("golangci_lint", "[\".golangci.yml\", \".golangci.yaml\", \".golangci.toml\", \".golangci.json\", \"go.mod\", \"go.sum\"]") + [
        attr("packages", "list<string>", default = "[\"./...\"]", docs = "Go package patterns passed to golangci-lint.", configurable = False),
        attr("cwd", "string", default = "\".\"", docs = "Workspace-relative Go module directory.", configurable = False),
    ],
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [_GOLANGCI_LINT_TOOL],
    source_references = [source_reference("golangci-lint", "output-configuration", "https://golangci-lint.run/docs/configuration/file/#output-configuration", "Confirm portable report output and issue exit-code behavior.")],
    examples = [example("golangci-lint-minimal", name = "Minimal Go lint target", use_when = "You want cacheable Go lint findings.")],
    impl = _golangci_lint_impl,
)

swiftlint_lint = target_kind(
    docs = "Runs SwiftLint and exposes normalized Swift lint findings.",
    attrs = _lint_common_attrs("swiftlint", "[\".swiftlint.yml\", \".swiftlint.yaml\"]"),
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [_SWIFTLINT_TOOL],
    source_references = [source_reference("SwiftLint", "command-line-usage", "https://github.com/realm/SwiftLint#command-line-usage", "Confirm reporter and command-line behavior.")],
    examples = [example("swiftlint-lint-minimal", name = "Minimal Swift lint target", use_when = "You want cacheable Swift lint findings.")],
    impl = _swiftlint_impl,
)

detekt_lint = target_kind(
    docs = "Runs detekt and exposes normalized Kotlin lint findings.",
    attrs = _lint_common_attrs("detekt", "[\"detekt.yml\", \"detekt.yaml\"]"),
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [_DETEKT_TOOL],
    source_references = [source_reference("detekt", "reporting", "https://detekt.dev/docs/introduction/reporting/", "Confirm portable report output and findings exit-code behavior.")],
    examples = [example("detekt-lint-minimal", name = "Minimal Kotlin lint target", use_when = "You want cacheable Kotlin lint findings.")],
    impl = _detekt_impl,
)

credo_lint = target_kind(
    docs = "Runs Credo and exposes normalized Elixir lint findings.",
    attrs = _lint_common_attrs("mix", "[\".credo.exs\", \"mix.exs\", \"mix.lock\"]") + [
        attr("cwd", "string", default = "\".\"", docs = "Workspace-relative Mix project directory.", configurable = False),
    ],
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [_CREDO_TOOL],
    source_references = [source_reference("Credo", "command-line-switches", "https://hexdocs.pm/credo/cli_switches.html", "Confirm reporter and muted exit-status behavior.")],
    examples = [example("credo-lint-minimal", name = "Minimal Credo lint target", use_when = "You want cacheable Elixir lint findings.")],
    impl = _credo_impl,
)

eslint_lint = target_kind(
    docs = "Runs ESLint and exposes normalized JavaScript and TypeScript lint findings.",
    attrs = _lint_common_attrs("eslint", "[\"eslint.config.js\", \"eslint.config.mjs\", \"eslint.config.cjs\", \"package.json\", \"package-lock.json\", \"pnpm-lock.yaml\", \"yarn.lock\"]") + [
        attr("node", "string", docs = "Node.js executable name, absolute path, or workspace-relative path.", configurable = False),
    ],
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [_ESLINT_TOOL, _LINT_NODE_TOOL],
    source_references = [source_reference("ESLint", "formatters", "https://eslint.org/docs/latest/extend/custom-formatters", "Confirm machine-readable formatter behavior.")],
    examples = [example("eslint-lint-minimal", name = "Minimal ESLint target", use_when = "You want cacheable JavaScript or TypeScript lint findings.")],
    impl = _eslint_impl,
)

rubocop_lint = target_kind(
    docs = "Runs RuboCop and exposes normalized Ruby lint findings.",
    attrs = _lint_common_attrs("rubocop", "[\".rubocop.yml\", \".rubocop_todo.yml\", \"Gemfile\", \"Gemfile.lock\"]") + [
        attr("ruby", "string", docs = "Ruby executable name, absolute path, or workspace-relative path.", configurable = False),
    ],
    providers = ["once_lint_info"],
    capabilities = [capability("lint", ["default", "lint_results"])],
    tools = [_RUBOCOP_TOOL],
    source_references = [source_reference("RuboCop", "formatters", "https://docs.rubocop.org/rubocop/formatters.html", "Confirm machine-readable formatter behavior.")],
    examples = [example("rubocop-lint-minimal", name = "Minimal RuboCop target", use_when = "You want cacheable Ruby lint findings.")],
    impl = _rubocop_impl,
)
