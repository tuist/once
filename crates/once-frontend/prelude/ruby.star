_RUBY_TOOL = tool("ruby", executables = ["ruby"])

def _ruby_attr(ctx, name, default):
    value = ctx["attr"].get(name)
    return default if value == None else value

def _ruby_executable(ctx):
    requested = _ruby_attr(ctx, "ruby", "ruby")
    resolved = _resolve_host_executable(requested)
    if not resolved:
        fail(ctx["label"]["id"] + ": Ruby interpreter `" + requested + "` was not found")
    return resolved

def _ruby_env(ctx, test_dir):
    env = {"HOME": test_dir + "/home"}
    for name in _ruby_attr(ctx, "env_inherit", []):
        value = host_env(name)
        if value:
            env[name] = value
    for name, value in _ruby_attr(ctx, "env", {}).items():
        env[name] = value
    return env

def _ruby_test_inputs(ctx):
    return _unique(glob(ctx["srcs"]) + _file_globs(_ruby_attr(ctx, "config", [])) + _file_globs(_ruby_attr(ctx, "data", [])))

def _ruby_rspec_adapter():
    return '''require "fileutils"
require "json"
require "optparse"

options = { sources: [], units: [] }
parser = OptionParser.new do |opts|
  opts.on("--once-results PATH") { |value| options[:results] = value }
  opts.on("--once-native-results PATH") { |value| options[:native_results] = value }
  opts.on("--once-target TARGET") { |value| options[:target] = value }
  opts.on("--once-source PATH") { |value| options[:sources] << value }
  opts.on("--once-test-unit UNIT") { |value| options[:units] << value }
end
parser.order!(ARGV)
ARGV.shift if ARGV.first == "--"
runner_args = ARGV

begin
  require "rspec/core"
rescue LoadError => error
  warn "unable to load Ruby Specification: #{error.message}"
  exit 2
end

prefix = options.fetch(:target) + "::"
selected = options[:units].map do |unit|
  unless unit.start_with?(prefix)
    warn "test unit does not belong to target: #{unit}"
    exit 2
  end
  unit.delete_prefix(prefix)
end
selection = selected.empty? ? options[:sources] : selected
native_results = options.fetch(:native_results)
args = runner_args + selection + ["--format", "json", "--out", native_results]
exit_code = RSpec::Core::Runner.run(args, $stderr, $stdout)
native = File.exist?(native_results) ? JSON.parse(File.read(native_results)) : {"examples" => []}
cases = native.fetch("examples", []).map do |example|
  id = example["id"] || "#{example["file_path"]}:#{example["line_number"]}"
  native_status = example["status"] || "failed"
  status = native_status == "pending" ? "skipped" : native_status
  file = example["file_path"].to_s.sub(%r{\A\./}, "")
  {
    "id" => options[:target] + "::" + id,
    "name" => example["full_description"] || example["description"] || id,
    "suite" => file,
    "file" => file,
    "status" => status,
    "attempts" => [{"status" => status}],
    "runner_metadata" => {
      "example_id" => id,
      "line" => example["line_number"],
      "duration_ms" => ((example["run_time"] || 0) * 1000).round,
    },
  }
end
passed = cases.count { |item| item["status"] == "passed" }
failed = cases.count { |item| item["status"] == "failed" }
skipped = cases.count { |item| item["status"] == "skipped" }
normalized = {
  "schema" => "once.test_results.v1",
  "target" => options[:target],
  "runner" => {"type" => "rspec", "metadata" => {}},
  "status" => exit_code == 0 ? "passed" : "failed",
  "summary" => {"total" => cases.length, "passed" => passed, "failed" => failed, "skipped" => skipped, "flaky" => 0},
  "cases" => cases,
  "artifacts" => {
    "logs" => [File.join(File.dirname(options[:results]), "rspec.log")],
    "native_results" => [native_results],
  },
}
FileUtils.mkdir_p(File.dirname(options[:results])) unless Dir.exist?(File.dirname(options[:results]))
File.write(options[:results], JSON.pretty_generate(normalized))
exit exit_code
'''

def _ruby_minitest_adapter():
    return '''require "json"
require "open3"
require "optparse"
require "rbconfig"

options = { sources: [], units: [] }
parser = OptionParser.new do |opts|
  opts.on("--once-results PATH") { |value| options[:results] = value }
  opts.on("--once-native-results PATH") { |value| options[:native_results] = value }
  opts.on("--once-target TARGET") { |value| options[:target] = value }
  opts.on("--once-source PATH") { |value| options[:sources] << value }
  opts.on("--once-test-unit UNIT") { |value| options[:units] << value }
end
parser.order!(ARGV)
ARGV.shift if ARGV.first == "--"
runner_args = ARGV
prefix = options.fetch(:target) + "::"
selected = options[:units].map do |unit|
  unless unit.start_with?(prefix)
    warn "test unit does not belong to target: #{unit}"
    exit 2
  end
  unit.delete_prefix(prefix)
end
sources = selected.empty? ? options[:sources] : selected
cases = []
native_chunks = []
sources.sort.each do |source|
  stdout, stderr, status = Open3.capture3(RbConfig.ruby, source, *runner_args)
  output = stdout + stderr
  native_chunks << "$ #{RbConfig.ruby} #{source}\n#{output}"
  case_status = status.success? ? "passed" : "failed"
  cases << {
    "id" => options[:target] + "::" + source,
    "name" => source,
    "suite" => source,
    "file" => source,
    "status" => case_status,
    "attempts" => [{"status" => case_status}],
    "runner_metadata" => {},
  }
end
passed = cases.count { |item| item["status"] == "passed" }
failed = cases.length - passed
File.write(options[:native_results], native_chunks.join("\n"))
normalized = {
  "schema" => "once.test_results.v1",
  "target" => options[:target],
  "runner" => {"type" => "minitest", "metadata" => {"unit" => "file"}},
  "status" => failed == 0 ? "passed" : "failed",
  "summary" => {"total" => cases.length, "passed" => passed, "failed" => failed, "skipped" => 0, "flaky" => 0},
  "cases" => cases,
  "artifacts" => {
    "logs" => [File.join(File.dirname(options[:results]), "minitest.log")],
    "native_results" => [options[:native_results]],
  },
}
File.write(options[:results], JSON.pretty_generate(normalized))
exit(failed == 0 ? 0 : 1)
'''

def _ruby_test_info(ctx, runner_type, display_name, ruby, adapter, results, log, native_results, test_dir, file_units):
    return {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {"type": runner_type, "display_name": display_name, "metadata": {}},
        "command": {"argv": [ruby, adapter], "env": _ruby_env(ctx, test_dir), "cwd": "."},
        "outputs": {"results": results, "logs": [log], "native_results": [native_results], "coverage": []},
        "listing": {"supported": True, "strategy": "normalized_results"},
        "filtering": {"case_filtering": "runner_args"},
        "sharding": {"supported": True, "granularity": _ruby_attr(ctx, "batching", "file")},
        "retries": {"supported": False, "default_attempts": 1},
        "execution": {"cacheable": True, "timeout_ms": _ruby_attr(ctx, "timeout_ms", None), "run_from_workspace_root": True},
        "labels": _ruby_attr(ctx, "labels", []),
        "metadata": {},
    }

def _ruby_test_impl(ctx, runner_type, display_name, adapter_source, adapter_name, file_units):
    ruby = _ruby_executable(ctx)
    inputs = _ruby_test_inputs(ctx)
    test_dir = _test_output_dir(ctx)
    results = test_dir + "/test_results.json"
    log = test_dir + "/" + runner_type + ".log"
    native_results = test_dir + "/" + runner_type + ("-results.txt" if file_units else "-results.json")
    adapter = test_dir + "/" + adapter_name
    info = _ruby_test_info(ctx, runner_type, display_name, ruby, adapter, results, log, native_results, test_dir, file_units)
    provider = {"label_id": ctx["label"]["id"], "target_kind": runner_type + "_test", "affected_inputs": inputs, "test_discovery_inputs": inputs, "test_info": info}
    if ctx["capability"] != "test":
        return provider
    write_path(adapter, adapter_source)
    argv = [ruby, adapter, "--once-results", results, "--once-native-results", native_results, "--once-target", ctx["label"]["id"]]
    for source in glob(ctx["srcs"]):
        argv.extend(["--once-source", source])
    for unit in (ctx.get("test") or {}).get("filters") or []:
        argv.extend(["--once-test-unit", unit])
    argv.append("--")
    argv.extend(_ruby_attr(ctx, "args", []))
    version = host_command([ruby, "--version"]).strip()
    run_action(
        argv = argv,
        inputs = _unique(inputs + [adapter]),
        outputs = [test_dir, results, log, native_results],
        clean_paths = [results, log, native_results],
        create_dirs = [test_dir, test_dir + "/home"],
        env = _ruby_env(ctx, test_dir),
        stdout = log,
        stderr = log,
        toolchain_identity = "once." + runner_type + ".v1\x00" + ruby + "\x00" + version,
        identifier = ctx["label"]["id"] + ":" + runner_type,
    )
    return provider

def _ruby_rspec_impl(ctx):
    return _ruby_test_impl(ctx, "rspec", "Ruby Specification", _ruby_rspec_adapter(), "once_rspec_adapter.rb", False)

def _ruby_minitest_impl(ctx):
    return _ruby_test_impl(ctx, "minitest", "Minitest", _ruby_minitest_adapter(), "once_minitest_adapter.rb", True)

_RUBY_TEST_ATTRS = [
    attr("ruby", "string", default = "\"ruby\"", docs = "Ruby interpreter executable name, absolute path, or workspace-relative path.", configurable = False),
    attr("config", "list<string>", default = "[\"Gemfile\", \"Gemfile.lock\"]", docs = "Configuration and dependency inputs.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Runtime data inputs read by tests.", configurable = False),
    attr("args", "list<string>", default = "[]", docs = "Additional runner arguments.", configurable = False),
    attr("env", "map<string,string>", default = "{}", docs = "Environment variables for test execution.", configurable = True),
    attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variables inherited by name.", configurable = False),
    attr("batching", "string", default = "\"file\"", docs = "Automatic batch granularity: `file`, `case`, or `target`.", configurable = False),
    attr("labels", "list<string>", default = "[]", docs = "Labels exposed through test discovery."),
    attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
]

rspec_test = target_kind(
    docs = "Runs Ruby Specification examples through Once with exact example filtering, normalized results, and automatic batching.",
    attrs = _RUBY_TEST_ATTRS,
    deps = [dep("deps", [], "Targets whose inputs should affect this test suite.")],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    tools = [_RUBY_TOOL],
    examples = [example("rspec-test-minimal", name = "Minimal Ruby Specification suite", use_when = "You want Ruby examples with normalized results and automatic batching.")],
    impl = _ruby_rspec_impl,
)

minitest_test = target_kind(
    docs = "Runs Minitest files through Once with stable file discovery, exact file filtering, normalized results, and automatic batching.",
    attrs = _RUBY_TEST_ATTRS,
    deps = [dep("deps", [], "Targets whose inputs should affect this test suite.")],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    tools = [_RUBY_TOOL],
    examples = [example("minitest-test-minimal", name = "Minimal Minitest suite", use_when = "You want Ruby Minitest files scheduled independently by Once.")],
    impl = _ruby_minitest_impl,
)
