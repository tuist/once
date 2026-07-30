_ELIXIR_TOOLS = [
    tool("elixir", executables = ["elixir", "elixirc", "mix"]),
    tool("erlang", executables = ["erl"]),
]

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
    if not path:
        return path
    rel = _elixir_workspace_root_rel(ctx)
    if rel == ".":
        return path
    return rel + "/" + path

def _elixir_package_cwd(ctx):
    return ctx["label"]["package"] or None

def _elixir_from_workspace_cwd(ctx, cwd, path):
    workspace_cwd = _package_relative(ctx, cwd)
    parents = []
    for component in workspace_cwd.split("/"):
        if component:
            parents.append("..")
    return "/".join(parents + [path]) if parents else path

def _elixir_from_action_cwd(ctx, cwd, path):
    if cwd:
        return _elixir_from_workspace_cwd(ctx, cwd, path)
    return _elixir_from_package(ctx, path)

def _elixir_is_workspace_input(path):
    if not path:
        return False
    if path.startswith("/") or (len(path) > 1 and path[1] == ":") or path == ".." or path.startswith("../") or "/../" in path:
        return False
    return True

def _elixir_workspace_source_root(package, relative):
    normalized = relative.replace("\\", "/")
    root = workspace_root().replace("\\", "/")
    base_package = package
    if normalized == root:
        normalized = "."
        base_package = ""
    elif normalized.startswith(root + "/"):
        normalized = normalized[len(root) + 1:]
        base_package = ""
    elif normalized.startswith("/") or (len(normalized) > 1 and normalized[1] == ":"):
        fail("Elixir source root `" + normalized + "` escapes the Once workspace")
    parts = []
    for component in (base_package + "/" + normalized).split("/"):
        if not component or component == ".":
            continue
        if component == "..":
            if not parts:
                fail("Elixir source root `" + normalized + "` escapes the Once workspace")
            parts.pop()
        else:
            parts.append(component)
    return "/".join(parts) if parts else "."

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
    mix_home_info = host_command([
        compiler["elixir"],
        "-e",
        """Mix.start()
mix_home = System.get_env("MIX_HOME") || Path.dirname(Mix.path_for(:archives))
hex_version =
  case Application.load(:hex) do
    :ok -> Application.spec(:hex, :vsn) |> to_string()
    {:error, {:already_loaded, :hex}} -> Application.spec(:hex, :vsn) |> to_string()
    _ -> "unavailable"
  end
IO.write(mix_home <> "\\n" <> hex_version)
""",
    ]).strip().split("\n")
    mix_home = mix_home_info[0]
    hex_version = mix_home_info[1] if len(mix_home_info) > 1 else "unavailable"
    identity = compiler["identity"] + "\x00mix\x00" + mix + "\x00" + mix_version + "\x00mix_home\x00" + mix_home + "\x00hex\x00" + hex_version
    return {
        "elixir": compiler["elixir"],
        "elixirc": compiler["elixirc"],
        "mix": mix,
        "erl": compiler["erl"],
        "version": compiler["version"],
        "elixirc_version": compiler["elixirc_version"],
        "mix_version": mix_version,
        "mix_home": mix_home,
        "hex_version": hex_version,
        "path": compiler["path"],
        "identity": identity,
    }

def _elixir_rebar_toolchain():
    compiler = _elixir_compiler_toolchain()
    requested = host_env("MIX_REBAR3")
    rebar3 = requested
    if not rebar3:
        rebar3 = host_command([
            compiler["elixir"],
            "-e",
            "Mix.start(); IO.write(Mix.Rebar.local_rebar_path(:rebar3))",
        ]).strip()
    if not host_file_exists(rebar3):
        fail("Rebar 3 is required for locked Rebar packages; install it with `mix local.rebar --force` or set MIX_REBAR3")
    rebar_version = host_command([rebar3, "version"], merge_stderr = True).strip()
    return {
        "elixir": compiler["elixir"],
        "elixirc": compiler["elixirc"],
        "erl": compiler["erl"],
        "rebar3": rebar3,
        "version": compiler["version"],
        "elixirc_version": compiler["elixirc_version"],
        "rebar_version": rebar_version,
        "path": compiler["path"],
        "identity": compiler["identity"] + "\x00rebar3\x00" + rebar3 + "\x00" + rebar_version,
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

def _elixir_run_action_env_with(ctx, toolchain, extra):
    env = _elixir_action_env(toolchain)
    for key, value in extra.items():
        env[key] = value
    if not _elixir_attr(ctx, "run_cacheable", False):
        host_path = host_env("PATH")
        if host_path:
            separator = ";" if host_os() == "windows" else ":"
            env["PATH"] = toolchain["path"] + separator + host_path
        host_home = host_env("HOME")
        if not host_home and host_os() == "windows":
            host_home = host_env("USERPROFILE")
        if host_home:
            env["HOME"] = host_home
    for key, value in _elixir_user_env(ctx).items():
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

def _elixir_collect_path_dependency_destinations(deps):
    destinations = {}
    for dep in deps:
        for app_name, source_root in dep.get("path_dependency_destinations", {}).items():
            previous = destinations.get(app_name)
            if previous and previous != source_root:
                fail("Mix path dependency `" + app_name + "` resolves to both `" + previous + "` and `" + source_root + "`")
            destinations[app_name] = source_root
    return destinations

def _elixir_app_inputs(apps):
    inputs = []
    for app in apps:
        build_env_dir = app.get("build_env_dir", "")
        if build_env_dir:
            inputs.append(build_env_dir)
        else:
            ebin_dir = app.get("ebin_dir", "")
            if ebin_dir:
                inputs.append(ebin_dir)
            if app.get("priv_inputs", []):
                inputs.append(app.get("priv_dir", ""))
            if app.get("include_inputs", []):
                inputs.append(app.get("include_dir", ""))
    return _unique(inputs)

def _elixir_app_source_inputs(apps):
    inputs = []
    for app in apps:
        inputs.extend(app.get("source_inputs", []))
    return _unique(inputs)

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

def _elixir_include_destination(ctx, src, include_dir):
    package = ctx["label"]["package"]
    rel = src
    if package and rel.startswith(package + "/"):
        rel = rel[len(package) + 1:]
    if rel.startswith("include/"):
        rel = rel[len("include/"):]
    return include_dir + "/" + rel

def _elixir_erlang_atom(value):
    return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"

def _elixir_erlang_string(value):
    return "\"" + value.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t") + "\""

def _elixir_string_list(values):
    return "[" + ",".join([_elixir_erlang_string(value) for value in values]) + "]"

def _elixir_command_plan(commands):
    for index, command in enumerate(commands):
        if not command:
            fail("command at index " + str(index) + " must contain a Mix task name")
        for argument in command:
            if "\x00" in argument:
                fail("command at index " + str(index) + " contains a NUL byte, which cannot be passed to a process")
    return "[" + ",".join([_elixir_string_list(command) for command in commands]) + "]"

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
    write_path(dep_code_paths_list, _elixir_lines([_elixir_from_package(ctx, path) for path in dep_code_paths]))
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
        inputs = _unique(compile_inputs + _elixir_app_inputs(apps)),
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
    include_dir = declare_output("include")
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
    app_writer = ctx["scratch_dir"] + "/app_writer.exs"
    app_file = ebin_dir + "/" + app_name + ".app"
    write_path(app_writer, _elixir_app_writer_source())
    run_action(
        argv = [toolchain["elixir"], app_writer],
        outputs = [app_file, priv_dir, include_dir],
        clean_paths = [priv_dir, include_dir],
        create_dirs = [priv_dir, include_dir],
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
    for src in priv_srcs:
        copy_path(
            src,
            _elixir_priv_destination(ctx, src, priv_dir),
            inputs = [src],
            toolchain_identity = toolchain["identity"] + "\x00elixir-stage-priv.v2",
            identifier = ctx["label"]["id"] + ":elixir-stage-priv:" + src,
        )
    for src in include:
        copy_path(
            src,
            _elixir_include_destination(ctx, src, include_dir),
            inputs = [src],
            toolchain_identity = toolchain["identity"] + "\x00elixir-stage-include.v1",
            identifier = ctx["label"]["id"] + ":elixir-stage-include:" + src,
        )
    app = {
        "elixir_app": True,
        "app_name": app_name,
        "mix_env": mix_env,
        "mix_config": mix_config,
        "source_root": ctx["label"]["package"] or ".",
        "source_inputs": _unique(srcs + config + data + priv_srcs + include + docs + _elixir_optional_inputs([mix_config])),
        "priv_inputs": priv_srcs,
        "include_inputs": include,
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "include_dir": include_dir,
        "compile_metadata": metadata_path,
        "compile_warnings": warnings_path,
    }
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "elixir_library",
        "elixir_app": True,
        "app_name": app_name,
        "mix_env": mix_env,
        "mix_config": mix_config,
        "priv_inputs": priv_srcs,
        "include_inputs": include,
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "include_dir": include_dir,
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
        if not src.endswith(".exs"):
            continue
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

def _elixir_code_path_args(ctx, apps):
    args = []
    for app in apps:
        ebin = app.get("ebin_dir", "")
        if ebin:
            args.extend(["-pa", _elixir_from_package(ctx, ebin)])
    return args

def _elixir_test_runner_argv(ctx, toolchain, apps, srcs, mix_config):
    test_paths = _elixir_test_paths(ctx, srcs)
    _elixir_validate_test_mode(ctx, mix_config)
    if mix_config:
        no_start = ["--no-start"] if _elixir_attr(ctx, "no_start", False) else []
        return (toolchain["mix"], ["test", "--no-compile", "--no-deps-check"] + no_start + test_paths + _elixir_test_args(ctx))
    start_args = []
    if "test/test_helper.exs" not in test_paths:
        start_args = ["-e", "ExUnit.start()"]
    require_args = []
    for path in test_paths:
        require_args.extend(["-r", path])
    return (
        toolchain["elixir"],
        _elixir_code_path_args(ctx, apps) + _elixir_interpreter_opts(ctx) + start_args + require_args + _elixir_test_args(ctx),
    )

def _elixir_test_runner_source(setup_tasks):
    return """
[runner, args_file, results, log, native_results, target, runner_type] = System.argv()
args = args_file |> File.read!() |> String.split("\\n", trim: true)
setup_tasks = """ + _elixir_command_plan(setup_tasks) + """

{setup_output, setup_status} =
  Enum.reduce_while(setup_tasks, {"", 0}, fn [task | task_args], {output, _status} ->
    {task_output, status} =
      System.cmd(runner, [task | task_args], stderr_to_stdout: true)

    combined = output <> task_output
    if status == 0, do: {:cont, {combined, status}}, else: {:halt, {combined, status}}
  end)

{test_output, test_status} =
  if setup_status == 0 do
    System.cmd(runner, args, stderr_to_stdout: true)
  else
    {"", setup_status}
  end

output = setup_output <> test_output
status = if setup_status == 0, do: test_status, else: setup_status
File.write!(log, output)
File.write!(native_results, output)

{total, failed} =
  case Regex.scan(~r/(\\d+) tests?, (\\d+) failures/, output) |> List.last() do
    [_, total, failed] -> {String.to_integer(total), String.to_integer(failed)}
    _ ->
      case Regex.scan(~r/Result: (\\d+)\\/(\\d+) passed/, output) |> List.last() do
        [_, passed, total] ->
          total = String.to_integer(total)
          {total, total - String.to_integer(passed)}

        _ ->
          case Regex.scan(~r/Result: (\\d+) passed/, output) |> List.last() do
            [_, passed] ->
              passed = String.to_integer(passed)
              {passed, 0}

            _ ->
              case Regex.scan(~r/Result: (\\d+) tests/, output) |> List.last() do
                [_, total] -> {String.to_integer(total), 0}
                _ -> {0, if(status == 0, do: 0, else: 1)}
              end
          end
      end
  end

passed = max(total - failed, 0)
run_status = if status == 0, do: "passed", else: "failed"

payload = %{
  schema: "once.test_results.v1",
  target: target,
  runner: %{type: runner_type, metadata: %{}},
  status: run_status,
  summary: %{total: total, passed: passed, failed: failed, skipped: 0, flaky: 0},
  cases: [],
  artifacts: %{logs: [log], native_results: [native_results]}
}

File.write!(results, :json.encode(payload))
System.halt(status)
"""

def _elixir_stage_apps(ctx, apps, build_root, mix_env, identifier):
    for app in apps:
        app_name = app.get("app_name", "")
        ebin = app.get("ebin_dir", "")
        if app_name and ebin:
            app_dir = _parent_dir(ebin)
            copy_path(
                app_dir,
                build_root + "/" + mix_env + "/lib/" + app_name,
                kind = "tree",
                inputs = [app_dir],
                identifier = identifier + ":" + app_name,
            )

def _elixir_stage_consumer(ctx, root_app, apps, build_root, mix_env, identifier):
    root_build_env = root_app.get("build_env_dir", "")
    if not root_build_env:
        _elixir_stage_apps(ctx, apps, build_root, mix_env, identifier)
        return

    copy_path(
        root_build_env,
        build_root + "/" + mix_env,
        kind = "tree",
        inputs = [root_build_env],
        identifier = identifier + ":root-build-environment",
    )
    for app in apps:
        app_name = app.get("app_name", "")
        ebin = app.get("ebin_dir", "")
        is_root = app_name == root_app.get("app_name", "") and ebin == root_app.get("ebin_dir", "")
        if app_name and ebin and not is_root:
            app_dir = _parent_dir(ebin)
            copy_path(
                app_dir,
                build_root + "/" + mix_env + "/lib/" + app_name,
                kind = "tree",
                inputs = [app_dir],
                identifier = identifier + ":" + app_name,
            )

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
            "cacheable": _elixir_attr(ctx, "cacheable", True),
            "timeout_ms": timeout_ms,
            "run_from_workspace_root": True,
        },
        "labels": labels,
        "metadata": {},
    }

def _elixir_test_info_for(ctx, runner, mix_config, apps, srcs, results, log, native_results):
    test_paths = _elixir_test_paths(ctx, srcs)
    _elixir_validate_test_mode(ctx, mix_config)
    if mix_config:
        no_start = ["--no-start"] if _elixir_attr(ctx, "no_start", False) else []
        return _elixir_test_info(ctx, "mix_test", "Mix test", runner, ["test", "--no-compile", "--no-deps-check"] + no_start + test_paths + _elixir_test_args(ctx), results, log, native_results)
    start_args = []
    if "test/test_helper.exs" not in test_paths:
        start_args = ["-e", "ExUnit.start()"]
    require_args = []
    for path in test_paths:
        require_args.extend(["-r", path])
    return _elixir_test_info(ctx, "elixir_exunit", "Elixir ExUnit", runner, _elixir_code_path_args(ctx, apps) + _elixir_interpreter_opts(ctx) + start_args + require_args + _elixir_test_args(ctx), results, log, native_results)

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
        "test_info": _elixir_test_info_for(ctx, "elixir", mix_config, [], srcs, results, log, native_results),
    }
    if ctx["capability"] != "test":
        return provider
    if len(ctx["deps"]) != 1:
        fail(ctx["label"]["id"] + ": elixir_test expects exactly one elixir_app dependency compiled with mix_env = \"test\"")
    lib = ctx["deps"][0]
    if not lib.get("elixir_app") or not lib.get("app_name") or not lib.get("ebin_dir") or not lib.get("transitive_elixir_apps"):
        fail(ctx["label"]["id"] + ": elixir_test dependency must provide a complete elixir_app record")
    if lib.get("mix_env") != "test":
        fail(ctx["label"]["id"] + ": elixir_test dependency must be compiled with mix_env = \"test\"")
    mix_config = lib.get("mix_config") or mix_config
    toolchain = _elixir_mix_toolchain() if mix_config else _elixir_compiler_toolchain()
    apps = _elixir_collect_apps([lib])
    provider["affected_inputs"] = _unique(_elixir_optional_inputs(input_srcs + config + data + tools + [mix_config]) + _elixir_app_inputs(apps))
    provider["test_info"] = _elixir_test_info_for(ctx, toolchain["mix"] if mix_config else toolchain["elixir"], mix_config, apps, srcs, results, log, native_results)
    if _elixir_attr(ctx, "setup", ""):
        run_action(
            argv = [_elixir_host_shell(ctx), "-c", _elixir_test_script(ctx, toolchain, lib, apps, srcs, config, data, mix_config, results, log, native_results)],
            inputs = provider["affected_inputs"],
            outputs = [test_dir, results, log, native_results],
            env = _elixir_action_env(toolchain),
            cacheable = _elixir_attr(ctx, "cacheable", True),
            toolchain_identity = toolchain["identity"] + ("\x00mix_test.v2" if mix_config else "\x00elixir_exunit.v2"),
            identifier = ctx["label"]["id"] + (":mix-test" if mix_config else ":elixir-test"),
        )
        return provider
    build_root = ctx["scratch_dir"] + "/test_build"
    home_dir = ctx["scratch_dir"] + "/test_home"
    runner_dir = ctx["scratch_dir"] + "/test_runner"
    runner_script = runner_dir + "/runner.exs"
    args_file = runner_dir + "/args.list"
    runner, args = _elixir_test_runner_argv(ctx, toolchain, apps, srcs, mix_config)
    setup_tasks = _elixir_attr(ctx, "setup_tasks", [])
    if setup_tasks and not mix_config:
        fail(ctx["label"]["id"] + ": setup_tasks requires Mix mode through an elixir_app provider with mix_config")
    write_path(runner_script, _elixir_test_runner_source(setup_tasks))
    write_path(args_file, _elixir_lines(args))
    project_inputs = []
    if mix_config:
        lockfile = _package_relative(ctx, _elixir_attr(ctx, "lockfile", "mix.lock"))
        lockfile_input = [lockfile] if host_file_exists(_elixir_abs_workspace_path(lockfile)) else []
        project_inputs = _unique(
            lib.get("sources", []) +
            input_srcs +
            config +
            data +
            tools +
            _elixir_app_source_inputs(apps) +
            [mix_config] +
            lockfile_input
        )
    _elixir_stage_consumer(ctx, lib, apps, build_root, "test", ctx["label"]["id"] + ":test-apps")
    run_action(
        argv = [
            toolchain["elixir"],
            _elixir_from_package(ctx, runner_script),
            runner,
            _elixir_from_package(ctx, args_file),
            _elixir_from_package(ctx, results),
            _elixir_from_package(ctx, log),
            _elixir_from_package(ctx, native_results),
            ctx["label"]["id"],
            "mix_test" if mix_config else "elixir_exunit",
        ],
        inputs = _unique(provider["affected_inputs"] + project_inputs + [runner_script, args_file]),
        outputs = [results, log, native_results],
        clean_paths = [home_dir, results, log, native_results],
        create_dirs = [home_dir, test_dir],
        cwd = _elixir_package_cwd(ctx),
        env = _elixir_action_env_with(ctx, toolchain, {
            "HOME": _elixir_from_package(ctx, home_dir),
            "MIX_ENV": "test",
            "MIX_BUILD_ROOT": _elixir_from_package(ctx, build_root),
            "MIX_BUILD_PATH": _elixir_from_package(ctx, build_root + "/test"),
            "MIX_HOME": toolchain.get("mix_home") or _elixir_from_package(ctx, home_dir + "/mix"),
            "HEX_HOME": _elixir_from_package(ctx, home_dir + "/hex"),
            "HEX_OFFLINE": "true",
            "ERL_LIBS": _elixir_from_package(ctx, build_root + "/test/lib"),
        }),
        cacheable = _elixir_attr(ctx, "cacheable", True),
        toolchain_identity = toolchain["identity"] + ("\x00mix_test.v2" if mix_config else "\x00elixir_exunit.v2"),
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
    is_nil(value) -> :null
    is_atom(value) -> Atom.to_string(value)
    is_binary(value) or is_number(value) or is_boolean(value) -> value
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
          "custom_compile" => dependency.opts[:compile] not in [nil, false],
          "destination" => destination,
          "manager" => normalize.(dependency.manager, normalize),
          "mix_env" => dependency.opts |> Keyword.get(:env, :prod) |> Atom.to_string(),
          "path_dependency" => dependency.opts[:path] != nil,
          "skip_compile" => dependency.opts[:compile] == false,
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

def _mix_target_name(prefix, app):
    return prefix + "-" + app.replace("_", "-").replace(".", "-").replace("+", "-")

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
    return [source_root + "/**/*"]

def _mix_generated_source_excludes(source_root):
    return [
        source_root + "/.once/**/*",
        source_root + "/_build/**/*",
        source_root + "/deps/**/*",
        source_root + "/node_modules/**/*",
        source_root + "/**/node_modules/**/*",
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
        target = targets_by_app.get(dependency["app"])
        if dependency.get("top_level") and target:
            roots.append(target)
    if roots:
        return _unique(roots)
    return [targets_by_app[app] for app in sorted(targets_by_app.keys())]

def _mix_dependencies_resolver(ctx):
    resolved = _mix_read_locked_graph(ctx)
    lock = resolved.get("lock") or {}
    all_dependencies = resolved.get("dependencies") or []
    path_dependency_targets = ctx["attrs"].get("path_dependencies") or {}
    active_path_dependencies = {}
    generated_path_dependencies = {}
    path_dependency_destinations = {}
    for dependency in all_dependencies:
        if dependency.get("path_dependency"):
            app = dependency.get("app") or ""
            active_path_dependencies[app] = True
            path_dependency_destinations[app] = _elixir_workspace_source_root(
                ctx["label"]["package"],
                dependency.get("destination") or "",
            )
            if not path_dependency_targets.get(app):
                generated_path_dependencies[app] = dependency
    for app in path_dependency_targets.keys():
        if not active_path_dependencies.get(app):
            fail(ctx["label"]["id"] + ": path_dependencies maps `" + app + "`, but that application is not an active Mix path dependency; active path dependencies: " + ", ".join(sorted(active_path_dependencies.keys())))
    dependencies = [dependency for dependency in all_dependencies if not dependency.get("path_dependency")]
    skipped_dependencies = {
        dependency["app"]: True
        for dependency in dependencies
        if dependency.get("skip_compile")
    }
    dependencies = [dependency for dependency in dependencies if not dependency.get("skip_compile")]
    dependency_index = _mix_dependency_index(dependencies)
    vendor_dir = _mix_trim_slash(ctx["attrs"].get("vendor_dir") or "deps")
    manifest = ctx["attrs"].get("manifest") or "mix.exs"
    lockfile = ctx["attrs"].get("lockfile") or "mix.lock"
    mix_env = ctx["attrs"].get("mix_env") or "prod"
    dependency_mix_env = ctx["attrs"].get("dependency_mix_env") or "prod"
    root_config = ctx["attrs"].get("config") or []
    root_config_entry = ctx["attrs"].get("config_entry") or ""
    root_config_target = ctx["attrs"].get("config_target") or "host"
    target_prefix = ctx["attrs"].get("target_prefix") or "mix"
    targets = []
    targets_by_app = {}
    identities = {}
    for app in active_path_dependencies.keys():
        targets_by_app[app] = path_dependency_targets.get(app) or ("./" + _mix_target_name(target_prefix, app))
    for app in sorted(dependency_index.keys()):
        dependency = dependency_index[app]
        identity = _mix_locked_identity(app, _mix_lock_entry(lock, app))
        targets_by_app[app] = "./" + _mix_target_name(target_prefix, app)
        identities[app] = identity
    for app in sorted(dependency_index.keys()):
        dependency = dependency_index[app]
        identity = identities[app]
        source_root = dependency.get("destination") or (vendor_dir + "/" + app)
        source_root = _mix_trim_slash(_mix_package_path(ctx, source_root))
        target_name = _mix_target_name(target_prefix, app)
        managers = identity["managers"]
        if not managers and dependency.get("manager"):
            managers = [dependency["manager"]]
        attrs = {
            "app_name": app,
            "mix_env": dependency.get("mix_env") or dependency_mix_env,
            "version": identity["version"],
            "config": root_config,
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
            "_mix_root_config_entry": root_config_entry,
            "_mix_root_config_env": mix_env,
            "_mix_root_config_target": root_config_target,
        }
        dependency_targets = []
        for child in dependency.get("dependencies") or []:
            target_ref = targets_by_app.get(child)
            if target_ref == None and skipped_dependencies.get(child):
                continue
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
    for app in sorted(generated_path_dependencies.keys()):
        dependency = generated_path_dependencies[app]
        workspace_source_root = path_dependency_destinations[app]
        source_root = _elixir_from_package(ctx, workspace_source_root)
        target_name = _mix_target_name(target_prefix, app)
        managers = [dependency["manager"]] if dependency.get("manager") else ["mix"]
        dependency_targets = []
        for child in dependency.get("dependencies") or []:
            target_ref = targets_by_app.get(child)
            if target_ref == None and skipped_dependencies.get(child):
                continue
            if target_ref == None:
                fail(ctx["label"]["id"] + ": Mix path dependency `" + app + "` references `" + child + "`, but the active graph contains no target for it")
            dependency_targets.append(target_ref)
        targets.append({
            "name": target_name,
            "kind": "mix_package",
            "deps": dependency_targets,
            "srcs": _mix_source_globs(source_root),
            "attrs": {
                "app_name": app,
                "mix_env": dependency.get("mix_env") or dependency_mix_env,
                "config": root_config,
                "data": [source_root + "/**/*"],
                "priv": [source_root + "/priv/**"],
                "include": [source_root + "/include/**/*.hrl"],
                "consolidate_protocols": False,
                "_mix_local": True,
                "_mix_package_name": app,
                "_mix_source": "path:" + workspace_source_root,
                "_mix_managers": managers,
                "_mix_custom_compile": dependency.get("custom_compile") or False,
                "_mix_source_root": source_root,
                "_mix_manifest": manifest,
                "_mix_lockfile": lockfile,
                "_mix_root_config_entry": root_config_entry,
                "_mix_root_config_env": mix_env,
                "_mix_root_config_target": root_config_target,
            },
        })
    roots = _mix_resolved_roots(ctx, all_dependencies, targets_by_app, identities)
    return {
        "targets": targets,
        "roots": roots,
        "attrs": {
            "_mix_resolved": True,
            "_mix_locked_roots": roots,
            "_mix_path_destinations": path_dependency_destinations,
        },
    }

def _mix_locked_scm_source():
    return """
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
"""

def _mix_materialize_app_directories_source():
    return """
materialize_symlink = fn path ->
  case File.read_link(path) do
    {:ok, target} ->
      source = Path.expand(target, Path.dirname(path))
      temporary = path <> ".once-materialized"
      File.rm_rf!(temporary)
      File.cp_r!(source, temporary)
      File.rm!(path)
      File.rename!(temporary, path)

    {:error, :einval} ->
      :ok

    {:error, :enoent} ->
      :ok

    {:error, reason} ->
      raise "could not inspect #{path}: #{inspect(reason)}"
  end
end

app_dir = Path.join([System.fetch_env!("MIX_BUILD_PATH"), "lib", app_name])
Enum.each(["priv", "include"], fn name ->
  path = Path.join(app_dir, name)
  materialize_symlink.(path)
  File.mkdir_p!(path)
end)
"""

def _mix_compile_source():
    return """
Mix.start()
""" + _mix_locked_scm_source() + """

[project_dir, app_name, mix_env, dependency_paths_file, dependency_apps_file, root_config_entry, root_config_env, root_config_target] = System.argv()
System.put_env("MIX_BUILD_ROOT", System.fetch_env!("MIX_BUILD_ROOT") |> Path.expand())
System.put_env("MIX_BUILD_PATH", System.fetch_env!("MIX_BUILD_PATH") |> Path.expand())

if root_config_entry != "" do
  dependency_env = Mix.env()
  dependency_target = Mix.target()
  Mix.env(String.to_atom(root_config_env))
  Mix.target(String.to_atom(root_config_target))

  config =
    root_config_entry
    |> Path.expand()
    |> Config.Reader.read!(
      env: String.to_atom(root_config_env),
      target: String.to_atom(root_config_target)
    )

  Mix.env(dependency_env)
  Mix.target(dependency_target)
  Application.put_all_env(config, persistent: true)
end

dependency_paths_file
|> File.read!()
|> String.split("\n", trim: true)
|> Enum.map(&Path.expand/1)
|> Enum.reverse()
|> Enum.each(&Code.prepend_path/1)

dependency_apps_file
|> File.read!()
|> String.split("\\n", trim: true)
|> Enum.map(&String.to_atom/1)
|> Enum.each(fn app ->
  case Application.load(app) do
    :ok -> :ok
    {:error, {:already_loaded, ^app}} -> :ok
    {:error, reason} -> raise "could not load dependency application #{app}: #{inspect(reason)}"
  end
end)

result =
  Mix.Project.in_project(String.to_atom(app_name), Path.expand(project_dir), [env: String.to_atom(mix_env)], fn _project ->
    Mix.Task.run("compile.all", [
      "--force",
      "--from-mix-deps-compile",
      "--no-deps-check",
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
    source_root = _elixir_attr(ctx, "_mix_source_root", "")
    source_prefix = _mix_workspace_path(ctx, source_root) if source_root else ""
    if (source_prefix and host_file_exists(source_prefix + "/mix.exs")) or "mix" in managers:
        return "mix"
    if (source_prefix and host_file_exists(source_prefix + "/rebar.config")) or "rebar3" in managers:
        return "rebar3"
    rendered = ", ".join(managers) if managers else "unknown"
    fail(ctx["label"]["id"] + ": Mix dependency build manager `" + rendered + "` is not supported; add a typed package implementation before building it")

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

def _mix_package_identity(ctx, manager):
    return "\x00".join([
        manager,
        _elixir_attr(ctx, "_mix_source", ""),
        _elixir_attr(ctx, "version", ""),
        _elixir_attr(ctx, "_mix_checksum", ""),
        _elixir_attr(ctx, "_mix_outer_checksum", ""),
        _elixir_attr(ctx, "_mix_revision", ""),
        ",".join(_elixir_attr(ctx, "_mix_managers", [])),
        "custom" if _elixir_attr(ctx, "_mix_custom_compile", False) else "standard",
    ])

def _mix_rebar_config_source():
    return """
Mix.start()
[source_root, output] = System.argv()
config =
  source_root
  |> Mix.Rebar.load_config()
  |> Mix.Rebar.dependency_config()
  |> Mix.Rebar.serialize_config()
File.write!(output, config)
"""

def _mix_rebar_app_validator_source():
    return """
[app_file, marker] = System.argv()
unless File.regular?(app_file), do: raise("Rebar compilation did not produce #{app_file}")
File.write!(marker, "ok\\n")
"""

def _mix_stage_package_tree(ctx, source_root, source_name, destination, identifier):
    inputs = glob([source_root + "/" + source_name + "/**/*"])
    if inputs:
        workspace_source_root = _elixir_workspace_source_root(ctx["label"]["package"], source_root)
        copy_path(
            workspace_source_root + "/" + source_name,
            destination,
            kind = "tree",
            inputs = inputs,
            identifier = identifier,
        )

def _mix_rebar_package_action(ctx, source_root, app_name, mix_env, apps, app_dir, ebin_dir, compile_log, compile_warnings, identity):
    toolchain = _elixir_rebar_toolchain()
    script_dir = ctx["scratch_dir"] + "/rebar"
    source_stage = script_dir + "/source"
    metadata_stage = script_dir + "/metadata"
    config_script = script_dir + "/config.exs"
    config_file = script_dir + "/rebar.config"
    config_home = script_dir + "/config_home"
    build_home = script_dir + "/build_home"
    compiled_root = ctx["build_dir"] + "/rebar-compiled"
    compiled_app_dir = compiled_root + "/" + app_name
    compiled_ebin_dir = compiled_app_dir + "/ebin"
    validation_marker = ctx["build_dir"] + "/rebar-app.valid"
    write_path(config_script, _mix_rebar_config_source())
    run_action(
        argv = [
            toolchain["elixir"],
            _elixir_from_package(ctx, config_script),
            source_root,
            _elixir_from_package(ctx, config_file),
        ],
        inputs = _unique(glob([source_root + "/rebar.config", source_root + "/rebar.config.script"]) + [config_script]),
        outputs = [config_file],
        clean_paths = [config_home],
        create_dirs = [config_home],
        cwd = _elixir_package_cwd(ctx),
        env = _elixir_action_env_with(ctx, toolchain, {
            "HOME": _elixir_from_package(ctx, config_home),
            "MIX_HOME": _elixir_from_package(ctx, config_home + "/mix"),
            "HEX_HOME": _elixir_from_package(ctx, config_home + "/hex"),
        }),
        toolchain_identity = toolchain["identity"] + "\x00mix-rebar-config.v1\x00" + identity,
        identifier = ctx["label"]["id"] + ":rebar-config",
    )
    copy_path(
        _elixir_workspace_source_root(ctx["label"]["package"], source_root),
        source_stage,
        kind = "tree",
        inputs = glob(ctx["srcs"]),
        identifier = ctx["label"]["id"] + ":rebar-source",
    )
    package_include = glob([source_root + "/include/**/*"])
    package_priv = glob([source_root + "/priv/**/*"])
    package_ebin = glob([source_root + "/ebin/**/*"])
    if package_include:
        _mix_stage_package_tree(
            ctx,
            source_root,
            "include",
            compiled_app_dir + "/include",
            ctx["label"]["id"] + ":rebar-include",
        )
    if package_priv:
        _mix_stage_package_tree(
            ctx,
            source_root,
            "priv",
            compiled_app_dir + "/priv",
            ctx["label"]["id"] + ":rebar-priv",
        )
    if package_ebin:
        _mix_stage_package_tree(
            ctx,
            source_root,
            "ebin",
            metadata_stage + "/ebin",
            ctx["label"]["id"] + ":rebar-metadata",
        )
    cwd = source_stage
    dependency_paths = [
        execution_path(path)
        for path in _elixir_app_code_paths(apps)
    ]
    rebar_paths = dependency_paths or [
        execution_path(compiled_ebin_dir),
    ]
    erlang_libraries = _unique([
        execution_path(_parent_dir(_parent_dir(path)))
        for path in _elixir_app_code_paths(apps)
    ] + [execution_path(compiled_root)])
    argv = [toolchain["rebar3"], "bare", "compile", "--paths", ":".join(rebar_paths)]
    resource_inputs = []
    if package_include:
        resource_inputs.append(compiled_app_dir + "/include")
    if package_priv:
        resource_inputs.append(compiled_app_dir + "/priv")
    run_action(
        argv = argv,
        inputs = _unique(_elixir_app_inputs(apps) + [config_file, source_stage] + resource_inputs),
        outputs = [compiled_ebin_dir, compile_log, compile_warnings],
        clean_paths = [build_home, compile_log, compile_warnings],
        create_dirs = [compiled_ebin_dir, build_home],
        cwd = _package_relative(ctx, cwd),
        env = _elixir_action_env_with(ctx, toolchain, {
            "ERL_LIBS": ":".join(erlang_libraries),
            "REBAR_BARE_COMPILER_OUTPUT_DIR": execution_path(compiled_app_dir),
            "REBAR_BUILD_DIR": execution_path(build_home + "/_build"),
            "REBAR_SKIP_PROJECT_PLUGINS": "true",
            "REBAR_CONFIG": execution_path(config_file),
            "REBAR_PROFILE": "prod",
            "TERM": "dumb",
            "HOME": execution_path(build_home),
        }),
        stdout = compile_log,
        stderr = compile_warnings,
        toolchain_identity = toolchain["identity"] + "\x00mix-rebar-package.v7\x00" + identity,
        identifier = ctx["label"]["id"] + ":rebar-compile",
    )
    app_sources = []
    app_inputs = []
    if package_ebin:
        app_sources.append(metadata_stage)
        app_inputs.append(metadata_stage)
    app_sources.append(compiled_app_dir)
    app_inputs.extend([compiled_ebin_dir] + resource_inputs)
    copy_path(
        app_sources,
        app_dir,
        kind = "tree",
        inputs = _unique(app_inputs),
        identifier = ctx["label"]["id"] + ":rebar-app",
    )
    run_action(
        argv = [
            toolchain["elixir"],
            "-e",
            _mix_rebar_app_validator_source(),
            "--",
            _elixir_from_package(ctx, ebin_dir + "/" + app_name + ".app"),
            _elixir_from_package(ctx, validation_marker),
        ],
        inputs = [ebin_dir],
        outputs = [validation_marker],
        cwd = _elixir_package_cwd(ctx),
        env = _elixir_action_env(toolchain),
        toolchain_identity = toolchain["identity"] + "\x00mix-rebar-validate-app.v1\x00" + identity,
        identifier = ctx["label"]["id"] + ":rebar-validate-app",
    )
    return (toolchain, validation_marker)

def _mix_package_impl(ctx):
    local_package = _elixir_attr(ctx, "_mix_local", False)
    if not _elixir_attr(ctx, "_mix_locked", False) and not local_package:
        fail(ctx["label"]["id"] + ": mix_package targets are generated by mix_dependencies and cannot be declared without resolved metadata")
    manager = _mix_supported_manager(ctx)
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
    include_dir = app_dir + "/include"
    home_dir = ctx["scratch_dir"] + "/mix_home"
    script_dir = ctx["scratch_dir"] + "/mix_compile"
    dependency_paths_file = script_dir + "/dependency_paths.list"
    dependency_apps_file = script_dir + "/dependency_apps.list"
    compile_script = script_dir + "/compile.exs"
    compile_log = ctx["build_dir"] + "/mix-compile.log"
    compile_warnings = ctx["build_dir"] + "/mix-compile-warnings.log"
    srcs = glob(ctx["srcs"], exclude = _mix_generated_source_excludes(source_root))
    root_config = _elixir_config_inputs(ctx)
    root_config_entry = _package_relative(ctx, _elixir_attr(ctx, "_mix_root_config_entry", ""))
    root_config_env = _elixir_attr(ctx, "_mix_root_config_env", mix_env)
    root_config_target = _elixir_attr(ctx, "_mix_root_config_target", "host")
    if root_config_entry and root_config_entry not in root_config:
        fail(ctx["label"]["id"] + ": root config entry `" + root_config_entry + "` must match one of the declared config inputs")
    identity = _mix_package_identity(ctx, manager)
    workspace_source_root = _elixir_workspace_source_root(ctx["label"]["package"], source_root)
    workspace_mix_config = workspace_source_root + "/mix.exs"
    compile_metadata = ""
    package_priv = glob([source_root + "/priv/**/*"])
    package_include = glob([source_root + "/include/**/*"])
    if manager == "mix":
        toolchain = _elixir_mix_toolchain()
        inputs = _unique(srcs + root_config + _elixir_app_inputs(apps) + [dependency_paths_file, dependency_apps_file, compile_script])
        write_path(dependency_paths_file, _elixir_lines([_elixir_from_package(ctx, path) for path in dep_code_paths]))
        write_path(dependency_apps_file, _elixir_lines([app.get("app_name", "") for app in apps if app.get("app_name", "")]))
        write_path(compile_script, _mix_compile_source())
        run_action(
            argv = [
                toolchain["elixir"],
                _elixir_from_package(ctx, compile_script),
                source_root,
                app_name,
                mix_env,
                _elixir_from_package(ctx, dependency_paths_file),
                _elixir_from_package(ctx, dependency_apps_file),
                _elixir_from_package(ctx, root_config_entry),
                root_config_env,
                root_config_target,
            ],
            inputs = inputs,
            outputs = [ebin_dir, priv_dir, include_dir, compile_log, compile_warnings],
            clean_paths = [build_root, home_dir, compile_log, compile_warnings],
            create_dirs = [priv_dir, include_dir, home_dir],
            cwd = _elixir_package_cwd(ctx),
            env = _elixir_action_env_with(ctx, toolchain, {
                "HOME": _elixir_from_package(ctx, home_dir),
                "MIX_HOME": _elixir_from_package(ctx, home_dir + "/mix"),
                "HEX_HOME": _elixir_from_package(ctx, home_dir + "/hex"),
                "MIX_BUILD_ROOT": _elixir_from_package(ctx, build_root),
                "MIX_BUILD_PATH": _elixir_from_package(ctx, build_root + "/" + mix_env),
                "MIX_ENV": mix_env,
                "HEX_OFFLINE": "true",
            }),
            stdout = compile_log,
            stderr = compile_warnings,
            toolchain_identity = toolchain["identity"] + "\x00mix-package-compile.v3\x00" + mix_env + "\x00" + identity,
            identifier = ctx["label"]["id"] + ":mix-compile",
        )
        _mix_stage_package_tree(ctx, source_root, "priv", priv_dir, ctx["label"]["id"] + ":mix-priv")
        _mix_stage_package_tree(ctx, source_root, "include", include_dir, ctx["label"]["id"] + ":mix-include")
    else:
        toolchain, compile_metadata = _mix_rebar_package_action(
            ctx,
            source_root,
            app_name,
            mix_env,
            apps,
            app_dir,
            ebin_dir,
            compile_log,
            compile_warnings,
            identity,
        )
    app = {
        "elixir_app": True,
        "app_name": app_name,
        "mix_env": mix_env,
        "mix_config": workspace_mix_config,
        "source_root": workspace_source_root,
        "source_inputs": srcs,
        "full_source_tree": True,
        "priv_inputs": package_priv,
        "include_inputs": package_include,
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "include_dir": include_dir,
        "compile_metadata": compile_metadata,
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
    transitive_locked_packages = _mix_collect_locked_packages(
        ctx["deps"],
        None if local_package else package_record,
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "mix_package",
        "elixir_app": True,
        "locked": not local_package,
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
        "mix_config": workspace_mix_config,
        "ebin_dir": ebin_dir,
        "priv_dir": priv_dir,
        "include_dir": include_dir,
        "compile_metadata": compile_metadata,
        "compile_warnings": compile_warnings,
        "sources": srcs,
        "transitive_sources": _elixir_collect_transitive_strings(ctx["deps"], "transitive_sources", srcs),
        "transitive_elixir_apps": _elixir_transitive_apps(ctx["deps"], app),
        "transitive_locked_packages": transitive_locked_packages,
    }

def _mix_dependencies_impl(ctx):
    apps = _elixir_collect_apps(ctx["deps"])
    packages = _mix_collect_locked_packages(ctx["deps"])
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "mix_dependencies",
        "elixir_app": True,
        "dependency_set": True,
        "locked": _elixir_attr(ctx, "_mix_resolved", False),
        "locked_roots": _elixir_attr(ctx, "_mix_locked_roots", []),
        "path_dependency_destinations": _elixir_attr(ctx, "_mix_path_destinations", {}),
        "locked_packages": packages,
        "transitive_sources": _elixir_collect_transitive_strings(ctx["deps"], "transitive_sources", []),
        "transitive_elixir_apps": apps,
    }

def _mix_project_compile_source():
    return """
Mix.start()
""" + _mix_locked_scm_source() + """

[project_dir, app_name, mix_env, dependency_paths_file, dependency_apps_file, compile_args_file] = System.argv()
System.put_env("MIX_BUILD_ROOT", System.fetch_env!("MIX_BUILD_ROOT") |> Path.expand())
System.put_env("MIX_BUILD_PATH", System.fetch_env!("MIX_BUILD_PATH") |> Path.expand())
paths = dependency_paths_file |> File.read!() |> String.split("\\n", trim: true) |> Enum.map(&Path.expand/1)
apps = dependency_apps_file |> File.read!() |> String.split("\\n", trim: true) |> Enum.map(&String.to_atom/1)
args = compile_args_file |> File.read!() |> String.split("\\n", trim: true)
Enum.each(Enum.reverse(paths), &Code.prepend_path/1)

Enum.each(apps, fn app ->
  case Application.load(app) do
    :ok -> :ok
    {:error, {:already_loaded, ^app}} -> :ok
    {:error, reason} -> raise "could not load dependency application #{app}: #{inspect(reason)}"
  end
end)

result =
  Mix.Project.in_project(String.to_atom(app_name), Path.expand(project_dir), [env: String.to_atom(mix_env)], fn _project ->
    project_app = Mix.Project.config()[:app]
    if project_app != String.to_atom(app_name), do: raise("Mix project application is #{project_app}, expected #{app_name}")
    Mix.Task.run("loadconfig")
    Mix.Task.run("compile.all", ["--force", "--no-deps-check", "--no-prune-code-paths", "--return-errors"] ++ args)
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
""" + _mix_materialize_app_directories_source()

def _mix_project_paths(ctx):
    app_name = _elixir_app_name(ctx)
    mix_env = _elixir_mix_env(ctx)
    build_root = ctx["build_dir"] + "/mix"
    app_dir = build_root + "/" + mix_env + "/lib/" + app_name
    return {
        "app_name": app_name,
        "mix_env": mix_env,
        "build_root": build_root,
        "build_env_dir": build_root + "/" + mix_env,
        "app_dir": app_dir,
        "ebin_dir": app_dir + "/ebin",
        "priv_dir": app_dir + "/priv",
        "include_dir": app_dir + "/include",
        "compile_metadata": app_dir + "/.mix",
        "compile_warnings": ctx["build_dir"] + "/mix-project-warnings.log",
        "compile_log": ctx["build_dir"] + "/mix-project.log",
    }

def _mix_project_provider(ctx, paths, apps, sources, source_inputs, mix_config, path_dependency_destinations):
    app = {
        "elixir_app": True,
        "mix_project": True,
        "app_name": paths["app_name"],
        "mix_env": paths["mix_env"],
        "mix_config": mix_config,
        "source_root": ctx["label"]["package"] or ".",
        "source_inputs": source_inputs,
        "path_dependency_destinations": path_dependency_destinations,
        "build_env_dir": paths["build_env_dir"],
        "app_dir": paths["app_dir"],
        "ebin_dir": paths["ebin_dir"],
        "priv_dir": paths["priv_dir"],
        "include_dir": paths["include_dir"],
        "compile_metadata": paths["compile_metadata"],
        "compile_warnings": paths["compile_warnings"],
    }
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "mix_project",
        "elixir_app": True,
        "mix_project": True,
        "app_name": paths["app_name"],
        "mix_env": paths["mix_env"],
        "mix_config": mix_config,
        "source_root": ctx["label"]["package"] or ".",
        "source_inputs": source_inputs,
        "path_dependency_destinations": path_dependency_destinations,
        "build_env_dir": paths["build_env_dir"],
        "app_dir": paths["app_dir"],
        "ebin_dir": paths["ebin_dir"],
        "priv_dir": paths["priv_dir"],
        "include_dir": paths["include_dir"],
        "compile_metadata": paths["compile_metadata"],
        "compile_warnings": paths["compile_warnings"],
        "sources": sources,
        "transitive_sources": _elixir_collect_transitive_strings(ctx["deps"], "transitive_sources", sources),
        "transitive_elixir_apps": _elixir_transitive_apps(ctx["deps"], app),
    }

def _mix_project_tasks_runner_source(commands):
    return """
Mix.start()
""" + _mix_locked_scm_source() + """

[project_dir, app_name, mix_env, dependency_paths_file, dependency_apps_file] = System.argv()
System.put_env("MIX_BUILD_ROOT", System.fetch_env!("MIX_BUILD_ROOT") |> Path.expand())
System.put_env("MIX_BUILD_PATH", System.fetch_env!("MIX_BUILD_PATH") |> Path.expand())
paths = dependency_paths_file |> File.read!() |> String.split("\\n", trim: true) |> Enum.map(&Path.expand/1)
apps = dependency_apps_file |> File.read!() |> String.split("\\n", trim: true) |> Enum.map(&String.to_atom/1)
commands = """ + _elixir_command_plan(commands) + """
Enum.each(Enum.reverse(paths), &Code.prepend_path/1)

Enum.each(apps, fn app ->
  case Application.load(app) do
    :ok -> :ok
    {:error, {:already_loaded, ^app}} -> :ok
    {:error, reason} -> raise "could not load run application #{app}: #{inspect(reason)}"
  end
end)

Mix.Project.in_project(String.to_atom(app_name), Path.expand(project_dir), [env: String.to_atom(mix_env)], fn _project ->
  Mix.Task.run("loadconfig")
  Mix.Task.run("loadpaths", ["--no-deps-check"])

  Enum.each(commands, fn [task | args] ->
    Mix.Task.reenable(task)
    Mix.Task.run(task, args)
  end)
end)
"""

def _mix_project_run_spec(ctx):
    run_task = _elixir_attr(ctx, "run_task", "")
    run_tasks = _elixir_attr(ctx, "run_tasks", [])
    runtime_args = ctx["run"]["args"]
    if run_task and run_tasks:
        fail(ctx["label"]["id"] + ": mix_project accepts either run_task or run_tasks, not both")
    if run_tasks and runtime_args:
        fail(ctx["label"]["id"] + ": run arguments cannot be combined with run_tasks")
    task_args = _elixir_attr(ctx, "run_args", [])
    if not run_task and not run_tasks:
        if not runtime_args:
            fail(ctx["label"]["id"] + ": mix_project requires run_task, run_tasks, or a task after `--` before it can be run")
        if not runtime_args[0] or runtime_args[0].startswith("-"):
            fail(ctx["label"]["id"] + ": expected a Mix task as the first argument after `--`, got `" + runtime_args[0] + "`")
        run_task = runtime_args[0]
        task_args = runtime_args[1:]
    elif run_task:
        task_args = task_args + runtime_args
    return {
        "task": run_task,
        "tasks": run_tasks,
        "args": task_args,
    }

def _mix_project_impl(ctx):
    toolchain = _elixir_mix_toolchain()
    paths = _mix_project_paths(ctx)
    apps = _elixir_collect_apps(ctx["deps"])
    mix_config = _package_relative(ctx, _elixir_attr(ctx, "manifest", "mix.exs"))
    lockfile = _package_relative(ctx, _elixir_attr(ctx, "lockfile", "mix.lock"))
    lockfile_input = [lockfile] if host_file_exists(_elixir_abs_workspace_path(lockfile)) else []
    sources = glob(ctx["srcs"] or ["lib/**/*.ex", "lib/**/*.exs"])
    config = _elixir_config_inputs(ctx)
    data = _elixir_data_inputs(ctx)
    priv = _elixir_priv_inputs(ctx)
    include = _elixir_include_inputs(ctx)
    tools = _elixir_tool_inputs(ctx)
    source_inputs = _unique(sources + config + data + priv + include + tools + [mix_config] + lockfile_input)
    path_dependency_destinations = _elixir_collect_path_dependency_destinations(ctx["deps"])
    provider = _mix_project_provider(ctx, paths, apps, sources, source_inputs, mix_config, path_dependency_destinations)
    if ctx["capability"] == "run":
        run_spec = _mix_project_run_spec(ctx)
        run_task = run_spec["task"]
        run_tasks = run_spec["tasks"]
        task_args = run_spec["args"]
        run_cacheable = _elixir_attr(ctx, "run_cacheable", False)
        runtime_root = ctx["scratch_dir"] + "/run_build"
        runtime_home = ctx["scratch_dir"] + "/run_home"
        _elixir_stage_consumer(ctx, provider, apps + [provider], runtime_root, paths["mix_env"], ctx["label"]["id"] + ":run-apps")
        run_flags = []
        if _elixir_attr(ctx, "run_no_compile", True):
            run_flags.append("--no-compile")
        if _elixir_attr(ctx, "run_no_deps_check", True):
            run_flags.append("--no-deps-check")
        run_env = _elixir_run_action_env_with(ctx, toolchain, {
            "HOME": _elixir_from_package(ctx, runtime_home),
            "MIX_ENV": paths["mix_env"],
            "MIX_BUILD_ROOT": _elixir_from_package(ctx, runtime_root),
            "MIX_BUILD_PATH": _elixir_from_package(ctx, runtime_root + "/" + paths["mix_env"]),
            "MIX_HOME": toolchain["mix_home"],
            "HEX_HOME": _elixir_from_package(ctx, runtime_home + "/hex"),
            "HEX_OFFLINE": "true",
            "ERL_LIBS": _elixir_from_package(ctx, runtime_root + "/" + paths["mix_env"] + "/lib"),
        })
        run_argv = [toolchain["mix"], run_task] + run_flags + task_args
        run_inputs = []
        run_identity = run_task
        if run_tasks:
            run_dir = ctx["scratch_dir"] + "/run_tasks"
            run_script = run_dir + "/runner.exs"
            run_paths = run_dir + "/dependency_paths.list"
            run_apps = run_dir + "/dependency_apps.list"
            write_path(run_script, _mix_project_tasks_runner_source(run_tasks))
            write_path(run_paths, _elixir_lines([_elixir_from_package(ctx, path) for path in _elixir_app_code_paths(apps + [provider])]))
            write_path(run_apps, _elixir_lines([app.get("app_name", "") for app in apps + [provider] if app.get("app_name", "")]))
            run_argv = [
                toolchain["elixir"],
                _elixir_from_package(ctx, run_script),
                ".",
                paths["app_name"],
                paths["mix_env"],
                _elixir_from_package(ctx, run_paths),
                _elixir_from_package(ctx, run_apps),
            ]
            run_inputs = [run_script, run_paths, run_apps]
            run_identity = "tasks"
        run_action(
            argv = run_argv,
            inputs = _unique(sources + _elixir_app_inputs(apps + [provider]) + config + data + priv + tools + [mix_config] + run_inputs),
            clean_paths = [runtime_home],
            create_dirs = [runtime_home],
            cwd = _elixir_package_cwd(ctx),
            env = run_env,
            cacheable = run_cacheable,
            inherit_parent_env = not run_cacheable,
            toolchain_identity = toolchain["identity"] + "\x00mix-project-run.v2\x00" + paths["mix_env"] + "\x00" + run_identity,
            identifier = ctx["label"]["id"] + ":mix-run",
        )
        return provider
    script_dir = ctx["scratch_dir"] + "/mix_project"
    home_dir = ctx["scratch_dir"] + "/mix_home"
    compile_script = script_dir + "/compile.exs"
    dependency_paths_file = script_dir + "/dependency_paths.list"
    dependency_apps_file = script_dir + "/dependency_apps.list"
    compile_args_file = script_dir + "/compile_args.list"
    project_files = source_inputs
    write_path(compile_script, _mix_project_compile_source())
    write_path(dependency_paths_file, _elixir_lines([_elixir_from_package(ctx, path) for path in _elixir_app_code_paths(apps)]))
    write_path(dependency_apps_file, _elixir_lines([app.get("app_name", "") for app in apps if app.get("app_name", "")]))
    write_path(compile_args_file, _elixir_lines(_elixir_attr(ctx, "compile_args", [])))
    inputs = _unique(
        project_files +
        _elixir_app_inputs(apps) +
        _elixir_app_source_inputs(apps) +
        [compile_script, dependency_paths_file, dependency_apps_file, compile_args_file]
    )
    run_action(
        argv = [
            toolchain["elixir"],
            _elixir_from_package(ctx, compile_script),
            ".",
            paths["app_name"],
            paths["mix_env"],
            _elixir_from_package(ctx, dependency_paths_file),
            _elixir_from_package(ctx, dependency_apps_file),
            _elixir_from_package(ctx, compile_args_file),
        ],
        inputs = inputs,
        outputs = [paths["build_env_dir"], paths["compile_log"], paths["compile_warnings"]],
        clean_paths = [paths["build_root"], home_dir, paths["compile_log"], paths["compile_warnings"]],
        create_dirs = [home_dir],
        cwd = _elixir_package_cwd(ctx),
        env = _elixir_action_env_with(ctx, toolchain, {
            "HOME": _elixir_from_package(ctx, home_dir),
            "MIX_ENV": paths["mix_env"],
            "MIX_BUILD_ROOT": _elixir_from_package(ctx, paths["build_root"]),
            "MIX_BUILD_PATH": _elixir_from_package(ctx, paths["build_root"] + "/" + paths["mix_env"]),
            "MIX_HOME": _elixir_from_package(ctx, home_dir + "/mix"),
            "HEX_HOME": _elixir_from_package(ctx, home_dir + "/hex"),
            "HEX_OFFLINE": "true",
        }),
        stdout = paths["compile_log"],
        stderr = paths["compile_warnings"],
        toolchain_identity = toolchain["identity"] + "\x00mix-project-compile.v2\x00" + paths["mix_env"],
        identifier = ctx["label"]["id"] + ":mix-compile",
    )
    for source in priv:
        copy_path(
            source,
            _elixir_priv_destination(ctx, source, paths["priv_dir"]),
            inputs = [source],
            toolchain_identity = toolchain["identity"] + "\x00mix-project-priv.v1",
            identifier = ctx["label"]["id"] + ":mix-project-priv:" + source,
        )
    for source in include:
        copy_path(
            source,
            _elixir_include_destination(ctx, source, paths["include_dir"]),
            inputs = [source],
            toolchain_identity = toolchain["identity"] + "\x00mix-project-include.v1",
            identifier = ctx["label"]["id"] + ":mix-project-include:" + source,
        )
    return provider

def _mix_release_runner_source(pre_tasks):
    return """
Mix.start()

[project_dir, app_name, mix_env, dependency_paths_file, dependency_apps_file, release_name, release_args_file, output_dir] = System.argv()
System.put_env("MIX_BUILD_ROOT", System.fetch_env!("MIX_BUILD_ROOT") |> Path.expand())
System.put_env("MIX_BUILD_PATH", System.fetch_env!("MIX_BUILD_PATH") |> Path.expand())
paths = dependency_paths_file |> File.read!() |> String.split("\\n", trim: true) |> Enum.map(&Path.expand/1)
apps = dependency_apps_file |> File.read!() |> String.split("\\n", trim: true) |> Enum.map(&String.to_atom/1)
tasks = """ + _elixir_command_plan(pre_tasks) + """
release_args = release_args_file |> File.read!() |> String.split("\\n", trim: true)
Enum.each(Enum.reverse(paths), &Code.prepend_path/1)

Enum.each(apps, fn app ->
  case Application.load(app) do
    :ok -> :ok
    {:error, {:already_loaded, ^app}} -> :ok
    {:error, reason} -> raise "could not load release application #{app}: #{inspect(reason)}"
  end
end)

Mix.Project.in_project(String.to_atom(app_name), Path.expand(project_dir), [env: String.to_atom(mix_env)], fn _project ->
  project_app = Mix.Project.config()[:app]
  if project_app != String.to_atom(app_name), do: raise("Mix project application is #{project_app}, expected #{app_name}")
  Mix.Task.run("loadconfig")
  Mix.Task.run("loadpaths", ["--no-deps-check"])

  Enum.each(tasks, fn [task | args] ->
    Mix.Task.reenable(task)
    Mix.Task.run(task, args)
  end)

  args =
    (if release_name == "", do: [], else: [release_name]) ++
      ["--no-compile", "--no-deps-check", "--overwrite", "--path", Path.expand(output_dir)] ++
      release_args

  Mix.Task.reenable("release")
  Mix.Task.run("release", args)
end)
"""

def _mix_release_impl(ctx):
    if len(ctx["deps"]) != 1:
        fail(ctx["label"]["id"] + ": mix_release expects exactly one mix_project dependency")
    app = ctx["deps"][0]
    if not app.get("mix_project") or not app.get("app_name") or not app.get("build_env_dir") or not app.get("transitive_elixir_apps"):
        fail(ctx["label"]["id"] + ": mix_release dependency must provide a complete mix_project record")
    toolchain = _elixir_mix_toolchain()
    app_name = app["app_name"]
    mix_env = app["mix_env"]
    manifest = _package_relative(ctx, _elixir_attr(ctx, "manifest", "mix.exs"))
    lockfile = _package_relative(ctx, _elixir_attr(ctx, "lockfile", "mix.lock"))
    lockfile_input = [lockfile] if host_file_exists(_elixir_abs_workspace_path(lockfile)) else []
    config = _elixir_config_inputs(ctx)
    data = _elixir_data_inputs(ctx)
    tools = _elixir_tool_inputs(ctx)
    project_files = _unique(glob(ctx["srcs"]) + config + data + tools + [manifest] + lockfile_input)
    apps = _elixir_collect_apps([app])

    build_root = ctx["scratch_dir"] + "/release_build"
    home_dir = ctx["scratch_dir"] + "/release_home"
    runner_dir = ctx["scratch_dir"] + "/release_runner"
    runner_script = runner_dir + "/release.exs"
    dependency_paths_file = runner_dir + "/dependency_paths.list"
    dependency_apps_file = runner_dir + "/dependency_apps.list"
    release_args_file = runner_dir + "/release_args.list"
    release_dir = declare_output("release")
    release_log = declare_output("mix-release.log")
    release_warnings = declare_output("mix-release-warnings.log")

    _elixir_stage_consumer(ctx, app, apps, build_root, mix_env, ctx["label"]["id"] + ":release-apps")

    pre_tasks = _elixir_attr(ctx, "pre_tasks", [])
    write_path(runner_script, _mix_release_runner_source(pre_tasks))
    write_path(
        dependency_paths_file,
        _elixir_lines([_elixir_from_package(ctx, path) for path in _elixir_app_code_paths(apps)]),
    )
    write_path(
        dependency_apps_file,
        _elixir_lines([candidate.get("app_name", "") for candidate in apps if candidate.get("app_name", "")]),
    )
    write_path(release_args_file, _elixir_lines(_elixir_attr(ctx, "release_args", [])))
    run_action(
        argv = [
            toolchain["elixir"],
            _elixir_from_package(ctx, runner_script),
            ".",
            app_name,
            mix_env,
            _elixir_from_package(ctx, dependency_paths_file),
            _elixir_from_package(ctx, dependency_apps_file),
            _elixir_attr(ctx, "release_name", ""),
            _elixir_from_package(ctx, release_args_file),
            _elixir_from_package(ctx, release_dir),
        ],
        inputs = _unique(
            project_files +
            _elixir_app_inputs(apps) +
            _elixir_app_source_inputs(apps) +
            [runner_script, dependency_paths_file, dependency_apps_file, release_args_file]
        ),
        outputs = [release_dir, release_log, release_warnings],
        clean_paths = [home_dir, release_dir, release_log, release_warnings],
        create_dirs = [home_dir],
        cwd = _elixir_package_cwd(ctx),
        env = _elixir_action_env_with(ctx, toolchain, {
            "HOME": _elixir_from_package(ctx, home_dir),
            "MIX_ENV": mix_env,
            "MIX_BUILD_ROOT": _elixir_from_package(ctx, build_root),
            "MIX_BUILD_PATH": _elixir_from_package(ctx, build_root + "/" + mix_env),
            "MIX_HOME": toolchain["mix_home"],
            # Pin the archives directory to the detected Mix home. Assembling a
            # release resolves each dependency's SCM, and a Hex dependency has no
            # SCM until the Hex archive is on the load path. An inherited
            # MIX_ARCHIVES can point at a different Mix home that never had Hex
            # installed, which leaves Hex unloaded and fails resolution.
            "MIX_ARCHIVES": toolchain["mix_home"] + "/archives",
            "HEX_HOME": _elixir_from_package(ctx, home_dir + "/hex"),
            "HEX_OFFLINE": "true",
            "ERL_LIBS": _elixir_from_package(ctx, build_root + "/" + mix_env + "/lib"),
        }),
        stdout = release_log,
        stderr = release_warnings,
        cacheable = _elixir_attr(ctx, "cacheable", True),
        toolchain_identity = toolchain["identity"] + "\x00mix-release.v2\x00" + mix_env,
        identifier = ctx["label"]["id"] + ":mix-release",
    )
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "mix_release",
        "mix_release": True,
        "app_name": app_name,
        "release": release_dir,
        "release_dir": release_dir,
        "release_log": release_log,
        "release_warnings": release_warnings,
        "default_output": release_dir,
        "affected_inputs": _unique(project_files + _elixir_app_inputs(apps)),
    }

def _mix_workspace_metadata_source():
    return """
Mix.start()
[project_dir] = System.argv()

unless Code.ensure_loaded?(:json) and function_exported?(:json, :encode, 1) do
  raise "Mix project adaptation requires Erlang 27 or newer for :json.encode/1"
end

dependency = fn dep, env ->
  {app, opts} =
    case dep do
      {app, _requirement, opts} when is_list(opts) -> {app, opts}
      {app, opts} when is_list(opts) -> {app, opts}
      {app, _requirement} -> {app, []}
      app when is_atom(app) -> {app, []}
    end

  only = List.wrap(opts[:only])
  path =
    case opts[:path] do
      nil -> :null
      value -> value |> Path.expand(project_dir) |> Path.relative_to(project_dir)
    end

  %{
    "app" => Atom.to_string(app),
    "active" => only == [] or env in only,
    "env" => opts |> Keyword.get(:env, :prod) |> Atom.to_string(),
    "path" => path
  }
end

environments =
  [:dev, :test, :prod]
  |> Map.new(fn env ->
    value =
      Mix.Project.in_project(:once_native_project, project_dir, [env: env], fn _project ->
        config = Mix.Project.config()
        %{
          "app" => if(config[:app], do: Atom.to_string(config[:app]), else: :null),
          "apps_path" => config[:apps_path] || :null,
          "deps" => Enum.map(config[:deps] || [], &dependency.(&1, env)),
          "elixirc_paths" => config[:elixirc_paths] || ["lib"],
          "erlc_paths" => config[:erlc_paths] || ["src"]
        }
      end)

    {Atom.to_string(env), value}
  end)

environments |> :json.encode() |> IO.binwrite()
"""

def _mix_workspace_metadata(ctx):
    toolchain = _elixir_compiler_toolchain()
    project_dir = _mix_workspace_path(ctx, ".")
    content = host_command(
        [toolchain["elixir"], "-e", _mix_workspace_metadata_source(), "--", project_dir],
        env = _elixir_action_env(toolchain),
        cwd = project_dir,
    )
    return json_decode(content)

def _mix_workspace_path_target(ctx, dependency, environment):
    package = _elixir_workspace_source_root(ctx["label"]["package"], dependency["path"])
    return ("" if package == "." else package + "/") + "mix_application_" + environment

def _mix_workspace_project_sources(metadata):
    patterns = []
    for path in (metadata.get("elixirc_paths") or ["lib"]) + (metadata.get("erlc_paths") or ["src"]):
        normalized = path.replace("\\", "/")
        if normalized and not normalized.startswith("/") and normalized != ".." and not normalized.startswith("../") and "/../" not in normalized:
            pattern = normalized + "/**/*"
            if glob([pattern]):
                patterns.append(pattern)
    return _unique(patterns)

def _mix_workspace_relative_paths(ctx, paths):
    package = ctx["label"]["package"]
    prefix = package + "/" if package else ""
    return [
        path[len(prefix):] if prefix and path.startswith(prefix) else path
        for path in paths
    ]

def _mix_workspace_children(ctx, apps_path):
    normalized = (apps_path or "").replace("\\", "/")
    if not normalized or normalized.startswith("/") or normalized == ".." or normalized.startswith("../") or "/../" in normalized:
        fail(ctx["label"]["id"] + ": Mix umbrella apps_path must stay inside the adapted project")
    root = _mix_workspace_path(ctx, normalized)
    children = []
    for name in host_read_dir(root):
        manifest = root + "/" + name + "/mix.exs"
        if host_file_exists(manifest):
            package = _elixir_workspace_source_root(ctx["label"]["package"], normalized + "/" + name)
            children.append(("" if package == "." else package + "/") + "mix")
    if not children:
        fail(ctx["label"]["id"] + ": Mix umbrella has no child mix.exs files under `" + normalized + "`")
    return sorted(children)

def _mix_workspace_resolver(ctx):
    metadata = _mix_workspace_metadata(ctx)
    if metadata["dev"].get("app") == None:
        apps_path = metadata["dev"].get("apps_path")
        if not apps_path:
            fail(ctx["label"]["id"] + ": Mix project metadata has neither app nor apps_path")
        return {
            "targets": [],
            "roots": _mix_workspace_children(ctx, apps_path),
        }
    manifest = ctx["attrs"].get("manifest") or "mix.exs"
    lockfile = ctx["attrs"].get("lockfile") or "mix.lock"
    config = ["config/**/*.exs"]
    config_entry = "config/config.exs" if (ctx.get("files") or {}).get("config/config.exs") != None else ""
    has_lock = (ctx.get("files") or {}).get(lockfile) != None
    targets = []
    applications = {}
    dependency_targets = {}

    for environment in ["dev", "test", "prod"]:
        environment_metadata = metadata[environment]
        active_dependencies = [dependency for dependency in environment_metadata["deps"] if dependency.get("active")]
        external_dependencies = [dependency for dependency in active_dependencies if dependency.get("path") == None]
        path_dependencies = [dependency for dependency in active_dependencies if dependency.get("path") != None]
        if external_dependencies and not has_lock:
            fail(ctx["label"]["id"] + ": Mix project declares external dependencies but has no mix.lock; run `mix deps.get` before loading the Once graph")

        dependency_target = ""
        application_deps = []
        if has_lock:
            dependency_target = "mix_dependencies_" + environment
            dependency_attrs = {
                "manifest": manifest,
                "lockfile": lockfile,
                "resolver_inputs": [
                    manifest,
                    lockfile,
                    "config/**/*.exs",
                    "deps/*/mix.exs",
                    "deps/*/rebar.config",
                    "deps/*/rebar.config.script",
                ],
                "vendor_dir": "deps",
                "mix_env": environment,
                "dependency_mix_env": "prod",
                "config": config,
                "config_entry": config_entry,
                "target_prefix": "mix-" + environment,
            }
            targets.append({
                "name": dependency_target,
                "kind": "mix_dependencies",
                "srcs": dependency_attrs["resolver_inputs"],
                "attrs": dependency_attrs,
            })
            application_deps.append("./" + dependency_target)
        else:
            application_deps.extend([
                _mix_workspace_path_target(ctx, dependency, dependency["env"])
                for dependency in path_dependencies
            ])

        application_target = "mix_application_" + environment
        applications[environment] = application_target
        dependency_targets[environment] = dependency_target
        targets.append({
            "name": application_target,
            "kind": "mix_project",
            "deps": application_deps,
            "srcs": _mix_workspace_project_sources(environment_metadata),
            "attrs": {
                "app_name": environment_metadata["app"],
                "mix_env": environment,
                "manifest": manifest,
                "lockfile": lockfile,
                "config": config,
                "data": [".formatter.exs"],
                "priv": ["priv/**/*"],
            },
        })

    lint_tasks = [["deps.unlock", "--check-unused"]]
    format_task = ["format", "--check-formatted"]
    if not host_file_exists(_mix_workspace_path(ctx, ".formatter.exs")):
        format_task.extend(_mix_workspace_relative_paths(
            ctx,
            _unique([manifest] + glob([
                "config/**/*.exs",
                "lib/**/*.ex",
                "lib/**/*.exs",
                "test/**/*.exs",
            ])),
        ))
    lint_tasks.append(format_task)
    targets.append({
        "name": "mix_lint",
        "kind": "mix_project",
        "deps": ["./" + dependency_targets["dev"]] if dependency_targets["dev"] else [],
        "srcs": _mix_workspace_project_sources(metadata["dev"]),
        "attrs": {
            "app_name": metadata["dev"]["app"],
            "mix_env": "dev",
            "manifest": manifest,
            "lockfile": lockfile,
            "config": config,
            "data": [".formatter.exs"],
            "priv": ["priv/**/*"],
            "run_tasks": lint_tasks,
            "run_cacheable": True,
        },
    })
    if glob(["test/**/*"]):
        targets.append({
            "name": "mix_tests",
            "kind": "elixir_test",
            "deps": ["./" + applications["test"]],
            "srcs": ["test/**/*"],
            "attrs": {
                "lockfile": lockfile,
                "config": config,
                "data": ["test/**/*"],
            },
        })
    targets.append({
        "name": "mix_release",
        "kind": "mix_release",
        "deps": ["./" + applications["prod"]],
        "srcs": ["mix.exs"],
        "attrs": {
            "manifest": manifest,
            "lockfile": lockfile,
            "config": config,
            "data": ["priv/**/*"],
        },
    })
    return {
        "targets": targets,
        "roots": ["./" + applications["dev"]],
    }

def _mix_workspace_impl(ctx):
    return {
        "label_id": ctx["label"]["id"],
        "target_kind": "mix_workspace",
        "mix_workspace": True,
        "applications": ctx["deps"],
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
    attr("lockfile", "string", default = "mix.lock", docs = "Package-relative Mix lockfile staged when present so tests evaluate the compiled dependency identities.", configurable = False),
    attr("config", "list<string>", default = "[\"config/**/*.exs\"]", docs = "Package-relative config file globs included in the test action key.", configurable = False),
    attr("config_files", "list<string>", default = "[]", docs = "Buck-compatible alias for additional config file globs.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative data file globs available during tests.", configurable = False),
    attr("os_env", "map<string, string>", default = "{}", docs = "Buck-compatible environment variables exported before running tests.", configurable = False),
    attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited before explicit `env` values.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables exported before running tests.", configurable = False),
    attr("test_args", "list<string>", default = "[]", docs = "Additional arguments appended to the test runner.", configurable = False),
    attr("setup_tasks", "list<list<string>>", default = "[]", docs = "Ordered Mix commands run before the test task. Each nested list contains the task name followed by its exact arguments.", configurable = False),
    attr("elixir_opts", "list<string>", default = "[]", docs = "Bazel-compatible options passed to the direct Elixir interpreter before test files. Not supported with `mix_config`.", configurable = False),
    attr("setup", "string", default = "", docs = "Shell snippet run after the test environment is prepared and before ExUnit starts.", configurable = False),
    attr("no_start", "bool", default = "false", docs = "Pass `--no-start` to `mix test` when `mix_config` enables Mix mode.", configurable = False),
    attr("ez_deps", "list<string>", default = "[]", docs = "Reserved for Bazel-compatible archive dependencies.", configurable = False, implemented = False),
    attr("tools", "list<string>", default = "[]", docs = "Package-relative executable or support-file globs available to the test setup command.", configurable = False),
    attr("labels", "list<string>", default = "[]", docs = "Labels exposed through once_test_info for test discovery.", configurable = True),
    attr("timeout_ms", "int", docs = "Optional test timeout in milliseconds.", configurable = False),
    attr("cacheable", "bool", default = "true", docs = "Whether successful test results may be restored from the action cache. Disable this for tests that must exercise an external service on every run.", configurable = False),
]

_MIX_PROJECT_ATTRS = [
    attr("app_name", "string", docs = "Mix application name. Defaults to the target name with `-` and `.` rewritten as `_`.", configurable = False),
    attr("mix_env", "string", default = "prod", docs = "Mix environment used to load configuration and run the compiler pipeline.", configurable = False),
    attr("manifest", "string", default = "mix.exs", docs = "Package-relative Mix project manifest included in the compile action key.", configurable = False),
    attr("lockfile", "string", default = "mix.lock", docs = "Package-relative Mix lockfile staged when present.", configurable = False),
    attr("config", "list<string>", default = "[\"config/**/*.exs\"]", docs = "Package-relative configuration files included in the compile action key.", configurable = False),
    attr("config_files", "list<string>", default = "[]", docs = "Additional package-relative configuration files included in the compile action key.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative data files available to project compilers.", configurable = False),
    attr("priv", "list<string>", default = "[]", docs = "Package-relative private application files copied into the application output.", configurable = False),
    attr("resources", "list<string>", default = "[]", docs = "Additional package-relative private application files copied into the application output.", configurable = False),
    attr("include", "list<string>", default = "[\"include/**/*.hrl\"]", docs = "Package-relative Erlang header files copied into the application output.", configurable = False),
    attr("tools", "list<string>", default = "[]", docs = "Workspace files executed or read by custom project compilers.", configurable = False),
    attr("os_env", "map<string, string>", default = "{}", docs = "Environment variables exported before Mix compile and run commands.", configurable = False),
    attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited before explicit env values.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables exported before Mix compile and run commands.", configurable = False),
    attr("compile_args", "list<string>", default = "[]", docs = "Additional arguments passed to the native Mix compiler pipeline.", configurable = False),
    attr("run_task", "string", default = "", docs = "Mix task invoked by the run capability. Arguments after `--` are appended to this task. When omitted, the first argument after `--` selects the task.", configurable = False),
    attr("run_tasks", "list<list<string>>", default = "[]", docs = "Ordered Mix commands invoked by the run capability. Each nested list contains the task name followed by its exact arguments. Runtime arguments after `--` are rejected because their destination would be ambiguous.", configurable = False),
    attr("run_no_compile", "bool", default = "true", docs = "Pass `--no-compile` to the run task so it consumes the built application output.", configurable = False),
    attr("run_no_deps_check", "bool", default = "true", docs = "Pass `--no-deps-check` to the run task so it consumes the resolved dependency graph.", configurable = False),
    attr("run_args", "list<string>", default = "[]", docs = "Arguments passed to run_task before any arguments supplied after `--` on the command line.", configurable = False),
    attr("run_cacheable", "bool", default = "false", docs = "Whether a successful run task may be restored from the action cache. Keep servers and other interactive tasks uncached.", configurable = False),
]

_MIX_RELEASE_ATTRS = [
    attr("manifest", "string", default = "mix.exs", docs = "Package-relative Mix project manifest staged into the isolated release project.", configurable = False),
    attr("lockfile", "string", default = "mix.lock", docs = "Package-relative Mix lockfile staged when present so release tasks evaluate the same dependency identities as the compiled application.", configurable = False),
    attr("release_name", "string", default = "", docs = "Optional configured Mix release name. When omitted, Mix uses the project's default release.", configurable = False),
    attr("pre_tasks", "list<list<string>>", default = "[]", docs = "Ordered Mix commands run before assembly. Each nested list contains the task name followed by its exact arguments.", configurable = False),
    attr("release_args", "list<string>", default = "[]", docs = "Additional arguments passed to `mix release` after compilation and dependency checks are disabled.", configurable = False),
    attr("config", "list<string>", default = "[\"config/**/*.exs\"]", docs = "Package-relative configuration files staged into the isolated release project.", configurable = False),
    attr("config_files", "list<string>", default = "[]", docs = "Additional package-relative configuration files staged into the isolated release project.", configurable = False),
    attr("data", "list<string>", default = "[]", docs = "Package-relative project files staged for release tasks and assembly.", configurable = False),
    attr("tools", "list<string>", default = "[]", docs = "Package-relative executable files staged for release tasks.", configurable = False),
    attr("os_env", "map<string, string>", default = "{}", docs = "Environment variables exported before Mix release tasks.", configurable = False),
    attr("env_inherit", "list<string>", default = "[]", docs = "Host environment variable names inherited before explicit env values.", configurable = False),
    attr("env", "map<string, string>", default = "{}", docs = "Environment variables exported before Mix release tasks.", configurable = False),
    attr("cacheable", "bool", default = "true", docs = "Whether the assembled release may be restored from the action cache.", configurable = False),
]

_MIX_WORKSPACE_ATTRS = [
    attr("manifest", "string", default = "mix.exs", docs = "Package-relative Mix project file adapted into Once targets.", configurable = False),
    attr("lockfile", "string", default = "mix.lock", docs = "Package-relative Mix lockfile used when the project declares external dependencies.", configurable = False),
    attr("resolver_inputs", "list<string>", default = "[]", docs = "Project files supplied to native project resolution. Defaults to srcs when empty.", configurable = False),
]

_MIX_DEPENDENCIES_ATTRS = [
    attr("manifest", "string", default = "mix.exs", docs = "Package-relative Mix project manifest used to select the active dependency graph.", configurable = False),
    attr("lockfile", "string", default = "mix.lock", docs = "Package-relative Mix lockfile. Resolution fails when the file is absent, and never updates it.", configurable = False),
    attr("resolver_inputs", "list<string>", default = "[]", docs = "Package-relative text globs supplied to the resolver. Defaults to srcs when empty or omitted; set this when build sources include binary or large files.", configurable = False),
    attr("graph_file", "string", docs = "Optional checked-in JavaScript Object Notation snapshot of the active Mix dependency graph with exact manifest, lockfile, Mix environment, and resolver input bindings. When present, graph loading does not execute Mix.", configurable = False),
    attr("vendor_dir", "string", default = "deps", docs = "Package-relative directory containing dependency source trees already fetched from Hex or Git.", configurable = False),
    attr("path_dependencies", "map<string, string>", default = "{}", docs = "Map active Mix path dependency application names to first-party Once target references.", configurable = False),
    attr("mix_env", "string", default = "prod", docs = "Mix environment used when evaluating dependency declarations.", configurable = False),
    attr("dependency_mix_env", "string", default = "prod", docs = "Mix environment used inside each separately compiled dependency project.", configurable = False),
    attr("config", "list<string>", default = "[]", docs = "Root project configuration input globs made available while compiling each dependency.", configurable = False),
    attr("config_entry", "string", default = "", docs = "Root project configuration entry file loaded before dependency compilation. When set, it must match one of the expanded config inputs.", configurable = False),
    attr("config_target", "string", default = "host", docs = "Root Mix target used while evaluating configuration before dependency compilation.", configurable = False),
    attr("roots", "list<string>", default = "[]", docs = "Optional application or Hex package names exposed as direct roots. Defaults to direct dependencies selected by Mix.", configurable = False),
    attr("target_prefix", "string", default = "mix", docs = "Prefix for generated package target names. Use distinct values when one package declares dependency graphs for multiple Mix environments.", configurable = False),
    attr("_mix_resolved", "bool", default = "false", docs = "Resolver-owned marker proving the dependency set came from mix.lock.", configurable = False),
    attr("_mix_locked_roots", "list<string>", default = "[]", docs = "Resolver-owned synthetic target names attached to the aggregate provider.", configurable = False),
    attr("_mix_path_destinations", "map<string, string>", default = "{}", docs = "Resolver-owned workspace-relative source roots for mapped path dependencies.", configurable = False),
]

_MIX_PACKAGE_ATTRS = _ELIXIR_LIBRARY_ATTRS + [
    attr("_mix_locked", "bool", default = "false", docs = "Resolver-owned marker proving the synthetic target has locked identity metadata.", configurable = False),
    attr("_mix_local", "bool", default = "false", docs = "Resolver-owned marker identifying a workspace-local path dependency.", configurable = False),
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
    attr("_mix_root_config_entry", "string", default = "", docs = "Resolver-owned root configuration entry loaded before package compilation.", configurable = False),
    attr("_mix_root_config_env", "string", default = "prod", docs = "Resolver-owned root configuration environment used while reading compile configuration.", configurable = False),
    attr("_mix_root_config_target", "string", default = "host", docs = "Resolver-owned root configuration target used while reading compile configuration.", configurable = False),
]

mix_workspace = target_kind(
    docs = "Derives first-class Once dependency, application, lint, test, and release targets from a native Mix project.",
    attrs = _MIX_WORKSPACE_ATTRS,
    deps = [dep("deps", ["mix_project", "mix_workspace"], "Default development application or nested workspace emitted by native project discovery.")],
    providers = ["mix_workspace"],
    capabilities = [capability("build", [])],
    tools = _ELIXIR_TOOLS,
    examples = [
        example(
            "mix-workspace-native-project",
            name = "Mix native project seed",
            use_when = "Use this when a Mix project should derive build, lint, test, and release targets from mix.exs.",
        ),
    ],
    resolver = _mix_workspace_resolver,
    impl = _mix_workspace_impl,
)

mix = native_project(
    target_kind = "mix_workspace",
    name = "mix",
    target_name = "mix",
    docs = "Recognizes a native Mix project from mix.exs.",
    markers = ["mix.exs"],
    inputs = ["mix.lock", ".formatter.exs", "config/**/*.exs"],
    exclude = ["deps", "_build"],
    requires_tools = ["elixir"],
)

mix_dependencies = target_kind(
    docs = "Locked Mix dependency graph consumed by Elixir targets. Mix evaluates the project in the selected environment, mix.lock supplies immutable package identities, and Once emits one cacheable target per vendored package.",
    attrs = _MIX_DEPENDENCIES_ATTRS,
    deps = [dep("deps", ["elixir_app"], "Locked package roots generated from the active Mix dependency graph.")],
    providers = ["elixir_app", "mix_dependency_set"],
    capabilities = [capability("build", [])],
    tools = _ELIXIR_TOOLS,
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
    docs = "Synthetic vendored package emitted by mix_dependencies with source, version, checksums, native Mix or Rebar compilation, and dependency edges preserved from mix.lock.",
    attrs = _MIX_PACKAGE_ATTRS,
    deps = [dep("deps", ["elixir_app"], "Locked Mix packages available on the compile path.")],
    providers = ["elixir_app"],
    capabilities = [capability("build", ["bytecode"])],
    tools = _ELIXIR_TOOLS,
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

mix_project = target_kind(
    docs = "Compiles a first-party Mix project with its registered compiler pipeline while consuming separately cached Elixir application dependencies.",
    attrs = _MIX_PROJECT_ATTRS,
    deps = [dep("deps", ["elixir_app"], "Compiled Mix and Elixir applications loaded before the first-party compiler pipeline runs.")],
    providers = ["elixir_app", "mix_project"],
    capabilities = [
        capability("build", ["bytecode"]),
        capability("run", ["default"], ["bytecode"]),
    ],
    tools = _ELIXIR_TOOLS,
    examples = [
        example(
            "mix-project-minimal",
            name = "Minimal first-party Mix project",
            use_when = "Use this when a Mix project must run its registered compiler pipeline instead of direct Elixir compilation.",
        ),
    ],
    source_references = [
        source_reference(
            "Mix",
            "mix compile",
            "https://hexdocs.pm/mix/Mix.Tasks.Compile.html",
            "Use this when the project declares custom compilers or relies on native Mix application metadata.",
        ),
    ],
    impl = _mix_project_impl,
)

mix_release = target_kind(
    docs = "Assembles a Mix release from a compiled mix_project in an isolated staged project, with optional cacheable pre-release Mix tasks.",
    attrs = _MIX_RELEASE_ATTRS,
    deps = [dep("deps", ["mix_project"], "Exactly one Mix project and its transitive compiled applications.")],
    providers = ["mix_release"],
    capabilities = [capability("build", ["default", "release"])],
    tools = _ELIXIR_TOOLS,
    examples = [
        example(
            "mix-project-minimal",
            name = "Minimal Mix release",
            use_when = "Use this when a Mix application should be assembled into a runnable release directory.",
        ),
    ],
    source_references = [
        source_reference(
            "Mix",
            "mix release",
            "https://hexdocs.pm/mix/Mix.Tasks.Release.html",
            "Use this for release naming, runtime configuration, assembly options, and target-platform requirements.",
        ),
    ],
    impl = _mix_release_impl,
)

elixir_library = target_kind(
    docs = "Elixir code compiled with one target-level `elixirc` action and staged into cacheable bytecode plus OTP application metadata.",
    attrs = _ELIXIR_LIBRARY_ATTRS,
    deps = [dep("deps", ["elixir_app"], "Elixir applications available on the compile path.")],
    providers = ["elixir_app"],
    capabilities = [capability("build", ["bytecode"])],
    tools = _ELIXIR_TOOLS,
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
    docs = "Runs ExUnit tests against an elixir_app provider already compiled with mix_env = \"test\".",
    attrs = _ELIXIR_TEST_ATTRS,
    deps = [dep("deps", ["elixir_app"], "The test-environment Elixir application under test.")],
    providers = ["once_test_info"],
    capabilities = [capability("test", ["default", "test_results", "logs"])],
    tools = _ELIXIR_TOOLS,
    examples = [
        example(
            "elixir-test-minimal",
            name = "Minimal Elixir test",
            use_when = "Use this when an Elixir application should compile once and run ExUnit tests through Once.",
        ),
    ],
    impl = _elixir_test_impl,
)
