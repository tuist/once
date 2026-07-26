defmodule Mix.Tasks.Compile.OnceReference do
  @moduledoc """
  Generates the CLI and MCP-tools reference documentation from the `once` binary.

  The command `once reference --out priv/docs/reference` walks the real command
  tree and writes one markdown page per command plus `mcp/tools.md`, so the
  published reference never drifts from the code. Running this as a compiler
  (rather than a mix alias) means it fires for every `mix compile` invocation:
  dev iex, `phx.server`, the test suite, and the release build.

  The `once` binary is resolved in order:

    1. `$ONCE_BIN` (set in the Docker build, which copies a release binary in).
    2. A binary at `../target/{release,debug}/once` relative to the app.
    3. `cargo run -p once-cli --` from the workspace root (dev fallback).

  Generation is skipped when the output is newer than the CLI crate sources.
  """
  use Mix.Task.Compiler

  @recursive false

  @out "priv/docs/reference"
  @marker Path.join(@out, "cli/index.md")

  @impl true
  def run(_args) do
    if stale?() do
      generate()
    else
      {:noop, []}
    end
  end

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
  defp stale? do
    marker = Path.expand(@marker)

    case File.stat(marker, time: :posix) do
      {:ok, %File.Stat{mtime: mtime}} -> mtime < newest_source_mtime()
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
