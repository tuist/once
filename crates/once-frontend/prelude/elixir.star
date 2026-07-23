def _elixir_attr(ctx, key, default):
    return _configured_attr(ctx, key, default)

def _elixir_host_shell(ctx):
    if host_os() == "windows":
        fail(ctx["label"]["id"] + ": this Elixir action requires a POSIX-compatible shell provided by the configured toolchain")
    return host_which("sh")

def _elixir_app_name(ctx):
    return _elixir_attr(ctx, "app_name", ctx["label"]["name"].replace("-", "_").replace(".", "_"))

def _elixir_description(ctx):
    description = _elixir_attr(ctx, "description", "")
    if description:
        return description
    return _elixir_attr(ctx, "app_description", "")

def _elixir_applications(ctx):
    return _unique(_elixir_attr(ctx, "applications", ["kernel", "stdlib", "elixir"]) + _elixir_attr(ctx, "extra_apps", []))

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
    return _file_globs(_elixir_attr(ctx, key, default))

def _elixir_data_inputs(ctx):
    return _elixir_glob_attr(ctx, "data", [])

def _elixir_config_inputs(ctx):
    return _unique(_elixir_glob_attr(ctx, "config", ["config/**/*.exs"]) + _elixir_glob_attr(ctx, "config_files", []))

def _elixir_priv_inputs(ctx):
    return _unique(_elixir_glob_attr(ctx, "priv", []) + _elixir_glob_attr(ctx, "resources", []))

def _elixir_include_inputs(ctx):
    return _elixir_glob_attr(ctx, "include", ["include/**/*.hrl"])

def _elixir_doc_inputs(ctx):
    return _elixir_glob_attr(ctx, "docs", [])

def _elixir_tool_inputs(ctx):
    return _elixir_glob_attr(ctx, "tools", [])

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
    for key, value in _elixir_attr(ctx, "os_env", {}).items():
        out[key] = value
    for key in _elixir_attr(ctx, "env_inherit", []):
        value = host_env(key)
        if value:
            out[key] = value
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

def _elixir_priv_destination(ctx, src, priv_dir):
    package = ctx["label"]["package"]
    rel = src
    if package and rel.startswith(package + "/"):
        rel = rel[len(package) + 1:]
    if rel.startswith("priv/"):
        rel = rel[len("priv/"):]
    return priv_dir + "/" + rel

def _elixir_erlang_atom(value):
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"

def _elixir_erlang_string(value):
    return "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"") + "\""

def _elixir_erlang_atom_list(values):
    return "[" + ",".join([_elixir_erlang_atom(value) for value in values]) + "]"

def _elixir_app_writer_source():
    return """
defmodule Once.ElixirAppWriter do
  def main do
    ebin_dir = System.fetch_env!("ONCE_EBIN_DIR")
    modules =
      ebin_dir
      |> Path.join("*.beam")
      |> Path.wildcard()
      |> Enum.map(&Path.basename(&1, ".beam"))
      |> Enum.sort()
      |> Enum.map(&("'" <> String.replace(&1, "'", "\\\\'") <> "'"))
      |> Enum.join(",")

    lines = [
      "{application, " <> System.fetch_env!("ONCE_APP_NAME") <> ",",
      " [{description, " <> System.fetch_env!("ONCE_APP_DESCRIPTION") <> "},",
      "  {vsn, " <> System.fetch_env!("ONCE_APP_VERSION") <> "},",
      "  {registered, " <> System.fetch_env!("ONCE_APP_REGISTERED") <> "},",
      "  {applications, " <> System.fetch_env!("ONCE_APP_APPLICATIONS") <> "},",
      "  {included_applications, " <> System.fetch_env!("ONCE_APP_INCLUDED_APPLICATIONS") <> "},",
      "  {modules, [" <> modules <> "]},",
      "  {env, []}]}."
    ]

    File.write!(System.fetch_env!("ONCE_APP_FILE"), Enum.join(lines, "\\n") <> "\\n")
  end
end

Once.ElixirAppWriter.main()
"""

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

def _elixir_compile_args(ctx):
    return _elixir_attr(ctx, "compile_args", []) + _elixir_attr(ctx, "elixirc_opts", [])

def _elixir_test_args(ctx):
    return _elixir_attr(ctx, "test_args", [])

def _elixir_interpreter_opts(ctx):
    return _elixir_attr(ctx, "elixir_opts", [])

def _elixir_validate_test_mode(ctx, mix_config):
    if mix_config and _elixir_interpreter_opts(ctx):
        fail(ctx["label"]["id"] + ": `elixir_opts` applies to direct ExUnit mode only; use `test_args` with `mix_config`")

def _elixir_reject_unsupported_attrs(ctx, keys):
    for key in keys:
        value = _elixir_attr(ctx, key, None)
        if value == None:
            continue
        if type(value) == type("") and value == "":
            continue
        if type(value) == type([]) and len(value) == 0:
            continue
        if type(value) == type({}) and len(value) == 0:
            continue
        if value == False:
            continue
        fail(ctx["label"]["id"] + ": attribute `" + key + "` is declared for Buck or Bazel parity but is not implemented by this target kind yet")

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
    _elixir_reject_unsupported_attrs(ctx, ["app_src", "app_src_vsn", "appup_src", "erl_opts", "ez_deps", "extra_includes", "extra_properties", "include_src", "mod", "shell_configs", "shell_libs"])
    toolchain = _elixir_compiler_toolchain()
    app_name = _elixir_app_name(ctx)
    mix_env = _elixir_mix_env(ctx)
    srcs = _elixir_sources(ctx)
    config = _elixir_config_inputs(ctx)
    data = _elixir_data_inputs(ctx)
    priv_srcs = _elixir_priv_inputs(ctx)
    include = _elixir_include_inputs(ctx)
    docs = _elixir_doc_inputs(ctx)
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
    compile_args = _elixir_compile_args(ctx)
    compile_inputs = _unique(_elixir_optional_inputs(config + data + include + docs + [mix_config]))
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
    copy_path(
        module_dirs,
        ebin_dir,
        kind = "tree",
        toolchain_identity = toolchain["identity"] + "\x00elixir-stage-modules.v2",
        identifier = ctx["label"]["id"] + ":elixir-stage-modules",
    )
    prepare_path(priv_dir, kind = "remove", identifier = ctx["label"]["id"] + ":elixir-stage-priv-clean")
    prepare_path(priv_dir, kind = "directory", identifier = ctx["label"]["id"] + ":elixir-stage-priv")
    for src in priv_srcs:
        copy_path(
            src,
            _elixir_priv_destination(ctx, src, priv_dir),
            inputs = [src],
            toolchain_identity = toolchain["identity"] + "\x00elixir-stage-priv.v2",
            identifier = ctx["label"]["id"] + ":elixir-stage-priv:" + src,
        )
    app_writer = ctx["scratch_dir"] + "/app_writer.exs"
    app_file = ebin_dir + "/" + app_name + ".app"
    write_path(app_writer, _elixir_app_writer_source())
    run_action(
        argv = [toolchain["elixir"], app_writer],
        outputs = [app_file],
        env = _elixir_action_env_with(ctx, toolchain, {
            "ONCE_EBIN_DIR": ebin_dir,
            "ONCE_APP_FILE": app_file,
            "ONCE_APP_NAME": _elixir_erlang_atom(app_name),
            "ONCE_APP_DESCRIPTION": _elixir_erlang_string(_elixir_description(ctx)),
            "ONCE_APP_VERSION": _elixir_erlang_string(_elixir_attr(ctx, "version", "0.1.0")),
            "ONCE_APP_REGISTERED": _elixir_erlang_atom_list(_elixir_attr(ctx, "registered", [])),
            "ONCE_APP_APPLICATIONS": _elixir_erlang_atom_list(_elixir_applications(ctx)),
            "ONCE_APP_INCLUDED_APPLICATIONS": _elixir_erlang_atom_list(_elixir_attr(ctx, "included_applications", [])),
        }),
        toolchain_identity = toolchain["identity"] + "\x00elixir-stage-app.v2\x00mix_env\x00" + mix_env,
        identifier = ctx["label"]["id"] + ":elixir-stage-app",
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
    _elixir_validate_test_mode(ctx, mix_config)
    opts = _elixir_test_args(ctx)
    if mix_config:
        no_start = ["--no-start"] if _elixir_attr(ctx, "no_start", False) else []
        return _shell_quote(toolchain["mix"]) + " test --no-compile --no-deps-check " + _elixir_shell_words(no_start + test_paths + opts)

    start_args = []
    if "test/test_helper.exs" not in test_paths:
        start_args = ["-e", "ExUnit.start()"]
    require_args = []
    for path in test_paths:
        require_args.extend(["-r", path])
    return _shell_quote(toolchain["elixir"]) + " " + _elixir_code_path_flags(apps) + " " + _elixir_shell_words(_elixir_interpreter_opts(ctx) + start_args + require_args + opts)

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
    }

def _elixir_test_info_for(ctx, runner, mix_config, srcs, results, log, native_results):
    test_paths = _elixir_test_paths(ctx, srcs)
    _elixir_validate_test_mode(ctx, mix_config)
    if mix_config:
        return _elixir_test_info(ctx, "mix_test", "Mix test", runner, ["test", "--no-compile", "--no-deps-check"] + test_paths + _elixir_test_args(ctx), results, log, native_results)
    start_args = []
    if "test/test_helper.exs" not in test_paths:
        start_args = ["-e", "ExUnit.start()"]
    require_args = []
    for path in test_paths:
        require_args.extend(["-r", path])
    return _elixir_test_info(ctx, "elixir_exunit", "Elixir ExUnit", runner, _elixir_interpreter_opts(ctx) + start_args + require_args + _elixir_test_args(ctx), results, log, native_results)

def _elixir_test_script(ctx, toolchain, lib, apps, srcs, config, data, mix_config, results, log, native_results):
    app_name = lib["app_name"]
    build_root = ctx["scratch_dir"] + "/test_build"
    home = ctx["scratch_dir"] + "/test_home"
    deps_root = ctx["scratch_dir"] + "/test_deps"
    user_env = _elixir_user_env(ctx)
    setup = _elixir_attr(ctx, "setup", "")
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
{setup}
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
        setup = setup,
        runner = runner,
        target = _shell_quote(ctx["label"]["id"]),
        runner_type = _shell_quote("mix_test" if mix_config else "elixir_exunit"),
    )

def _elixir_test_impl(ctx):
    _elixir_reject_unsupported_attrs(ctx, ["ez_deps"])
    srcs = _elixir_test_sources(ctx)
    input_srcs = _elixir_test_input_sources(ctx, srcs)
    config = _elixir_config_inputs(ctx)
    data = _elixir_data_inputs(ctx)
    tools = _elixir_tool_inputs(ctx)
    mix_config = _elixir_mix_config(ctx)
    test_dir = ctx["build_dir"] + "/test"
    results = test_dir + "/test_results.json"
    log = test_dir + "/elixir-test.log"
    native_results = test_dir + "/native_results.txt"
    provider = {
        "label_id": ctx["label"]["id"],
        "target_kind": "elixir_test",
        "affected_inputs": _unique(_elixir_optional_inputs(input_srcs + config + data + tools + [mix_config])),
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
    provider["affected_inputs"] = _unique(_elixir_optional_inputs(input_srcs + config + data + tools + [mix_config]))
    provider["test_info"] = _elixir_test_info_for(ctx, toolchain["mix"] if mix_config else toolchain["elixir"], mix_config, srcs, results, log, native_results)
    run_action(
        argv = [_elixir_host_shell(ctx), "-c", _elixir_test_script(ctx, toolchain, lib, apps, srcs, config, data, mix_config, results, log, native_results)],
        inputs = provider["affected_inputs"],
        outputs = [test_dir, results, log, native_results],
        env = _elixir_action_env(toolchain),
        toolchain_identity = toolchain["identity"] + ("\x00mix_test.v1" if mix_config else "\x00elixir_exunit.v1"),
        identifier = ctx["label"]["id"] + (":mix-test" if mix_config else ":elixir-test"),
    )
    return provider

def _mix_workspace_path(ctx, path):
    if path.startswith("/"):
        return path
    package = ctx["label"]["package"]
    if package:
        return workspace_root() + "/" + package + "/" + path
    return workspace_root() + "/" + path

def _mix_package_path(ctx, path):
    normalized = path.replace("\\", "/")
    package_root = workspace_root().replace("\\", "/")
    package = ctx["label"]["package"]
    if package:
        package_root += "/" + package
    prefix = package_root + "/"
    if normalized.startswith(prefix):
        return normalized[len(prefix):]
    if not normalized.startswith("/"):
        if normalized == ".." or normalized.startswith("../") or "/../" in normalized:
            fail(ctx["label"]["id"] + ": dependency source `" + normalized + "` is outside the package; vendor Mix dependencies inside the Once package")
        return normalized
    fail(ctx["label"]["id"] + ": dependency source `" + normalized + "` is outside the package; vendor Mix dependencies inside the Once package")

def _mix_trim_slash(path):
    return path[:-1] if len(path) > 1 and path.endswith("/") else path

def _mix_lock_reader_source():
    return """
Mix.start()

defmodule Once.MixLockedSource do
  @behaviour Mix.SCM

  @impl true
  def fetchable?, do: true

  @impl true
  def format(opts), do: opts[:dest] || "locked dependency"

  @impl true
  def format_lock(opts), do: inspect(opts[:lock])

  @impl true
  def accepts_options(_app, opts), do: opts

  @impl true
  def checked_out?(opts), do: File.dir?(opts[:dest])

  @impl true
  def checkout(opts), do: opts[:lock]

  @impl true
  def update(opts), do: opts[:lock]

  @impl true
  def lock_status(opts), do: if(opts[:lock], do: :ok, else: :mismatch)

  @impl true
  def equal?(opts1, opts2), do: opts1[:dest] == opts2[:dest]

  @impl true
  def managers(opts) do
    case opts[:lock] do
      {:hex, _, _, _, managers, _, _, _} when is_list(managers) -> managers
      _ -> []
    end
  end
end

Mix.SCM.append(Once.MixLockedSource)

normalize = fn value, recur ->
  cond do
    is_atom(value) -> Atom.to_string(value)
    is_binary(value) or is_number(value) or is_boolean(value) or is_nil(value) -> value
    is_tuple(value) -> value |> Tuple.to_list() |> Enum.map(&recur.(&1, recur))
    is_list(value) -> Enum.map(value, &recur.(&1, recur))
    is_map(value) -> Map.new(value, fn {key, item} -> {to_string(key), recur.(item, recur)} end)
    true -> inspect(value)
  end
end

[project_file, lockfile, mix_env, package_root] = System.argv()
project_dir = File.cd!(Path.dirname(project_file), fn -> File.cwd!() end)

unless Code.ensure_loaded?(:json) and function_exported?(:json, :encode, 1) do
  raise "Mix dependency resolution requires Erlang 27 or newer for :json.encode/1"
end

payload =
  Mix.Project.in_project(:once_dependency_resolver, project_dir, [env: String.to_atom(mix_env)], fn _project ->
    Mix.Dep.clear_cached()
    lock = Mix.Dep.Lock.read(lockfile)

    dependencies =
      Mix.Dep.load_and_cache()
      |> Enum.map(fn dependency ->
        destination =
          case dependency.opts[:dest] do
            nil -> ""
            path -> Path.relative_to(path, package_root)
          end

        %{
          "app" => Atom.to_string(dependency.app),
          "dependencies" => Enum.map(dependency.deps || [], &Atom.to_string(&1.app)),
          "custom_compile" => dependency.opts[:compile] != nil,
          "destination" => destination,
          "manager" => normalize.(dependency.manager, normalize),
          "path_dependency" => dependency.opts[:path] != nil,
          "top_level" => dependency.top_level == true
        }
      end)

    %{
      "manifest" => File.read!(project_file),
      "lockfile" => File.read!(lockfile),
      "mix_env" => mix_env,
      "lock" => normalize.(lock, normalize),
      "dependencies" => dependencies
    }
  end)

payload |> :json.encode() |> IO.binwrite()
"""

def _mix_read_locked_graph(ctx):
    manifest = ctx["attrs"].get("manifest") or "mix.exs"
    lockfile = ctx["attrs"].get("lockfile") or "mix.lock"
    mix_env = ctx["attrs"].get("mix_env") or "prod"
    if ctx["files"].get(manifest) == None:
        fail(ctx["label"]["id"] + ": `" + manifest + "` must be declared in resolver_inputs, or srcs when resolver_inputs is omitted, so Mix project resolution is content-addressed")
    if ctx["files"].get(lockfile) == None:
        fail(ctx["label"]["id"] + ": `" + lockfile + "` must be declared in resolver_inputs, or srcs when resolver_inputs is omitted; mix.lock is required and authoritative")
    if _basename(manifest) != "mix.exs":
        fail(ctx["label"]["id"] + ": Mix project manifests must be named mix.exs")
    graph_file = ctx["attrs"].get("graph_file") or ""
    if graph_file:
        graph_content = ctx["files"].get(graph_file)
        if graph_content == None:
            fail(ctx["label"]["id"] + ": Mix graph snapshot `" + graph_file + "` must be declared in resolver_inputs, or srcs when resolver_inputs is omitted")
        graph = json_decode(graph_content)
        if graph.get("manifest") == None:
            fail(ctx["label"]["id"] + ": Mix graph snapshot `" + graph_file + "` does not contain its authoritative manifest binding")
        if graph["manifest"] != ctx["files"][manifest]:
            fail(ctx["label"]["id"] + ": Mix graph snapshot `" + graph_file + "` is stale relative to `" + manifest + "`; regenerate it from the committed manifest")
        if graph.get("lockfile") == None:
            fail(ctx["label"]["id"] + ": Mix graph snapshot `" + graph_file + "` does not contain its authoritative lockfile binding")
        if graph["lockfile"] != ctx["files"][lockfile]:
            fail(ctx["label"]["id"] + ": Mix graph snapshot `" + graph_file + "` is stale relative to `" + lockfile + "`; regenerate it from the committed lockfile")
        if graph.get("mix_env") != mix_env:
            fail(ctx["label"]["id"] + ": Mix graph snapshot `" + graph_file + "` was generated for MIX_ENV=" + (graph.get("mix_env") or "<missing>") + ", but this target selects MIX_ENV=" + mix_env + "; regenerate it for the selected environment")
        expected_inputs = _resolver_snapshot_inputs(ctx["files"], [graph_file])
        if graph.get("once_inputs") != expected_inputs:
            fail(ctx["label"]["id"] + ": Mix graph snapshot `" + graph_file + "` input binding does not match resolver_inputs")
        return graph
    toolchain = _elixir_mix_toolchain()
    resolver_home = workspace_root() + "/.once/tmp/mix-resolver"
    env = _elixir_action_env(toolchain)
    env["MIX_HOME"] = resolver_home + "/mix"
    env["MIX_ARCHIVES"] = resolver_home + "/archives"
    env["HEX_HOME"] = resolver_home + "/hex"
    env["MIX_ENV"] = mix_env
    env["HEX_OFFLINE"] = "true"
    content = host_command([
        toolchain["elixir"],
        "-e",
        _mix_lock_reader_source(),
        "--",
        _mix_workspace_path(ctx, manifest),
        _mix_workspace_path(ctx, lockfile),
        mix_env,
        _mix_workspace_path(ctx, "."),
    ], env = env, cwd = _mix_workspace_path(ctx, "."))
    graph = json_decode(content)
    graph["once_inputs"] = _resolver_snapshot_inputs(ctx["files"])
    return graph

def _mix_target_name(app):
    return "mix-" + app.replace("_", "-").replace(".", "-").replace("+", "-")

def _mix_lock_entry(lock, app):
    entry = lock.get(app)
    if entry == None:
        fail("Mix selected dependency `" + app + "`, but mix.lock has no entry for it")
    if type(entry) != type([]) or len(entry) == 0:
        fail("mix.lock entry `" + app + "` has an unsupported shape")
    return entry

def _mix_lock_value(entry, index, default = ""):
    if len(entry) <= index or entry[index] == None:
        return default
    return entry[index]

def _mix_locked_identity(app, entry):
    kind = _mix_lock_value(entry, 0)
    if kind == "hex":
        package_name = _mix_lock_value(entry, 1, app)
        version = _mix_lock_value(entry, 2)
        checksum = _mix_lock_value(entry, 3)
        managers = _mix_lock_value(entry, 4, [])
        repository = _mix_lock_value(entry, 6, "hexpm")
        outer_checksum = _mix_lock_value(entry, 7)
        return {
            "package_name": package_name,
            "version": version,
            "source": "hex:" + repository + "/" + package_name,
            "checksum": checksum,
            "outer_checksum": outer_checksum,
            "revision": "",
            "repository": repository,
            "managers": managers,
        }
    if kind == "git":
        repository = _mix_lock_value(entry, 1)
        revision = _mix_lock_value(entry, 2)
        return {
            "package_name": app,
            "version": revision,
            "source": "git:" + repository,
            "checksum": "",
            "outer_checksum": "",
            "revision": revision,
            "repository": repository,
            "managers": [],
        }
    fail("mix.lock entry `" + app + "` uses unsupported source `" + kind + "`; supported locked sources are Hex and Git")

def _mix_dependency_index(dependencies):
    index = {}
    for dependency in dependencies:
        app = dependency.get("app") or ""
        if app:
            index[app] = dependency
    return index

def _mix_source_globs(source_root):
    return [
        source_root + "/mix.exs",
        source_root + "/**/*",
    ]

def _mix_resolved_roots(ctx, dependencies, targets_by_app, identities):
    requested = ctx["attrs"].get("roots") or []
    if requested:
        roots = []
        for requested_name in requested:
            target = targets_by_app.get(requested_name)
            if target == None:
                for app, identity in identities.items():
                    if identity["package_name"] == requested_name:
                        target = targets_by_app.get(app)
                        break
            if target == None:
                fail(ctx["label"]["id"] + ": requested Mix dependency root `" + requested_name + "` is not present in the locked graph")
            roots.append(target)
        return _unique(roots)
    roots = []
    for dependency in dependencies:
        if dependency.get("top_level"):
            roots.append(targets_by_app[dependency["app"]])
    if roots:
        return _unique(roots)
    return [targets_by_app[app] for app in sorted(targets_by_app.keys())]

def _mix_dependencies_resolver(ctx):
    resolved = _mix_read_locked_graph(ctx)
    lock = resolved.get("lock") or {}
    all_dependencies = resolved.get("dependencies") or []
    path_dependency_targets = ctx["attrs"].get("path_dependencies") or {}
    active_path_dependencies = {}
    for dependency in all_dependencies:
        if dependency.get("path_dependency"):
            app = dependency.get("app") or ""
            active_path_dependencies[app] = True
            if not path_dependency_targets.get(app):
                fail(ctx["label"]["id"] + ": Mix path dependency `" + app + "` must be mapped to a first-party target through path_dependencies")
    for app in path_dependency_targets.keys():
        if not active_path_dependencies.get(app):
            fail(ctx["label"]["id"] + ": path_dependencies maps `" + app + "`, but that application is not an active Mix path dependency")
    dependencies = [dependency for dependency in all_dependencies if not dependency.get("path_dependency")]
    dependency_index = _mix_dependency_index(dependencies)
    vendor_dir = _mix_trim_slash(ctx["attrs"].get("vendor_dir") or "deps")
    manifest = ctx["attrs"].get("manifest") or "mix.exs"
    lockfile = ctx["attrs"].get("lockfile") or "mix.lock"
    mix_env = ctx["attrs"].get("mix_env") or "prod"
    targets = []
    targets_by_app = {}
    identities = {}
    for app in active_path_dependencies.keys():
        targets_by_app[app] = path_dependency_targets[app]
    for app in sorted(dependency_index.keys()):
        dependency = dependency_index[app]
        identity = _mix_locked_identity(app, _mix_lock_entry(lock, app))
        targets_by_app[app] = "./" + _mix_target_name(app)
        identities[app] = identity
    for app in sorted(dependency_index.keys()):
        dependency = dependency_index[app]
        identity = identities[app]
        source_root = dependency.get("destination") or (vendor_dir + "/" + app)
        source_root = _mix_trim_slash(_mix_package_path(ctx, source_root))
        target_name = _mix_target_name(app)
        managers = identity["managers"]
        if not managers and dependency.get("manager"):
            managers = [dependency["manager"]]
        attrs = {
            "app_name": app,
            "mix_env": mix_env,
            "version": identity["version"],
            "config": [],
            "data": [source_root + "/**/*"],
            "priv": [source_root + "/priv/**"],
            "include": [source_root + "/include/**/*.hrl"],
            "consolidate_protocols": False,
            "_mix_locked": True,
            "_mix_package_name": identity["package_name"],
            "_mix_source": identity["source"],
            "_mix_checksum": identity["checksum"],
            "_mix_outer_checksum": identity["outer_checksum"],
            "_mix_revision": identity["revision"],
            "_mix_repository": identity["repository"],
            "_mix_managers": managers,
            "_mix_custom_compile": dependency.get("custom_compile") or False,
            "_mix_source_root": source_root,
            "_mix_manifest": manifest,
            "_mix_lockfile": lockfile,
        }
        dependency_targets = []
        for child in dependency.get("dependencies") or []:
            target_ref = targets_by_app.get(child)
            if target_ref == None:
                fail(ctx["label"]["id"] + ": Mix dependency `" + app + "` references `" + child + "`, but the active graph contains no target for it")
            dependency_targets.append(target_ref)
        targets.append({
            "name": target_name,
            "kind": "mix_package",
            "deps": dependency_targets,
            "srcs": _mix_source_globs(source_root),
            "attrs": attrs,
        })
    roots = _mix_resolved_roots(ctx, all_dependencies, targets_by_app, identities)
    return {
        "targets": targets,
        "roots": roots,
        "attrs": {
            "_mix_resolved": True,
            "_mix_locked_roots": roots,
        },
    }

def _mix_compile_source():
    return """
Mix.start()

[project_dir, app_name, mix_env, dependency_paths_file] = System.argv()
System.put_env("MIX_BUILD_ROOT", System.fetch_env!("MIX_BUILD_ROOT") |> Path.expand())

dependency_paths_file
|> File.read!()
|> String.split("\n", trim: true)
|> Enum.map(&Path.expand/1)
|> Enum.reverse()
|> Enum.each(&Code.prepend_path/1)

result =
  Mix.Project.in_project(String.to_atom(app_name), Path.expand(project_dir), [env: String.to_atom(mix_env)], fn _project ->
    Mix.Task.run("compile.all", [
      "--force",
      "--from-mix-deps-compile",
      "--no-prune-code-paths",
      "--no-optional-deps",
      "--return-errors"
    ])
  end)

case result do
  {:error, diagnostics} ->
    Enum.each(diagnostics, &Code.print_diagnostic/1)
    System.halt(1)

  {_status, diagnostics} ->
    Enum.each(diagnostics, &Code.print_diagnostic/1)

  _ ->
    :ok
end
"""

def _mix_supported_manager(ctx):
    managers = _elixir_attr(ctx, "_mix_managers", [])
    if _elixir_attr(ctx, "_mix_custom_compile", False):
        fail(ctx["label"]["id"] + ": custom Mix dependency compile commands are not supported; model this package with a dedicated target kind")
    if managers != ["mix"]:
        rendered = ", ".join(managers) if managers else "unknown"
        fail(ctx["label"]["id"] + ": Mix dependency build manager `" + rendered + "` is not supported; Rebar and Make packages require dedicated target kinds")
    return managers[0]

def _mix_locked_package_record(provider):
    return {
        "app_name": provider.get("app_name") or "",
        "package_name": provider.get("package_name") or "",
        "version": provider.get("version") or "",
        "source": provider.get("source") or "",
        "checksum": provider.get("checksum") or "",
        "outer_checksum": provider.get("outer_checksum") or "",
        "revision": provider.get("revision") or "",
    }

def _mix_collect_locked_packages(deps, own = None):
    seen = {}
    packages = []
    for dependency in deps:
        transitive = dependency.get("transitive_locked_packages") or []
        if not transitive and dependency.get("locked"):
            transitive = [_mix_locked_package_record(dependency)]
        for package in transitive:
            key = (package.get("app_name") or "") + "\x00" + (package.get("source") or "") + "\x00" + (package.get("version") or "")
            if key and key not in seen:
                seen[key] = True
                packages.append(package)
    if own != None:
        key = (own.get("app_name") or "") + "\x00" + (own.get("source") or "") + "\x00" + (own.get("version") or "")
        if key and key not in seen:
            packages.append(own)
    return packages

def _mix_package_impl(ctx):
    if not _elixir_attr(ctx, "_mix_locked", False):
        fail(ctx["label"]["id"] + ": mix_package targets are generated by mix_dependencies and cannot be declared without locked metadata")
    _mix_supported_manager(ctx)
    toolchain = _elixir_mix_toolchain()
    app_name = _elixir_app_name(ctx)
    mix_env = _elixir_mix_env(ctx)
    source_root = _elixir_attr(ctx, "_mix_source_root", "")
    if not source_root:
        fail(ctx["label"]["id"] + ": locked Mix package is missing its vendored source root")
    apps = _elixir_collect_apps(ctx["deps"])
    dep_code_paths = _elixir_app_code_paths(apps)
    build_root = ctx["build_dir"] + "/mix"
    app_dir = build_root + "/" + mix_env + "/lib/" + app_name
    ebin_dir = app_dir + "/ebin"
    priv_dir = app_dir + "/priv"
    home_dir = ctx["scratch_dir"] + "/mix_home"
    script_dir = ctx["scratch_dir"] + "/mix_compile"
    dependency_paths_file = script_dir + "/dependency_paths.list"
    compile_script = script_dir + "/compile.exs"
    compile_log = ctx["build_dir"] + "/mix-compile.log"
    compile_warnings = ctx["build_dir"] + "/mix-compile-warnings.log"
    srcs = glob(ctx["srcs"])
    root_manifest = _package_relative(ctx, _elixir_attr(ctx, "_mix_manifest", ""))
    lockfile = _package_relative(ctx, _elixir_attr(ctx, "_mix_lockfile", ""))
    inputs = _unique(_elixir_optional_inputs(srcs + [root_manifest, lockfile, dependency_paths_file, compile_script]))
    write_path(dependency_paths_file, _elixir_lines(dep_code_paths))
    write_path(compile_script, _mix_compile_source())
    run_action(
        argv = [
            toolchain["elixir"],
            _elixir_from_package(ctx, compile_script),
            _elixir_from_package(ctx, source_root),
            app_name,
            mix_env,
            _elixir_from_package(ctx, dependency_paths_file),
        ],
        inputs = inputs,
        outputs = [ebin_dir, compile_log, compile_warnings],
        clean_paths = [build_root, home_dir, compile_log, compile_warnings],
        create_dirs = [home_dir],
        cwd = _elixir_package_cwd(ctx),
        env = _elixir_action_env_with(ctx, toolchain, {
            "MIX_HOME": _elixir_from_package(ctx, home_dir + "/mix"),
            "HEX_HOME": _elixir_from_package(ctx, home_dir + "/hex"),
            "MIX_BUILD_ROOT": _elixir_from_package(ctx, build_root),
            "MIX_ENV": mix_env,
            "HEX_OFFLINE": "true",
        }),
        stdout = compile_log,
        stderr = compile_warnings,
        toolchain_identity = toolchain["identity"] + "\x00mix-package-compile.v1\x00" + mix_env,
        identifier = ctx["label"]["id"] + ":mix-compile",
    )
    prepare_path(priv_dir, kind = "remove", identifier = ctx["label"]["id"] + ":mix-priv-clean")
    source_priv = _package_relative(ctx, source_root + "/priv")
    if host_file_exists(_mix_workspace_path(ctx, source_root + "/priv")):
        priv_inputs = glob([source_root + "/priv/**"])
        copy_path(
            source_priv,
            priv_dir,
            kind = "tree",
            inputs = priv_inputs,
            toolchain_identity = toolchain["identity"] + "\x00mix-package-priv.v1",
            identifier = ctx["label"]["id"] + ":mix-priv",
        )
    else:
        prepare_path(priv_dir, kind = "directory", identifier = ctx["label"]["id"] + ":mix-priv")
    app = {
        "app_name": app_name,
        "mix_env": mix_env,
        "mix_config": source_root + "/mix.exs",
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "compile_metadata": "",
        "compile_warnings": compile_warnings,
    }
    package_record = {
        "app_name": app_name,
        "package_name": _elixir_attr(ctx, "_mix_package_name", app_name),
        "version": _elixir_attr(ctx, "version", ""),
        "source": _elixir_attr(ctx, "_mix_source", ""),
        "checksum": _elixir_attr(ctx, "_mix_checksum", ""),
        "outer_checksum": _elixir_attr(ctx, "_mix_outer_checksum", ""),
        "revision": _elixir_attr(ctx, "_mix_revision", ""),
    }
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "mix_package",
        "locked": True,
        "app_name": app_name,
        "package_name": package_record["package_name"],
        "version": package_record["version"],
        "source": package_record["source"],
        "checksum": package_record["checksum"],
        "outer_checksum": package_record["outer_checksum"],
        "revision": package_record["revision"],
        "repository": _elixir_attr(ctx, "_mix_repository", ""),
        "managers": _elixir_attr(ctx, "_mix_managers", []),
        "mix_env": mix_env,
        "mix_config": source_root + "/mix.exs",
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "compile_metadata": "",
        "compile_warnings": compile_warnings,
        "sources": srcs,
        "transitive_sources": _elixir_collect_transitive_strings(ctx["deps"], "transitive_sources", srcs),
        "transitive_elixir_apps": _elixir_transitive_apps(ctx["deps"], app),
        "transitive_locked_packages": _mix_collect_locked_packages(ctx["deps"], package_record),
    }

def _mix_dependencies_impl(ctx):
    apps = _elixir_collect_apps(ctx["deps"])
    packages = _mix_collect_locked_packages(ctx["deps"])
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "mix_dependencies",
        "dependency_set": True,
        "locked": _elixir_attr(ctx, "_mix_resolved", False),
        "locked_roots": _elixir_attr(ctx, "_mix_locked_roots", []),
        "locked_packages": packages,
        "transitive_sources": _elixir_collect_transitive_strings(ctx["deps"], "transitive_sources", []),
        "transitive_elixir_apps": apps,
    }

_ELIXIR_LIBRARY_ATTRS = [
    attr("app_name", "string", docs = "Elixir application name. Defaults to the target name with `-` and `.` rewritten as `_`.", configurable = False),
    attr("mix_env", "string", default = "prod", docs = "Mix environment exported while compiling and testing, such as `prod`, `test`, or `dev`.", configurable = False),
    attr("mix_config", "string", default = "", docs = "Optional package-relative Mix project file included in the library action key when the project still needs one.", configurable = False),
    attr("version", "string", default = "0.1.0", docs = "Application version written to the generated application metadata.", configurable = False),
    attr("description", "string", default = "", docs = "Application description written to the generated application metadata.", configurable = False),
    attr("app_description", "string", default = "", docs = "Bazel-compatible alias used when `description` is omitted.", configurable = False),
    attr("applications", "list<string>", default = "[\"kernel\", \"stdlib\", \"elixir\"]", docs = "Runtime applications written to the generated application metadata.", configurable = False),
    attr("extra_apps", "list<string>", default = "[]", docs = "Bazel-compatible runtime applications appended to `applications`.", configurable = False),
    attr("included_applications", "list<string>", default = "[]", docs = "Included applications written to the generated application metadata.", configurable = False),
    attr("registered", "list<string>", default = "[]", docs = "Registered process names written to the generated application metadata.", configurable = False),
    attr("consolidate_protocols", "bool", default = "true", docs = "Consolidate protocols after compilation and stage consolidated protocol bytecode into the application.", configurable = False),
    attr("config", "list<string>", default = "[\"config/**/*.exs\"]", docs = "Package-relative config file globs included in the compile action key.", configurable = False),
    attr("config_files", "list<string>", default = "[]", docs = "Buck-compatible alias for additional config file globs.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative data file globs available during compile.", configurable = False),
    attr("priv", "list<string>", default = "[]", docs = "Package-relative priv file globs copied into the application priv output.", configurable = False),
    attr("resources", "list<string>", default = "[]", docs = "Buck-compatible resource globs copied into the application priv output.", configurable = False),
    attr("include", "list<string>", default = "[\"include/**/*.hrl\"]", docs = "Package-relative Erlang header globs included in the compile action key.", configurable = False),
    attr("docs", "list<string>", default = "[]", docs = "Buck-compatible documentation file globs included in the compile action key.", configurable = False),
    attr("os_env", "map<string, string>", default = "{}", docs = "Buck-compatible environment variables exported before Elixir compile and test commands.", configurable = False),
    attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited before explicit `env` values.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables exported before running Elixir compile and test commands.", configurable = False),
    attr("compile_args", "list<string>", default = "[]", docs = "Additional arguments appended to `elixirc`.", configurable = False),
    attr("elixirc_opts", "list<string>", default = "[]", docs = "Bazel-compatible alias for additional `elixirc` arguments.", configurable = False),
    attr("app_src", "string", docs = "Reserved for Buck-compatible application source file generation.", configurable = False, implemented = False),
    attr("app_src_vsn", "string", docs = "Reserved for Buck-compatible application source versions.", configurable = False, implemented = False),
    attr("appup_src", "string", docs = "Reserved for Buck-compatible upgrade files.", configurable = False, implemented = False),
    attr("erl_opts", "list<string>", default = "[]", docs = "Reserved for Erlang compiler options.", configurable = False, implemented = False),
    attr("ez_deps", "list<string>", default = "[]", docs = "Reserved for Bazel-compatible archive dependencies.", configurable = False, implemented = False),
    attr("extra_includes", "list<string>", default = "[]", docs = "Reserved for Buck-compatible include directories.", configurable = False, implemented = False),
    attr("extra_properties", "map<string, string>", default = "{}", docs = "Reserved for extra application metadata properties.", configurable = False, implemented = False),
    attr("include_src", "bool", default = "false", docs = "Reserved for Buck-compatible source inclusion.", configurable = False, implemented = False),
    attr("mod", "string", docs = "Reserved for application callback module metadata.", configurable = False, implemented = False),
    attr("shell_configs", "list<string>", default = "[]", docs = "Reserved for Buck-compatible shell configuration files.", configurable = False, implemented = False),
    attr("shell_libs", "list<string>", default = "[]", docs = "Reserved for Buck-compatible shell libraries.", configurable = False, implemented = False),
]

_ELIXIR_TEST_ATTRS = [
    attr("mix_config", "string", default = "", docs = "Optional package-relative Mix project file. When omitted, tests run through direct ExUnit without requiring a Mix project.", configurable = False),
    attr("config", "list<string>", default = "[\"config/**/*.exs\"]", docs = "Package-relative config file globs included in the test action key.", configurable = False),
    attr("config_files", "list<string>", default = "[]", docs = "Buck-compatible alias for additional config file globs.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative data file globs available during tests.", configurable = False),
    attr("os_env", "map<string, string>", default = "{}", docs = "Buck-compatible environment variables exported before running tests.", configurable = False),
    attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited before explicit `env` values.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables exported before running tests.", configurable = False),
    attr("test_args", "list<string>", default = "[]", docs = "Additional arguments appended to the test runner.", configurable = False),
    attr("elixir_opts", "list<string>", default = "[]", docs = "Bazel-compatible options passed to the direct Elixir interpreter before test files. Not supported with `mix_config`.", configurable = False),
    attr("setup", "string", default = "", docs = "Shell snippet run after the test environment is prepared and before ExUnit starts.", configurable = False),
    attr("no_start", "bool", default = "false", docs = "Pass `--no-start` to `mix test` when `mix_config` enables Mix mode.", configurable = False),
    attr("ez_deps", "list<string>", default = "[]", docs = "Reserved for Bazel-compatible archive dependencies.", configurable = False, implemented = False),
    attr("tools", "list<string>", default = "[]", docs = "Package-relative executable or support-file globs available to the test setup command.", configurable = False),
    attr("labels", "list<string>", default = "[]", docs = "Labels exposed through once_test_info for test discovery.", configurable = True),
    attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
]

_MIX_DEPENDENCIES_ATTRS = [
    attr("manifest", "string", default = "mix.exs", docs = "Package-relative Mix project manifest used to select the active dependency graph.", configurable = False),
    attr("lockfile", "string", default = "mix.lock", docs = "Package-relative Mix lockfile. Resolution fails when the file is absent, and never updates it.", configurable = False),
    attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to the resolver. Defaults to srcs when empty or omitted; set this when build sources include binary or large files.", configurable = False),
    attr("graph_file", "string", docs = "Optional checked-in JavaScript Object Notation snapshot of the active Mix dependency graph with exact manifest, lockfile, Mix environment, and resolver input bindings. When present, graph loading does not execute Mix.", configurable = False),
    attr("vendor_dir", "string", default = "deps", docs = "Package-relative directory containing dependency source trees already fetched from Hex or Git.", configurable = False),
    attr("path_dependencies", "map<string, string>", default = "{}", docs = "Map active Mix path dependency application names to first-party Once target references.", configurable = False),
    attr("mix_env", "string", default = "prod", docs = "Mix environment used when evaluating dependency declarations.", configurable = False),
    attr("roots", "list<string>", default = "[]", docs = "Optional application or Hex package names exposed as direct roots. Defaults to direct dependencies selected by Mix.", configurable = False),
    attr("_mix_resolved", "bool", default = "false", docs = "Resolver-owned marker proving the dependency set came from mix.lock.", configurable = False),
    attr("_mix_locked_roots", "list<string>", default = "[]", docs = "Resolver-owned synthetic target names attached to the aggregate provider.", configurable = False),
]

_MIX_PACKAGE_ATTRS = _ELIXIR_LIBRARY_ATTRS + [
    attr("_mix_locked", "bool", default = "false", docs = "Resolver-owned marker proving the synthetic target has locked identity metadata.", configurable = False),
    attr("_mix_package_name", "string", default = "", docs = "Resolver-owned registry package name.", configurable = False),
    attr("_mix_source", "string", default = "", docs = "Resolver-owned stable source identity from mix.lock.", configurable = False),
    attr("_mix_checksum", "string", default = "", docs = "Resolver-owned Hex package checksum from mix.lock.", configurable = False),
    attr("_mix_outer_checksum", "string", default = "", docs = "Resolver-owned Hex registry checksum from mix.lock.", configurable = False),
    attr("_mix_revision", "string", default = "", docs = "Resolver-owned Git revision from mix.lock.", configurable = False),
    attr("_mix_repository", "string", default = "", docs = "Resolver-owned Hex repository or Git repository identity.", configurable = False),
    attr("_mix_managers", "list<string>", default = "[]", docs = "Resolver-owned native build manager metadata.", configurable = False),
    attr("_mix_custom_compile", "bool", default = "false", docs = "Resolver-owned marker for dependency declarations that override Mix compilation.", configurable = False),
    attr("_mix_source_root", "string", default = "", docs = "Resolver-owned package-relative vendored source root.", configurable = False),
    attr("_mix_manifest", "string", default = "", docs = "Resolver-owned root Mix manifest path.", configurable = False),
    attr("_mix_lockfile", "string", default = "", docs = "Resolver-owned authoritative lockfile path.", configurable = False),
]

mix_dependencies = target_kind(
    docs = "Locked Mix dependency graph consumed by Elixir targets. Mix evaluates the project in the selected environment, mix.lock supplies immutable package identities, and Once emits one cacheable target per vendored package.",
    attrs = _MIX_DEPENDENCIES_ATTRS,
    deps = [dep("deps", ["elixir_app"], "Locked package roots generated from the active Mix dependency graph.")],
    providers = ["elixir_app", "mix_dependency_set"],
    capabilities = [capability("build", [])],
    examples = [
        example(
            "elixir-library-with-mix-dependency",
            name = "Elixir library with a locked Mix dependency",
            use_when = "Use this when Elixir code imports vendored packages selected by mix.exs and pinned by mix.lock.",
        ),
    ],
    source_references = [
        source_reference(
            "Mix",
            "mix deps",
            "https://hexdocs.pm/mix/Mix.Tasks.Deps.html",
            "Use this for the native dependency declaration, environment selection, and graph semantics.",
        ),
        source_reference(
            "Hex",
            "locked dependency fetching",
            "https://hex.pm/docs/usage",
            "Use this for Hex lockfile repeatability and registry package behavior.",
        ),
    ],
    resolver = _mix_dependencies_resolver,
    impl = _mix_dependencies_impl,
)

mix_package = target_kind(
    docs = "Synthetic vendored Mix package emitted by mix_dependencies with source, version, checksums, and dependency edges preserved from mix.lock and Mix.",
    attrs = _MIX_PACKAGE_ATTRS,
    deps = [dep("deps", ["elixir_app"], "Locked Mix packages available on the compile path.")],
    providers = ["elixir_app"],
    capabilities = [capability("build", ["bytecode"])],
    examples = [
        example(
            "elixir-library-with-mix-dependency",
            name = "Generated locked Mix package",
            use_when = "Inspect how mix_dependencies lowers a vendored package into the Once build graph.",
        ),
    ],
    source_references = [
        source_reference(
            "Mix",
            "mix compile",
            "https://hexdocs.pm/mix/Mix.Tasks.Compile.html",
            "Use this for native compiler selection and Mix project compilation behavior.",
        ),
    ],
    impl = _mix_package_impl,
)

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
