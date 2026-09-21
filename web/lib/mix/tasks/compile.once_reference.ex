defmodule Mix.Tasks.Compile.OnceReference do
  @moduledoc """
  Generates the command reference from the `once` binary and the event
  reference from the versioned protocol definition.

  The command `once reference --out priv/docs/reference` walks the real command
  tree and writes one markdown page per command plus `mcp/tools.md`, so the
  published reference never drifts from the code. Running this as a compiler
  (rather than a mix alias) means it fires for every `mix compile` invocation:
  dev iex, `phx.server`, the test suite, and the release build.

  The `once` binary is resolved in order:

    1. `$ONCE_BIN` (set in the Docker build, which copies a release binary in).
    2. A binary at `../target/{release,debug}/once` relative to the app.
    3. `cargo run -p once-cli --` from the workspace root (dev fallback).

  The event page is compared with the protocol definition on every compile.
  Command generation is skipped when its output is newer than command sources.
  """
  use Mix.Task.Compiler

  @recursive false

  @out "priv/docs/reference"
  @marker Path.join(@out, "cli/index.md")
  @events_marker Path.join(@out, "events/index.md")
  @event_definition Path.expand("../crates/once-events-client/proto/once/events/v1/events.proto")

  @impl true
  def run(_args) do
    generate_events_if_stale()

    if stale_cli?() do
      generate()
    else
      {:noop, []}
    end
  end

  defp generate_events_if_stale do
    definition = File.read!(@event_definition)
    generated = render_event_definition(definition)
    output = Path.expand(@events_marker)

    if not File.exists?(output) or File.read!(output) != generated do
      File.mkdir_p!(Path.dirname(output))
      File.write!(output, generated)
    end
  end

  defp render_event_definition(source) do
    header = [
      "# Live run event protocol",
      "",
      "This reference is generated from the versioned wire definition during the documentation build. It describes the service, event variants, fields, and enum values shipped by Once.",
      "",
      "The client publishes ordered batches and the service acknowledges the contiguous durable frontier. The server may return a dashboard link after run creation.",
      ""
    ]

    {lines, _, _, _} =
      source
      |> String.split("\n")
      |> Enum.reduce({header, nil, 0, []}, &render_definition_line/2)

    String.trim_trailing(Enum.join(lines, "\n")) <> "\n"
  end

  defp render_definition_line(raw, {lines, section, depth, comments}) do
    case String.trim(raw) do
      "//" <> comment ->
        {lines, section, depth, comments ++ [String.trim(comment)]}

      "" ->
        {lines, section, depth, []}

      text ->
        render_code_line(text, {lines, section, depth, comments})
    end
  end

  defp render_code_line(text, state) do
    [code | inline] = String.split(text, "//", parts: 2)
    code = String.trim(code)
    note = inline |> List.first("") |> String.trim()

    case Regex.run(~r/^(service|message|enum)\s+(\w+)\s*\{/, code) do
      [_, kind, name] -> render_definition_header(code, kind, name, state)
      _ -> render_definition_member(code, note, state)
    end
  end

  defp render_definition_header(code, kind, name, {lines, _section, depth, comments}) do
    description = if comments == [], do: [], else: [Enum.join(comments, " "), ""]

    heading =
      ["## #{kind} `#{name}`", ""] ++
        description ++
        ["| Name | Type | Number | Description |", "| --- | --- | ---: | --- |"]

    inline_fields =
      code
      |> String.split("{", parts: 2)
      |> List.last()
      |> String.split("}")
      |> hd()
      |> String.split(";", trim: true)
      |> Enum.map(&render_field(&1, kind, ""))
      |> Enum.reject(&is_nil/1)

    new_depth = depth + brace_delta(code)

    {lines ++ heading ++ inline_fields ++ if(new_depth == 0, do: [""], else: []),
     if(new_depth != 0, do: kind), new_depth, []}
  end

  defp render_definition_member(code, note, {lines, section, depth, comments}) do
    entry = definition_entry(code, note, section, comments)
    validate_definition_entry(code, section, entry)
    new_depth = max(0, depth + brace_delta(code))
    end_lines = if depth > 0 and new_depth == 0, do: [""], else: []

    {lines ++ if(entry, do: [entry], else: []) ++ end_lines, if(new_depth != 0, do: section),
     new_depth, []}
  end

  defp definition_entry("rpc " <> call, note, "service", _comments),
    do: "| `rpc #{String.trim_trailing(call, ";")}` | call | | #{note} |"

  defp definition_entry("oneof " <> field, _note, _section, _comments),
    do: "| *#{field |> String.trim_trailing("{") |> String.trim()}* | one of | | |"

  defp definition_entry(code, note, section, comments) when section in ["message", "enum"],
    do: render_field(code, section, Enum.join(comments ++ [note], " "))

  defp definition_entry(_code, _note, _section, _comments), do: nil

  defp validate_definition_entry(code, section, entry) do
    if section in ["message", "enum"] and String.contains?(code, "=") and is_nil(entry) and
         not String.starts_with?(code, "reserved ") do
      Mix.raise("Unsupported protocol field in generated reference: #{code}")
    end
  end

  defp render_field(code, "enum", note) do
    case Regex.run(~r/^\s*(\w+)\s*=\s*(\d+)/, code) do
      [_, name, number] -> "| `#{name}` | value | #{number} | #{escape_cell(note)} |"
      _ -> nil
    end
  end

  defp render_field(code, _, note) do
    case Regex.run(
           ~r/^\s*((?:(?:repeated|optional)\s+)?(?:map<[^>]+>|\w+))\s+(\w+)\s*=\s*(\d+)/,
           code
         ) do
      [_, type, name, number] -> "| `#{name}` | `#{type}` | #{number} | #{escape_cell(note)} |"
      _ -> nil
    end
  end

  defp escape_cell(value), do: String.replace(String.trim(value), "|", "\\|")

  defp brace_delta(code),
    do:
      (String.graphemes(code) |> Enum.filter(&(&1 == "{")) |> length()) -
        (String.graphemes(code) |> Enum.filter(&(&1 == "}")) |> length())

  defp generate do
    out = Path.expand(@out)

    case command(out) do
      :unavailable ->
        Mix.shell().error(
          "[once_reference] no `once` binary and no cargo found; skipping CLI reference generation. " <>
            "Set ONCE_BIN or install the Rust toolchain to generate it."
        )

        {:noop, []}

      {source, cmd, args} ->
        File.rm_rf!(Path.join(out, "cli"))
        Mix.shell().info("Generating CLI reference (#{cmd})")

        case System.cmd(cmd, args, stderr_to_stdout: true) do
          {_output, 0} ->
            {:ok, []}

          {output, status} when source == :binary ->
            # An explicit binary (ONCE_BIN / target/) is expected to work, e.g.
            # in the release image, so a failure is fatal.
            Mix.raise("`once reference` failed with status #{status}:\n#{output}")

          {output, _status} ->
            # The cargo fallback is a convenience; if the workspace can't build
            # (e.g. missing system libraries in a CI job that doesn't need the
            # generated pages), skip rather than fail the build.
            Mix.shell().error(
              "[once_reference] cargo build failed; skipping CLI reference generation:\n#{output}"
            )

            {:noop, []}
        end
    end
  end

  # Absolute-path binaries (ONCE_BIN, target/) are called directly; the cargo
  # fallback runs from the workspace root so it resolves the workspace crate.
  defp command(out) do
    case once_binary() do
      {:binary, bin} ->
        {:binary, bin, ["reference", "--out", out]}

      :unavailable ->
        :unavailable

      :cargo ->
        {:cargo, "cargo",
         [
           "run",
           "--quiet",
           "--manifest-path",
           Path.expand("../Cargo.toml"),
           "-p",
           "once-cli",
           "--",
           "reference",
           "--out",
           out
         ]}
    end
  end

  defp once_binary do
    env = System.get_env("ONCE_BIN")

    cond do
      env not in [nil, ""] -> {:binary, env}
      File.exists?(target_bin("release")) -> {:binary, target_bin("release")}
      File.exists?(target_bin("debug")) -> {:binary, target_bin("debug")}
      System.find_executable("cargo") -> :cargo
      true -> :unavailable
    end
  end

  defp target_bin(profile), do: Path.expand("../target/#{profile}/once")

  # Regenerate when the marker is missing or older than the newest source file
  # in the CLI crate (where the command tree and help text live).
  defp stale_cli? do
    source_mtime = newest_source_mtime()

    case File.stat(Path.expand(@marker), time: :posix) do
      {:ok, %File.Stat{mtime: mtime}} -> mtime < source_mtime
      _ -> true
    end
  end

  defp newest_source_mtime do
    Path.expand("../crates/once-cli/src")
    |> Path.join("**/*.rs")
    |> Path.wildcard()
    |> Enum.map(fn path ->
      case File.stat(path, time: :posix) do
        {:ok, %File.Stat{mtime: mtime}} -> mtime
        _ -> 0
      end
    end)
    |> Enum.max(fn -> 0 end)
  end
end
