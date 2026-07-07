def _elixir_attr(ctx, key, default):
    value = ctx["attr"].get(key)
    if value == None:
        return default
    return value

def _elixir_app_name(ctx):
    return _elixir_attr(ctx, "app_name", ctx["label"]["name"].replace("-", "_").replace(".", "_"))

def _elixir_mix_env(ctx):
    return _elixir_attr(ctx, "mix_env", "prod")

def _elixir_mix_config(ctx, default = ""):
    return _package_relative(ctx, _elixir_attr(ctx, "mix_config", default))

def _elixir_optional_inputs(values):
    out = []
    for value in values:
        if value:
            out.append(value)
    return out

def _elixir_source_patterns(ctx):
    if ctx["srcs"]:
        return ctx["srcs"]
    return [
        "lib/**/*.ex",
        "lib/**/*.exs",
    ]

def _elixir_test_patterns(ctx):
    if ctx["srcs"]:
        return ctx["srcs"]
    return ["test/**/*.exs"]

def _elixir_test_sources(ctx):
    return _unique(glob(["test/test_helper.exs"]) + glob(_elixir_test_patterns(ctx)))

def _elixir_test_input_sources(ctx, srcs):
    return _unique(srcs + glob(["test/support/**/*.ex", "test/support/**/*.exs"]))

def _elixir_glob_attr(ctx, key, default):
    return glob(_elixir_attr(ctx, key, default))

def _elixir_data_inputs(ctx):
    return _elixir_glob_attr(ctx, "data", [])

def _elixir_config_inputs(ctx):
    return _elixir_glob_attr(ctx, "config", ["config/**/*.exs"])

def _elixir_priv_inputs(ctx):
    return _elixir_glob_attr(ctx, "priv", [])

def _elixir_include_inputs(ctx):
    return _elixir_glob_attr(ctx, "include", ["include/**/*.hrl"])

def _elixir_sources(ctx):
    return glob(_elixir_source_patterns(ctx))

def _elixir_shell_words(values):
    return " ".join([_shell_quote(value) for value in values])

def _elixir_lines(values):
    return "\n".join(values) + ("\n" if values else "")

def _elixir_abs_workspace_path(path):
    return workspace_root() + "/" + path

def _elixir_host_read_workspace_file(path):
    abs_path = _elixir_abs_workspace_path(path)
    if not host_file_exists(abs_path):
        return ""
    return host_file_read(abs_path)

def _elixir_workspace_root_rel(ctx):
    package = ctx["label"]["package"]
    if not package:
        return "."
    parts = []
    for part in package.split("/"):
        if part:
            parts.append("..")
    if not parts:
        return "."
    return "/".join(parts)

def _elixir_from_package(ctx, path):
    rel = _elixir_workspace_root_rel(ctx)
    if rel == ".":
        return path
    return rel + "/" + path

def _elixir_package_cwd(ctx):
    return ctx["label"]["package"] or None

def _elixir_is_workspace_input(path):
    if not path:
        return False
    if path.startswith("/") or path == ".." or path.startswith("../") or "/../" in path:
        return False
    return True

def _elixir_previous_compile_metadata(ctx, metadata_path, static_inputs):
    content = _elixir_host_read_workspace_file(metadata_path)
    result = {
        "exists": bool(content),
        "external_inputs": [],
        "has_dynamic": False,
        "static_inputs_match": False,
    }
    if not content:
        return result

    input_hashes = {}
    external_inputs = []
    for line in content.split("\n"):
        if not line:
            continue
        parts = line.split("\t")
        if len(parts) >= 3 and parts[0] == "input":
            input_hashes[parts[1]] = parts[2]
        elif len(parts) >= 2 and parts[0] == "external":
            result["has_dynamic"] = True
            path = parts[1]
            if _elixir_is_workspace_input(path) and host_file_exists(_elixir_abs_workspace_path(path)):
                external_inputs.append(path)
        elif len(parts) >= 2 and (parts[0] == "compile_env" or parts[0] == "recompile"):
            result["has_dynamic"] = True

    current_inputs = _unique(static_inputs)
    static_inputs_match = len(input_hashes) == len(current_inputs)
    if static_inputs_match:
        for path in current_inputs:
            if input_hashes.get(path) != host_file_sha256(_elixir_abs_workspace_path(path)):
                static_inputs_match = False
                break

    result["external_inputs"] = _unique(external_inputs)
    result["static_inputs_match"] = static_inputs_match
    return result

def _elixir_sources_have_dynamic_compile_markers(srcs):
    markers = [
        "@external_resource",
        "Application.compile_env",
        "Application.compile_env!",
        "__mix_recompile__?",
    ]
    for src in srcs:
        abs_path = _elixir_abs_workspace_path(src)
        for marker in markers:
            if host_file_contains(abs_path, marker):
                return True
    return False

def _elixir_app_code_paths(apps):
    paths = []
    for app in apps:
        ebin = app.get("ebin_dir", "")
        if ebin:
            paths.append(ebin)
    return _unique(paths)

def _elixir_compiler_toolchain():
    elixir = host_which("elixir")
    elixirc = host_which("elixirc")
    erl = host_which("erl")
    version = host_command([elixir, "--version"]).strip()
    elixirc_version = host_command([elixirc, "--version"]).strip()
    tool_dir = _parent_dir(elixir)
    erlang_tool_dir = _parent_dir(erl)
    action_path = tool_dir + ":" + erlang_tool_dir + ":/usr/bin:/bin:/usr/sbin:/sbin"
    identity = "once.elixir.compiler.v1\x00" + elixir + "\x00" + elixirc + "\x00" + erl + "\x00" + version + "\x00" + elixirc_version
    return {
        "elixir": elixir,
        "elixirc": elixirc,
        "erl": erl,
        "version": version,
        "elixirc_version": elixirc_version,
        "path": action_path,
        "identity": identity,
    }

def _elixir_mix_toolchain():
    compiler = _elixir_compiler_toolchain()
    mix = host_which("mix")
    mix_version = host_command([mix, "--version"]).strip()
    identity = compiler["identity"] + "\x00mix\x00" + mix + "\x00" + mix_version
    return {
        "elixir": compiler["elixir"],
        "elixirc": compiler["elixirc"],
        "mix": mix,
        "erl": compiler["erl"],
        "version": compiler["version"],
        "elixirc_version": compiler["elixirc_version"],
        "mix_version": mix_version,
        "path": compiler["path"],
        "identity": identity,
    }

def _elixir_action_env(toolchain):
    env = {
        "PATH": toolchain["path"],
    }
    lang = host_env("LANG")
    if lang:
        env["LANG"] = lang
    lc_all = host_env("LC_ALL")
    if lc_all:
        env["LC_ALL"] = lc_all
    return env

def _elixir_user_env(ctx):
    out = {}
    for key, value in _elixir_attr(ctx, "env", {}).items():
        out[key] = value
    return out

def _elixir_action_env_with(ctx, toolchain, extra):
    env = _elixir_action_env(toolchain)
    for key, value in _elixir_user_env(ctx).items():
        env[key] = value
    for key, value in extra.items():
        env[key] = value
    return env

def _elixir_export_env(env):
    lines = []
    for key, value in env.items():
        lines.append("export " + key + "=" + _shell_quote(value))
    return "\n".join(lines)

def _elixir_collect_apps(deps):
    seen = {}
    apps = []
    for dep in deps:
        for app in dep.get("transitive_elixir_apps", []):
            key = app.get("app_name", "") + "\x00" + app.get("ebin_dir", "")
            if key and key not in seen:
                seen[key] = True
                apps.append(app)
    return apps

def _elixir_collect_transitive_strings(deps, key, own_values):
    values = []
    for dep in deps:
        values.extend(dep.get(key, []))
    values.extend(own_values)
    return _unique(values)

def _elixir_transitive_apps(deps, own_app):
    apps = _elixir_collect_apps(deps)
    key = own_app.get("app_name", "") + "\x00" + own_app.get("ebin_dir", "")
    for app in apps:
        existing = app.get("app_name", "") + "\x00" + app.get("ebin_dir", "")
        if existing == key:
            return apps
    apps.append(own_app)
    return apps

def _elixir_dep_symlink_script(apps, root_var, deps_root_var):
    lines = [
        "rm -rf \"$" + deps_root_var + "\"",
        "mkdir -p \"$" + deps_root_var + "\"",
    ]
    for app in apps:
        app_name = app.get("app_name", "")
        ebin = app.get("ebin_dir", "")
        priv = app.get("priv_dir", "")
        if not app_name or not ebin:
            continue
        app_dir = "\"$" + deps_root_var + "\"/" + _shell_quote(app_name)
        lines.append("mkdir -p " + app_dir)
        lines.append("ln -s \"$" + root_var + "\"/" + _shell_quote(ebin) + " " + app_dir + "/ebin")
        if priv:
            lines.append("ln -s \"$" + root_var + "\"/" + _shell_quote(priv) + " " + app_dir + "/priv")
    return "\n".join(lines)

def _elixir_copy_priv_script(ctx, priv_srcs, priv_dir):
    if not priv_srcs:
        return "mkdir -p \"$PRIV_OUT\""
    lines = ["mkdir -p \"$PRIV_OUT\""]
    package = ctx["label"]["package"]
    for src in priv_srcs:
        rel = src
        if package and rel.startswith(package + "/"):
            rel = rel[len(package) + 1:]
        if rel.startswith("priv/"):
            rel = rel[len("priv/"):]
        dest = "\"$PRIV_OUT\"/" + _shell_quote(rel)
        lines.append("mkdir -p \"$(dirname " + dest + ")\"")
        lines.append("cp \"$WORKSPACE_ROOT\"/" + _shell_quote(src) + " " + dest)
    return "\n".join(lines)

def _elixir_erlang_atom(value):
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"

def _elixir_erlang_string(value):
    return "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

def _elixir_erlang_atom_list(values):
    return "[" + ",".join([_elixir_erlang_atom(value) for value in values]) + "]"

def _elixir_write_app_script(ctx, app_name, ebin_dir):
    version = _elixir_attr(ctx, "version", "0.1.0")
    description = _elixir_attr(ctx, "description", "")
    applications = _elixir_attr(ctx, "applications", ["kernel", "stdlib", "elixir"])
    included_applications = _elixir_attr(ctx, "included_applications", [])
    registered = _elixir_attr(ctx, "registered", [])
    app_file = app_name + ".app"
    lines = [
        "MODULES_FILE=\"$STAGE_ROOT/modules.txt\"",
        "find \"$EBIN_OUT\" -maxdepth 1 -type f -name '*.beam' | sort > \"$MODULES_FILE\"",
        "modules=\"\"",
        "while IFS= read -r beam; do",
        "  name=$(basename \"$beam\" .beam)",
        "  escaped=$(printf '%s' \"$name\" | sed \"s/'/\\\\'/g\")",
        "  if [ -n \"$modules\" ]; then modules=\"$modules,\"; fi",
        "  modules=\"$modules'$escaped'\"",
        "done < \"$MODULES_FILE\"",
        "{",
        "  printf '%s\\n' " + _shell_quote("{application, " + _elixir_erlang_atom(app_name) + ","),
        "  printf '%s\\n' " + _shell_quote(" [{description, " + _elixir_erlang_string(description) + "},"),
        "  printf '%s\\n' " + _shell_quote("  {vsn, " + _elixir_erlang_string(version) + "},"),
        "  printf '%s\\n' " + _shell_quote("  {registered, " + _elixir_erlang_atom_list(registered) + "},"),
        "  printf '%s\\n' " + _shell_quote("  {applications, " + _elixir_erlang_atom_list(applications) + "},"),
        "  printf '%s\\n' " + _shell_quote("  {included_applications, " + _elixir_erlang_atom_list(included_applications) + "},"),
        "  printf '  {modules, [%s]},\\n' \"$modules\"",
        "  printf '%s\\n' " + _shell_quote("  {env, []}]}." ),
        "} > \"$EBIN_OUT\"/" + _shell_quote(app_file),
    ]
    return "\n".join(lines)

def _elixir_stale_fingerprint_source():
    return """
defmodule Once.ElixirStaleProbe do
  def main do
    workspace_root = Path.expand(System.fetch_env!("ONCE_WORKSPACE_ROOT"), File.cwd!())
    metadata_path = Path.join(workspace_root, System.fetch_env!("ONCE_METADATA_PATH"))
    fingerprint_path = Path.join(workspace_root, System.fetch_env!("ONCE_FINGERPRINT_PATH"))
    ebin_dir = Path.join(workspace_root, System.fetch_env!("ONCE_EBIN_DIR"))
    mix_env = System.fetch_env!("MIX_ENV") |> String.to_atom()
    config_paths = read_lines(Path.join(workspace_root, System.fetch_env!("ONCE_CONFIGS_FILE"))) |> Enum.map(&Path.join(workspace_root, &1))

    load_config(config_paths, mix_env)
    metadata = read_metadata(metadata_path)
    Code.prepend_path(ebin_dir)

    lines = ["version\t1"]
    lines = lines ++ external_fingerprints(metadata.external, workspace_root)
    lines = lines ++ compile_env_fingerprints(metadata.compile_env)
    lines = lines ++ recompile_fingerprints(metadata.recompile_modules)

    File.write!(fingerprint_path, Enum.sort(lines) |> Enum.join("\n") |> Kernel.<>("\n"))
  end

  defp read_lines(path) do
    path
    |> File.read!()
    |> String.split("\n", trim: true)
  end

  defp read_metadata(path) do
    base = %{external: [], compile_env: [], recompile_modules: []}

    if File.regular?(path) do
      path
      |> read_lines()
      |> Enum.reduce(base, fn line, acc ->
        case String.split(line, "\t") do
          ["external", path | _] -> %{acc | external: [path | acc.external]}
          ["compile_env", encoded | _] -> %{acc | compile_env: [decode_term(encoded) | acc.compile_env]}
          ["recompile", module | _] -> %{acc | recompile_modules: [module | acc.recompile_modules]}
          _ -> acc
        end
      end)
    else
      base
    end
  end

  defp load_config(config_paths, mix_env) do
    Enum.each(config_paths, fn path ->
      if File.regular?(path) do
        path
        |> Config.Reader.read!(env: mix_env, target: :host)
        |> Application.put_all_env(persistent: true)
      end
    end)
  end

  defp external_fingerprints(paths, workspace_root) do
    paths
    |> Enum.uniq()
    |> Enum.map(fn path ->
      absolute = external_absolute_path(path, workspace_root)

      case File.read(absolute) do
        {:ok, bytes} -> "external\t#{path}\t#{sha256(bytes)}"
        {:error, _} -> "external\t#{path}\tmissing"
      end
    end)
  end

  defp compile_env_fingerprints(compile_env) do
    Enum.map(compile_env, fn entry ->
      status = if Config.Provider.valid_compile_env?([entry]), do: "valid", else: "invalid"
      "compile_env\t#{encode_term(entry)}\t#{status}"
    end)
  end

  defp recompile_fingerprints(modules) do
    modules
    |> Enum.uniq()
    |> Enum.map(fn module_name ->
      module = String.to_atom(module_name)

      status =
        with {:module, ^module} <- Code.ensure_loaded(module),
             true <- function_exported?(module, :__mix_recompile__?, 0) do
          if module.__mix_recompile__?() do
            "true:#{System.os_time(:nanosecond)}"
          else
            "false"
          end
        else
          _ -> "unavailable:#{System.os_time(:nanosecond)}"
        end

      "recompile\t#{module_name}\t#{status}"
    end)
  end

  defp external_absolute_path(path, workspace_root) do
    if Path.type(path) == :absolute do
      path
    else
      Path.expand(path, workspace_root)
    end
  end

  defp encode_term(term), do: term |> :erlang.term_to_binary() |> Base.encode64()
  defp decode_term(encoded), do: encoded |> Base.decode64!() |> :erlang.binary_to_term()
  defp sha256(bytes), do: :crypto.hash(:sha256, bytes) |> Base.encode16(case: :lower)
end

Once.ElixirStaleProbe.main()
"""

def _elixir_compile_source():
    return """
defmodule Once.ElixirCompiler do
  def main do
    workspace_root = Path.expand(System.fetch_env!("ONCE_WORKSPACE_ROOT"), File.cwd!())
    out_dir = Path.join(workspace_root, System.fetch_env!("ONCE_OUT_DIR"))
    metadata_path = Path.join(workspace_root, System.fetch_env!("ONCE_METADATA_PATH"))
    warnings_path = Path.join(workspace_root, System.fetch_env!("ONCE_WARNINGS_PATH"))
    consolidated_dir = Path.join(workspace_root, System.fetch_env!("ONCE_CONSOLIDATED_DIR"))
    mix_env = System.fetch_env!("MIX_ENV") |> String.to_atom()
    consolidate_protocols? = System.fetch_env!("ONCE_CONSOLIDATE_PROTOCOLS") == "true"
    sources = read_lines(Path.join(workspace_root, System.fetch_env!("ONCE_SOURCES_FILE"))) |> Enum.map(&Path.join(workspace_root, &1))
    static_inputs = read_lines(Path.join(workspace_root, System.fetch_env!("ONCE_STATIC_INPUTS_FILE")))
    config_paths = read_lines(Path.join(workspace_root, System.fetch_env!("ONCE_CONFIGS_FILE"))) |> Enum.map(&Path.join(workspace_root, &1))
    compile_args = read_lines(Path.join(workspace_root, System.fetch_env!("ONCE_COMPILE_ARGS_FILE")))
    dep_code_paths = read_lines(Path.join(workspace_root, System.fetch_env!("ONCE_DEP_CODE_PATHS_FILE"))) |> Enum.map(&Path.join(workspace_root, &1))

    Enum.each(Enum.reverse(dep_code_paths), &Code.prepend_path/1)
    load_config(config_paths, mix_env)

    {compiler_options, parallel_options, warnings_as_errors?} = parse_compile_args(compile_args)
    previous_options = Code.compiler_options(compiler_options)
    {:ok, agent} = Agent.start_link(fn -> %{files: %{}, modules: %{}} end)

    callbacks = [
      each_file: fn file, lexical ->
        references = Kernel.LexicalTracker.references(lexical)
        record_file(agent, workspace_root, file, references)
      end,
      each_module: fn file, module, _binary ->
        record_module(agent, workspace_root, file, module)
      end,
      return_diagnostics: true
    ]

    try do
      result = Kernel.ParallelCompiler.compile_to_path(sources, out_dir, callbacks ++ parallel_options)
      handle_result(result, agent, workspace_root, static_inputs, out_dir, consolidated_dir, metadata_path, warnings_path, warnings_as_errors?, consolidate_protocols?, dep_code_paths)
    after
      Code.compiler_options(previous_options)
    end
  end

  defp read_lines(path) do
    path
    |> File.read!()
    |> String.split("\n", trim: true)
  end

  defp load_config(config_paths, mix_env) do
    Enum.each(config_paths, fn path ->
      if File.regular?(path) do
        path
        |> Config.Reader.read!(env: mix_env, target: :host)
        |> Application.put_all_env(persistent: true)
      end
    end)
  end

  defp parse_compile_args(args), do: parse_compile_args(args, [], [], false)
  defp parse_compile_args([], compiler, parallel, warnings_as_errors?), do: {compiler, parallel, warnings_as_errors?}
  defp parse_compile_args(["--ignore-module-conflict" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, Keyword.put(compiler, :ignore_module_conflict, true), parallel, warnings_as_errors?)
  defp parse_compile_args(["--warnings-as-errors" | rest], compiler, parallel, _), do: parse_compile_args(rest, compiler, parallel, true)
  defp parse_compile_args(["--no-warnings-as-errors" | rest], compiler, parallel, _), do: parse_compile_args(rest, compiler, parallel, false)
  defp parse_compile_args(["--debug-info" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, Keyword.put(compiler, :debug_info, true), parallel, warnings_as_errors?)
  defp parse_compile_args(["--no-debug-info" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, Keyword.put(compiler, :debug_info, false), parallel, warnings_as_errors?)
  defp parse_compile_args(["--docs" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, Keyword.put(compiler, :docs, true), parallel, warnings_as_errors?)
  defp parse_compile_args(["--no-docs" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, Keyword.put(compiler, :docs, false), parallel, warnings_as_errors?)
  defp parse_compile_args(["--no-verification" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, compiler, Keyword.put(parallel, :verification, false), warnings_as_errors?)
  defp parse_compile_args(["--verbose" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, compiler, Keyword.put(parallel, :verbose, true), warnings_as_errors?)
  defp parse_compile_args(["--long-compilation-threshold", value | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, compiler, Keyword.put(parallel, :long_compilation_threshold, String.to_integer(value)), warnings_as_errors?)
  defp parse_compile_args(["--long-verification-threshold", value | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, compiler, Keyword.put(parallel, :long_verification_threshold, String.to_integer(value)), warnings_as_errors?)
  defp parse_compile_args(["--profile", "time" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, compiler, Keyword.put(parallel, :profile, :time), warnings_as_errors?)
  defp parse_compile_args(["-pa", path | rest], compiler, parallel, warnings_as_errors?) do
    Code.prepend_path(path)
    parse_compile_args(rest, compiler, parallel, warnings_as_errors?)
  end
  defp parse_compile_args(["--no-optional-deps" | rest], compiler, parallel, warnings_as_errors?), do: parse_compile_args(rest, compiler, parallel, warnings_as_errors?)
  defp parse_compile_args([arg | _], _compiler, _parallel, _warnings_as_errors?) do
    IO.puts(:stderr, "unsupported elixir_library compile_args entry: #{arg}")
    System.halt(64)
  end

  defp handle_result({:ok, _modules, info}, agent, workspace_root, static_inputs, out_dir, consolidated_dir, metadata_path, warnings_path, warnings_as_errors?, consolidate_protocols?, dep_code_paths) do
    diagnostics = diagnostics(info)
    print_diagnostics(diagnostics)
    write_warning_log(warnings_path, diagnostics)
    if consolidate_protocols?, do: consolidate_protocols(out_dir, consolidated_dir, dep_code_paths)
    write_metadata(metadata_path, Agent.get(agent, & &1), workspace_root, static_inputs, diagnostics)

    if warnings_as_errors? and diagnostics != [] do
      IO.puts(:stderr, "Compilation failed due to warnings while using the --warnings-as-errors option")
      System.halt(1)
    end
  end

  defp handle_result({:error, errors, info}, _agent, _workspace_root, _static_inputs, _out_dir, _consolidated_dir, _metadata_path, warnings_path, _warnings_as_errors?, _consolidate_protocols?, _dep_code_paths) do
    diagnostics = diagnostics(info) ++ errors
    print_diagnostics(diagnostics)
    write_warning_log(warnings_path, diagnostics)
    System.halt(1)
  end

  defp diagnostics(info), do: Map.get(info, :runtime_warnings, []) ++ Map.get(info, :compile_warnings, [])

  defp print_diagnostics(diagnostics), do: Enum.each(diagnostics, &Code.print_diagnostic/1)

  defp write_warning_log(path, diagnostics) do
    diagnostics
    |> Enum.map(&diagnostic_line/1)
    |> Enum.join("")
    |> then(&File.write!(path, &1))
  end

  defp diagnostic_line(%{file: file, position: position, severity: severity, message: message}) do
    "#{file}:#{inspect(position)}: #{severity}: #{message}\n"
  end

  defp diagnostic_line(diagnostic), do: inspect(diagnostic) <> "\n"

  defp record_file(agent, workspace_root, file, {compile_refs, export_refs, runtime_refs, compile_env}) do
    path = workspace_relative(file, workspace_root)

    Agent.update(agent, fn state ->
      entry = Map.get(state.files, path, %{compile_env: [], external: [], modules: []})
      entry = Map.merge(entry, %{compile_refs: compile_refs, export_refs: export_refs, runtime_refs: runtime_refs, compile_env: compile_env})
      %{state | files: Map.put(state.files, path, entry)}
    end)
  end

  defp record_module(agent, workspace_root, file, module) do
    path = workspace_relative(file, workspace_root)
    external = Module.get_attribute(module, :external_resource) |> List.wrap() |> Enum.map(&workspace_relative(&1, workspace_root))
    recompile? = Module.defines?(module, {:__mix_recompile__?, 0}, :def)
    kind = module_kind(module)

    Agent.update(agent, fn state ->
      entry = Map.get(state.files, path, %{compile_env: [], external: [], modules: []})
      entry = %{entry | external: Enum.uniq(entry.external ++ external), modules: Enum.uniq([module | entry.modules])}
      module_entry = %{source: path, kind: kind, recompile?: recompile?}
      %{state | files: Map.put(state.files, path, entry), modules: Map.put(state.modules, module, module_entry)}
    end)
  end

  defp module_kind(module) do
    protocol_metadata = Module.get_attribute(module, :__impl__)

    cond do
      is_list(protocol_metadata) and protocol_metadata[:protocol] -> {:impl, protocol_metadata[:protocol]}
      is_list(Module.get_attribute(module, :__protocol__)) -> :protocol
      true -> :module
    end
  end

  defp consolidate_protocols(out_dir, consolidated_dir, dep_code_paths) do
    File.rm_rf!(consolidated_dir)
    File.mkdir_p!(consolidated_dir)
    paths = [out_dir | dep_code_paths] |> Enum.uniq() |> Enum.map(&path_with_files/1)

    paths
    |> Protocol.extract_protocols()
    |> Enum.uniq()
    |> Enum.each(fn protocol ->
      impls = Protocol.extract_impls(protocol, paths)
      :code.purge(protocol)
      :code.delete(protocol)

      case Protocol.consolidate(protocol, impls) do
        {:ok, binary} -> File.write!(Path.join(consolidated_dir, Atom.to_string(protocol) <> ".beam"), binary)
        {:error, :no_beam_info} -> :ok
      end
    end)
  end

  defp path_with_files(path) do
    files = case File.ls(path) do
      {:ok, files} -> Enum.map(files, &String.to_charlist/1)
      {:error, _} -> []
    end

    {String.to_charlist(path), files}
  end

  defp write_metadata(path, state, workspace_root, static_inputs, diagnostics) do
    input_lines = Enum.map(static_inputs, fn input -> "input\t#{input}\t#{file_sha256(Path.join(workspace_root, input))}" end)

    external_lines =
      state.files
      |> Enum.flat_map(fn {_source, entry} -> entry.external end)
      |> Enum.uniq()
      |> Enum.map(&"external\t#{&1}")

    compile_env_lines =
      state.files
      |> Enum.flat_map(fn {_source, entry} -> entry.compile_env end)
      |> Enum.uniq()
      |> Enum.map(&"compile_env\t#{encode_term(&1)}")

    recompile_lines =
      state.modules
      |> Enum.filter(fn {_module, entry} -> entry.recompile? end)
      |> Enum.map(fn {module, _entry} -> "recompile\t#{Atom.to_string(module)}" end)

    protocol_lines =
      state.modules
      |> Enum.flat_map(fn {module, entry} ->
        case entry.kind do
          :protocol -> ["protocol\t#{Atom.to_string(module)}"]
          {:impl, protocol} -> ["impl\t#{Atom.to_string(module)}\t#{Atom.to_string(protocol)}"]
          :module -> []
        end
      end)

    warning_lines = Enum.map(diagnostics, &"warning\t#{encode_term(&1)}")

    lines = ["version\t1"] ++ input_lines ++ external_lines ++ compile_env_lines ++ recompile_lines ++ protocol_lines ++ warning_lines
    File.write!(path, Enum.sort(lines) |> Enum.join("\n") |> Kernel.<>("\n"))
  end

  defp workspace_relative(path, workspace_root) do
    path
    |> to_string()
    |> Path.expand(File.cwd!())
    |> Path.relative_to(workspace_root)
  end

  defp file_sha256(path), do: path |> File.read!() |> sha256()
  defp sha256(bytes), do: :crypto.hash(:sha256, bytes) |> Base.encode16(case: :lower)
  defp encode_term(term), do: term |> :erlang.term_to_binary() |> Base.encode64()
end

Once.ElixirCompiler.main()
"""

def _elixir_stage_script(ctx, apps, module_dirs, priv_srcs, ebin_dir, priv_dir):
    app_name = _elixir_app_name(ctx)
    mix_env = _elixir_mix_env(ctx)
    stage_root = ctx["scratch_dir"] + "/stage"
    deps_root = ctx["scratch_dir"] + "/stage_deps"
    user_env = _elixir_user_env(ctx)
    copy_modules = []
    for module_dir in module_dirs:
        copy_modules.append("if [ -d \"$WORKSPACE_ROOT\"/" + _shell_quote(module_dir) + " ]; then find \"$WORKSPACE_ROOT\"/" + _shell_quote(module_dir) + " -maxdepth 1 -type f -name '*.beam' -exec cp {} \"$EBIN_OUT\"/ \\;; fi")
    return """set -eu
WORKSPACE_ROOT="$PWD"
STAGE_ROOT={stage_root}
DEPS_ROOT={deps_root}
EBIN_OUT={ebin_dir}
PRIV_OUT={priv_dir}
rm -rf "$STAGE_ROOT" "$DEPS_ROOT" "$EBIN_OUT" "$PRIV_OUT"
mkdir -p "$STAGE_ROOT" "$DEPS_ROOT" "$EBIN_OUT" "$PRIV_OUT"
{dep_symlinks}
cd {package}
{user_env}
export MIX_ENV={mix_env}
export HEX_OFFLINE=true
export ERL_LIBS="$WORKSPACE_ROOT/$DEPS_ROOT"
{copy_modules}
{write_app}
{copy_priv}
""".format(
        stage_root = _shell_quote(stage_root),
        deps_root = _shell_quote(deps_root),
        ebin_dir = _shell_quote(ebin_dir),
        priv_dir = _shell_quote(priv_dir),
        dep_symlinks = _elixir_dep_symlink_script(apps, "WORKSPACE_ROOT", "DEPS_ROOT"),
        package = _shell_quote(ctx["label"]["package"] or "."),
        user_env = _elixir_export_env(user_env),
        mix_env = _shell_quote(mix_env),
        copy_modules = "\n".join(copy_modules),
        write_app = _elixir_write_app_script(ctx, app_name, ebin_dir),
        copy_priv = _elixir_copy_priv_script(ctx, priv_srcs, priv_dir),
    )

def _elixir_stale_fingerprint_action(ctx, toolchain, metadata_path, config, ebin_dir, fingerprint):
    mix_env = _elixir_mix_env(ctx)
    scratch = ctx["scratch_dir"]
    home_dir = scratch + "/stale_home"
    script_dir = scratch + "/stale_probe"
    configs_list = script_dir + "/configs.list"
    probe_exs = script_dir + "/probe.exs"
    write_path(configs_list, _elixir_lines(config))
    write_path(probe_exs, _elixir_stale_fingerprint_source())
    env = _elixir_action_env_with(ctx, toolchain, {
        "HOME": _elixir_from_package(ctx, home_dir),
        "MIX_ENV": mix_env,
        "HEX_OFFLINE": "true",
        "ONCE_WORKSPACE_ROOT": _elixir_workspace_root_rel(ctx),
        "ONCE_METADATA_PATH": metadata_path,
        "ONCE_FINGERPRINT_PATH": fingerprint,
        "ONCE_EBIN_DIR": ebin_dir,
        "ONCE_CONFIGS_FILE": configs_list,
    })
    run_action(
        argv = [toolchain["elixir"], _elixir_from_package(ctx, probe_exs)],
        inputs = [],
        outputs = [fingerprint],
        # script_dir holds the write_path outputs (probe.exs, configs.list),
        # so it must not be cleaned here or the probe would delete its own
        # program before running.
        clean_paths = [home_dir, fingerprint],
        create_dirs = [home_dir],
        cwd = _elixir_package_cwd(ctx),
        env = env,
        cacheable = False,
        toolchain_identity = toolchain["identity"] + "\x00elixir-stale-fingerprint.v1\x00mix_env\x00" + mix_env,
        identifier = ctx["label"]["id"] + ":elixir-stale-fingerprint",
    )

def _elixir_compile_action(ctx, toolchain, apps, srcs, static_inputs, config, compile_args, compile_inputs, module_dir, metadata_path, warnings_path, consolidated_dir, cacheable):
    mix_env = _elixir_mix_env(ctx)
    scratch = ctx["scratch_dir"]
    home_dir = scratch + "/compile_home"
    script_dir = scratch + "/compile_script"
    dep_code_paths = _elixir_app_code_paths(apps)
    consolidate_protocols = "true" if _elixir_attr(ctx, "consolidate_protocols", True) else "false"
    sources_list = script_dir + "/sources.list"
    static_inputs_list = script_dir + "/static_inputs.list"
    configs_list = script_dir + "/configs.list"
    compile_args_list = script_dir + "/compile_args.list"
    dep_code_paths_list = script_dir + "/dep_code_paths.list"
    compile_exs = script_dir + "/compile.exs"
    write_path(sources_list, _elixir_lines(srcs))
    write_path(static_inputs_list, _elixir_lines(static_inputs))
    write_path(configs_list, _elixir_lines(config))
    write_path(compile_args_list, _elixir_lines(["--ignore-module-conflict"] + compile_args))
    write_path(dep_code_paths_list, _elixir_lines(dep_code_paths))
    write_path(compile_exs, _elixir_compile_source())
    env = _elixir_action_env_with(ctx, toolchain, {
        "HOME": _elixir_from_package(ctx, home_dir),
        "MIX_ENV": mix_env,
        "HEX_OFFLINE": "true",
        "ONCE_WORKSPACE_ROOT": _elixir_workspace_root_rel(ctx),
        "ONCE_OUT_DIR": module_dir,
        "ONCE_METADATA_PATH": metadata_path,
        "ONCE_WARNINGS_PATH": warnings_path,
        "ONCE_CONSOLIDATED_DIR": consolidated_dir,
        "ONCE_CONSOLIDATE_PROTOCOLS": consolidate_protocols,
        "ONCE_SOURCES_FILE": sources_list,
        "ONCE_STATIC_INPUTS_FILE": static_inputs_list,
        "ONCE_CONFIGS_FILE": configs_list,
        "ONCE_COMPILE_ARGS_FILE": compile_args_list,
        "ONCE_DEP_CODE_PATHS_FILE": dep_code_paths_list,
    })
    run_action(
        argv = [toolchain["elixir"], _elixir_from_package(ctx, compile_exs)],
        inputs = compile_inputs,
        outputs = [module_dir, metadata_path, warnings_path, consolidated_dir],
        clean_paths = [module_dir, metadata_path, warnings_path, consolidated_dir, home_dir],
        create_dirs = [module_dir, consolidated_dir, home_dir],
        cwd = _elixir_package_cwd(ctx),
        env = env,
        cacheable = cacheable,
        toolchain_identity = toolchain["identity"] + "\x00elixir-compile.target.v2\x00mix_env\x00" + mix_env,
        identifier = ctx["label"]["id"] + ":elixir-compile",
    )

def _elixir_library_impl(ctx):
    toolchain = _elixir_compiler_toolchain()
    app_name = _elixir_app_name(ctx)
    mix_env = _elixir_mix_env(ctx)
    srcs = _elixir_sources(ctx)
    config = _elixir_config_inputs(ctx)
    data = _elixir_data_inputs(ctx)
    priv_srcs = _elixir_priv_inputs(ctx)
    include = _elixir_include_inputs(ctx)
    mix_config = _elixir_mix_config(ctx)
    apps = _elixir_collect_apps(ctx["deps"])
    ebin_dir = declare_output("ebin")
    priv_dir = declare_output("priv")
    module_dir = declare_output("modules")
    metadata_path = declare_output("compile-metadata.tsv")
    warnings_path = declare_output("compile-warnings.log")
    consolidated_dir = declare_output("consolidated")
    stale_fingerprint = declare_output("stale-fingerprint.txt")
    module_dirs = [module_dir, consolidated_dir]
    compile_args = _elixir_attr(ctx, "compile_args", [])
    compile_inputs = _unique(_elixir_optional_inputs(config + data + include + [mix_config]))
    static_compile_inputs = _unique(srcs + compile_inputs)
    previous_metadata = _elixir_previous_compile_metadata(ctx, metadata_path, static_compile_inputs)
    dynamic_inputs = previous_metadata["external_inputs"]
    has_dynamic_markers = _elixir_sources_have_dynamic_compile_markers(srcs)
    needs_stale_probe = has_dynamic_markers or previous_metadata["has_dynamic"]
    compile_cacheable = (previous_metadata["exists"] and previous_metadata["static_inputs_match"]) or not has_dynamic_markers
    compile_action_inputs = _unique(static_compile_inputs + dynamic_inputs)
    if needs_stale_probe:
        _elixir_stale_fingerprint_action(ctx, toolchain, metadata_path, config, ebin_dir, stale_fingerprint)
        compile_action_inputs = _unique(compile_action_inputs + [stale_fingerprint])
    _elixir_compile_action(ctx, toolchain, apps, srcs, static_compile_inputs, config, compile_args, compile_action_inputs, module_dir, metadata_path, warnings_path, consolidated_dir, compile_cacheable)
    run_action(
        argv = ["/bin/sh", "-c", _elixir_stage_script(ctx, apps, module_dirs, priv_srcs, ebin_dir, priv_dir)],
        inputs = _unique(_elixir_optional_inputs(priv_srcs + [mix_config])),
        outputs = [ebin_dir, priv_dir],
        env = _elixir_action_env(toolchain),
        toolchain_identity = toolchain["identity"] + "\x00elixir-stage.v1\x00mix_env\x00" + mix_env,
        identifier = ctx["label"]["id"] + ":elixir-stage",
    )
    app = {
        "app_name": app_name,
        "mix_env": mix_env,
        "mix_config": mix_config,
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "compile_metadata": metadata_path,
        "compile_warnings": warnings_path,
    }
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "elixir_library",
        "app_name": app_name,
        "mix_env": mix_env,
        "mix_config": mix_config,
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "compile_metadata": metadata_path,
        "compile_warnings": warnings_path,
        "sources": srcs,
        "transitive_sources": _elixir_collect_transitive_strings(ctx["deps"], "transitive_sources", srcs),
        "transitive_elixir_apps": _elixir_transitive_apps(ctx["deps"], app),
    }

def _elixir_test_paths(ctx, srcs):
    package = ctx["label"]["package"]
    paths = []
    for src in srcs:
        if package and src.startswith(package + "/"):
            paths.append(src[len(package) + 1:])
        else:
            paths.append(src)
    return paths

def _elixir_code_path_flags(apps):
    seen = {}
    flags = []
    for app in apps:
        ebin = app.get("ebin_dir", "")
        if ebin and ebin not in seen:
            seen[ebin] = True
            flags.append("-pa \"$WORKSPACE_ROOT\"/" + _shell_quote(ebin))
    return " ".join(flags)

def _elixir_test_runner_command(ctx, toolchain, apps, srcs, mix_config):
    test_paths = _elixir_test_paths(ctx, srcs)
    opts = _elixir_attr(ctx, "test_args", [])
    if mix_config:
        no_start = ["--no-start"] if _elixir_attr(ctx, "no_start", False) else []
        return _shell_quote(toolchain["mix"]) + " test --no-compile --no-deps-check " + _elixir_shell_words(no_start + test_paths + opts)

    start_args = []
    if "test/test_helper.exs" not in test_paths:
        start_args = ["-e", "ExUnit.start()"]
    require_args = []
    for path in test_paths:
        require_args.extend(["-r", path])
    return _shell_quote(toolchain["elixir"]) + " " + _elixir_code_path_flags(apps) + " " + _elixir_shell_words(start_args + require_args + opts)

def _elixir_test_info(ctx, runner_type, runner_display_name, runner, args, results, log, native_results):
    labels = _elixir_attr(ctx, "labels", [])
    timeout_ms = _elixir_attr(ctx, "timeout_ms", None)
    return {
        "schema": "once.test_info.v1",
        "target": ctx["label"]["id"],
        "runner": {
            "type": runner_type,
            "display_name": runner_display_name,
            "metadata": {},
        },
        "command": {
            "argv": [runner] + args,
            "env": _elixir_user_env(ctx),
            "cwd": ctx["label"]["package"] or ".",
        },
        "outputs": {
            "results": results,
            "logs": [log],
            "native_results": [native_results],
            "coverage": [],
        },
        "listing": {
            "supported": False,
            "strategy": "unsupported",
        },
        "filtering": {
            "case_filtering": "runner_args",
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
    }

def _elixir_test_info_for(ctx, runner, mix_config, srcs, results, log, native_results):
    test_paths = _elixir_test_paths(ctx, srcs)
    if mix_config:
        return _elixir_test_info(ctx, "mix_test", "Mix test", runner, ["test", "--no-compile", "--no-deps-check"] + test_paths + _elixir_attr(ctx, "test_args", []), results, log, native_results)
    start_args = []
    if "test/test_helper.exs" not in test_paths:
        start_args = ["-e", "ExUnit.start()"]
    require_args = []
    for path in test_paths:
        require_args.extend(["-r", path])
    return _elixir_test_info(ctx, "elixir_exunit", "Elixir ExUnit", runner, start_args + require_args + _elixir_attr(ctx, "test_args", []), results, log, native_results)

def _elixir_test_script(ctx, toolchain, lib, apps, srcs, config, data, mix_config, results, log, native_results):
    app_name = lib["app_name"]
    build_root = ctx["scratch_dir"] + "/test_build"
    home = ctx["scratch_dir"] + "/test_home"
    deps_root = ctx["scratch_dir"] + "/test_deps"
    user_env = _elixir_user_env(ctx)
    runner = _elixir_test_runner_command(ctx, toolchain, apps, srcs, mix_config)
    return """set -eu
WORKSPACE_ROOT="$PWD"
BUILD_ROOT={build_root}
HOME_DIR={home}
DEPS_ROOT={deps_root}
RESULTS={results}
LOG={log}
NATIVE_RESULTS={native_results}
rm -rf "$BUILD_ROOT" "$HOME_DIR" "$DEPS_ROOT"
mkdir -p "$BUILD_ROOT/test/lib/{app_name}" "$HOME_DIR" "$(dirname "$RESULTS")" "$(dirname "$LOG")" "$(dirname "$NATIVE_RESULTS")"
ln -s "$WORKSPACE_ROOT"/{lib_ebin} "$BUILD_ROOT/test/lib/{app_name}/ebin"
if [ -d "$WORKSPACE_ROOT"/{lib_priv} ]; then
  ln -s "$WORKSPACE_ROOT"/{lib_priv} "$BUILD_ROOT/test/lib/{app_name}/priv"
fi
{dep_symlinks}
cd {package}
{user_env}
export HOME="$WORKSPACE_ROOT/$HOME_DIR"
export MIX_ENV=test
export MIX_BUILD_ROOT="$WORKSPACE_ROOT/$BUILD_ROOT"
export MIX_HOME="$WORKSPACE_ROOT/$HOME_DIR/.mix"
export HEX_OFFLINE=true
export ERL_LIBS="$WORKSPACE_ROOT/$DEPS_ROOT"
set +e
{runner} > "$WORKSPACE_ROOT/$LOG" 2>&1
status=$?
set -e
cp "$WORKSPACE_ROOT/$LOG" "$WORKSPACE_ROOT/$NATIVE_RESULTS"
summary_line=$(grep -E '[0-9]+ tests?, [0-9]+ failures' "$WORKSPACE_ROOT/$LOG" | tail -n 1 || true)
if [ -n "$summary_line" ]; then
  total=$(printf '%s\\n' "$summary_line" | sed -E 's/.*([0-9]+) tests?, ([0-9]+) failures.*/\\1/')
  failed=$(printf '%s\\n' "$summary_line" | sed -E 's/.*([0-9]+) tests?, ([0-9]+) failures.*/\\2/')
  passed=$((total - failed))
else
  result_line=$(grep -E '^Result: ' "$WORKSPACE_ROOT/$LOG" | tail -n 1 || true)
  if printf '%s\\n' "$result_line" | grep -Eq '^Result: [0-9]+/[0-9]+ passed'; then
    passed=$(printf '%s\\n' "$result_line" | sed -E 's/^Result: ([0-9]+)\\/([0-9]+) passed.*/\\1/')
    total=$(printf '%s\\n' "$result_line" | sed -E 's/^Result: ([0-9]+)\\/([0-9]+) passed.*/\\2/')
    failed=$((total - passed))
  elif printf '%s\\n' "$result_line" | grep -Eq '^Result: [0-9]+ passed'; then
    passed=$(printf '%s\\n' "$result_line" | sed -E 's/^Result: ([0-9]+) passed.*/\\1/')
    total=$passed
    failed=0
  elif printf '%s\\n' "$result_line" | grep -Eq '^Result: [0-9]+ tests'; then
    total=$(printf '%s\\n' "$result_line" | sed -E 's/^Result: ([0-9]+) tests.*/\\1/')
    passed=0
    failed=0
  else
    total=0
    passed=0
    failed=$([ "$status" -eq 0 ] && printf 0 || printf 1)
  fi
fi
if [ "$passed" -lt 0 ]; then passed=0; fi
if [ "$status" -eq 0 ]; then run_status=passed; else run_status=failed; fi
printf '{{"schema":"once.test_results.v1","target":"%s","runner":{{"type":"%s","metadata":{{}}}},"status":"%s","summary":{{"total":%s,"passed":%s,"failed":%s,"skipped":0,"flaky":0}},"cases":[],"artifacts":{{"logs":["%s"],"native_results":["%s"]}}}}\\n' {target} {runner_type} "$run_status" "$total" "$passed" "$failed" "$LOG" "$NATIVE_RESULTS" > "$WORKSPACE_ROOT/$RESULTS"
exit "$status"
""".format(
        build_root = _shell_quote(build_root),
        home = _shell_quote(home),
        deps_root = _shell_quote(deps_root),
        results = _shell_quote(results),
        log = _shell_quote(log),
        native_results = _shell_quote(native_results),
        app_name = app_name,
        lib_ebin = _shell_quote(lib["ebin_dir"]),
        lib_priv = _shell_quote(lib["priv_dir"]),
        dep_symlinks = _elixir_dep_symlink_script(apps, "WORKSPACE_ROOT", "DEPS_ROOT"),
        package = _shell_quote(ctx["label"]["package"] or "."),
        user_env = _elixir_export_env(user_env),
        runner = runner,
        target = _shell_quote(ctx["label"]["id"]),
        runner_type = _shell_quote("mix_test" if mix_config else "elixir_exunit"),
    )

def _elixir_test_impl(ctx):
    srcs = _elixir_test_sources(ctx)
    input_srcs = _elixir_test_input_sources(ctx, srcs)
    config = _elixir_config_inputs(ctx)
    data = _elixir_data_inputs(ctx)
    mix_config = _elixir_mix_config(ctx)
    test_dir = ctx["build_dir"] + "/test"
    results = test_dir + "/test_results.json"
    log = test_dir + "/elixir-test.log"
    native_results = test_dir + "/native_results.txt"
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "elixir_test",
        "affected_inputs": _unique(_elixir_optional_inputs(input_srcs + config + data + [mix_config])),
        "test_info": _elixir_test_info_for(ctx, "elixir", mix_config, srcs, results, log, native_results),
    }
    if ctx["capability"] != "test":
        return provider
    if len(ctx["deps"]) != 1:
        fail(ctx["label"]["id"] + ": elixir_test expects exactly one elixir_library dependency compiled with mix_env = \"test\"")
    lib = ctx["deps"][0]
    if lib.get("target_kind") != "elixir_library":
        fail(ctx["label"]["id"] + ": elixir_test dependency must be an elixir_library target")
    if lib.get("mix_env") != "test":
        fail(ctx["label"]["id"] + ": elixir_test dependency must be compiled with mix_env = \"test\"")
    mix_config = lib.get("mix_config") or mix_config
    toolchain = _elixir_mix_toolchain() if mix_config else _elixir_compiler_toolchain()
    apps = _elixir_collect_apps([lib])
    provider["affected_inputs"] = _unique(_elixir_optional_inputs(input_srcs + config + data + [mix_config]))
    provider["test_info"] = _elixir_test_info_for(ctx, toolchain["mix"] if mix_config else toolchain["elixir"], mix_config, srcs, results, log, native_results)
    run_action(
        argv = ["/bin/sh", "-c", _elixir_test_script(ctx, toolchain, lib, apps, srcs, config, data, mix_config, results, log, native_results)],
        inputs = provider["affected_inputs"],
        outputs = [test_dir, results, log, native_results],
        env = _elixir_action_env(toolchain),
        toolchain_identity = toolchain["identity"] + ("\x00mix_test.v1" if mix_config else "\x00elixir_exunit.v1"),
        identifier = ctx["label"]["id"] + (":mix-test" if mix_config else ":elixir-test"),
    )
    return provider

_ELIXIR_LIBRARY_ATTRS = [
    attr("app_name", "string", docs = "Elixir application name. Defaults to the target name with `-` and `.` rewritten as `_`.", configurable = False),
    attr("mix_env", "string", default = "prod", docs = "Mix environment exported while compiling and testing, such as `prod`, `test`, or `dev`.", configurable = False),
    attr("mix_config", "string", default = "", docs = "Optional package-relative Mix project file included in the library action key when the project still needs one.", configurable = False),
    attr("version", "string", default = "0.1.0", docs = "Application version written to the generated application metadata.", configurable = False),
    attr("description", "string", default = "", docs = "Application description written to the generated application metadata.", configurable = False),
    attr("applications", "list<string>", default = "[\"kernel\", \"stdlib\", \"elixir\"]", docs = "Runtime applications written to the generated application metadata.", configurable = False),
    attr("included_applications", "list<string>", default = "[]", docs = "Included applications written to the generated application metadata.", configurable = False),
    attr("registered", "list<string>", default = "[]", docs = "Registered process names written to the generated application metadata.", configurable = False),
    attr("consolidate_protocols", "bool", default = "true", docs = "Consolidate protocols after compilation and stage consolidated protocol bytecode into the application.", configurable = False),
    attr("config", "list<string>", default = "[\"config/**/*.exs\"]", docs = "Package-relative config file globs included in the compile action key.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative data file globs available during compile.", configurable = False),
    attr("priv", "list<string>", default = "[]", docs = "Package-relative priv file globs copied into the application priv output.", configurable = False),
    attr("include", "list<string>", default = "[\"include/**/*.hrl\"]", docs = "Package-relative Erlang header globs included in the compile action key.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables exported before running Elixir compile and test commands.", configurable = False),
    attr("compile_args", "list<string>", default = "[]", docs = "Additional arguments appended to `elixirc`.", configurable = False),
]

_ELIXIR_TEST_ATTRS = [
    attr("mix_config", "string", default = "", docs = "Optional package-relative Mix project file. When omitted, tests run through direct ExUnit without requiring a Mix project.", configurable = False),
    attr("config", "list<string>", default = "[\"config/**/*.exs\"]", docs = "Package-relative config file globs included in the test action key.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative data file globs available during tests.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables exported before running tests.", configurable = False),
    attr("test_args", "list<string>", default = "[]", docs = "Additional arguments appended to the test runner.", configurable = False),
    attr("no_start", "bool", default = "false", docs = "Pass `--no-start` to `mix test` when `mix_config` enables Mix mode.", configurable = False),
    attr("labels", "list<string>", default = "[]", docs = "Labels exposed through once_test_info for test discovery.", configurable = True),
    attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
]

elixir_library = target_kind(
    docs = "Elixir code compiled with one target-level `elixirc` action and staged into cacheable bytecode plus OTP application metadata.",
    attrs = _ELIXIR_LIBRARY_ATTRS,
    deps = [dep("deps", ["elixir_app"], "Elixir applications available on the compile path.")],
    providers = ["elixir_app"],
    capabilities = [capability("build", ["bytecode"])],
    examples = [
        example(
            "elixir-library-minimal",
            name = "Minimal Elixir library",
            use_when = "Start here for a small Elixir application compiled through Once.",
        ),
    ],
    impl = _elixir_library_impl,
)

elixir_test = target_kind(
    docs = "Runs ExUnit tests against an elixir_library already compiled with mix_env = \"test\".",
    attrs = _ELIXIR_TEST_ATTRS,
    deps = [dep("deps", ["elixir_app"], "The test-environment Elixir application under test.")],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    examples = [
        example(
            "elixir-test-minimal",
            name = "Minimal Elixir test",
            use_when = "Use this when an Elixir application should compile once and run ExUnit tests through Once.",
        ),
    ],
    impl = _elixir_test_impl,
)
