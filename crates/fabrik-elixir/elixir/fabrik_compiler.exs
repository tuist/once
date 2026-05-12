# Long-lived compile daemon. Started by `fabrik elixir-daemon`; serves
# JSON-line requests over a unix domain socket so per-target actions
# (`fabrik elixir-compile`) can reuse one warm BEAM instead of paying
# startup costs each time elixirc would normally fork.
#
# Wire format: one JSON object per line, both directions.
#   Request:  {"v":1,"id":N,"cwd":ABS,"out":REL,"pa":[REL...],"srcs":[REL...]}
#   Response: {"v":1,"id":N,"ok":true}
#         or  {"v":1,"id":N,"ok":false,"error":STRING}
#
# `cwd` is the absolute workspace root; every other path is workspace
# relative. The daemon resolves paths against `cwd` so it doesn't depend
# on its own working directory.

defmodule Fabrik.Compiler do
  @moduledoc false

  # One request at a time: Code.prepend_path/1 mutates the global BEAM
  # code path, so concurrent compiles with disjoint dep sets would race.
  # Serializing through a single process is the honest fix; future work
  # can isolate via spawned slave nodes or parse-transform sandboxes.
  def serve(socket, buf \\ "") do
    case :binary.match(buf, "\n") do
      {pos, _} ->
        <<line::binary-size(pos), "\n", rest::binary>> = buf
        reply = handle(line)
        :ok = :socket.send(socket, reply <> "\n")
        serve(socket, rest)

      :nomatch ->
        case :socket.recv(socket, 0, :infinity) do
          {:ok, more} -> serve(socket, buf <> more)
          _ -> :ok
        end
    end
  end

  defp handle(line) do
    try do
      req = :json.decode(line)
      do_compile(req)
    catch
      kind, reason ->
        encode(%{
          "v" => 1,
          "id" => 0,
          "ok" => false,
          "error" => Exception.format(kind, reason, __STACKTRACE__)
        })
    end
  end

  defp do_compile(req) do
    cwd = Map.fetch!(req, "cwd")
    id = Map.fetch!(req, "id")
    out_rel = Map.fetch!(req, "out")
    pa_rel = Map.get(req, "pa", [])
    srcs_rel = Map.fetch!(req, "srcs")

    out_abs = Path.join(cwd, out_rel)
    pa_abs = Enum.map(pa_rel, &Path.join(cwd, &1))
    srcs_abs = Enum.map(srcs_rel, &Path.join(cwd, &1))

    File.mkdir_p!(out_abs)

    # Prepend dep ebin dirs so macros, behaviours, and `use` references
    # resolve during this compile, then back them out so the next job
    # starts from a known code path.
    Enum.each(pa_abs, &Code.prepend_path/1)

    try do
      modules = Enum.flat_map(srcs_abs, &Code.compile_file(&1))
      Enum.each(modules, fn {mod, beam} ->
        File.write!(Path.join(out_abs, "#{mod}.beam"), beam)
      end)
      encode(%{"v" => 1, "id" => id, "ok" => true})
    rescue
      e ->
        encode(%{
          "v" => 1,
          "id" => id,
          "ok" => false,
          "error" => Exception.format(:error, e, __STACKTRACE__)
        })
    after
      Enum.each(pa_abs, &Code.delete_path/1)
    end
  end

  defp encode(map), do: IO.iodata_to_binary(:json.encode(map))
end

defmodule Fabrik.CompilerServer do
  @moduledoc false

  def main([socket_path | _]) do
    # Stale socket files from a crashed prior daemon would block the
    # bind below; the right behavior is to overwrite, matching what
    # most unix daemons do on startup.
    _ = File.rm(socket_path)
    _ = File.mkdir_p(Path.dirname(socket_path))

    {:ok, listen} = :socket.open(:local, :stream)
    :ok = :socket.bind(listen, %{family: :local, path: socket_path})
    :ok = :socket.listen(listen)
    IO.puts(:stderr, "fabrik elixir-daemon: listening on #{socket_path}")
    accept_loop(listen)
  end

  defp accept_loop(listen) do
    case :socket.accept(listen) do
      {:ok, sock} ->
        spawn(fn -> Fabrik.Compiler.serve(sock) end)
        accept_loop(listen)

      {:error, _} ->
        :ok
    end
  end
end

Fabrik.CompilerServer.main(System.argv())
