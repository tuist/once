# Long-lived compile daemon. Started by `fabrik elixir-daemon`; serves
# JSON-line requests over a unix domain socket so per-target actions
# (`fabrik elixir-compile`) can reuse one warm BEAM instead of paying
# startup costs each time elixirc would normally fork.
#
# Wire format: one JSON object per line, both directions.
#   Request:  {"v":1,"id":N,"cwd":ABS,"out":REL,"pa":[REL...],"srcs":[REL...]}
#   Response: {"v":1,"id":N,"ok":true}
#         or  {"v":1,"id":N,"ok":false,"error":STRING}
#         or  {"v":1,"id":N,"ok":false,"error":STRING,"retryable":true}
#
# `cwd` is the absolute workspace root; every other path is workspace
# relative. The daemon resolves paths against `cwd` so it doesn't depend
# on its own working directory.
#
# Concurrency + backpressure: many fabrik invocations can hit the same
# daemon simultaneously (the socket is a per-user resource shared
# across workspaces). I/O for each connection runs in its own Erlang
# process, but `Code.compile_file/1` and `Code.prepend_path/1` mutate
# VM-global state; letting two compiles overlap would race the code
# path and the loaded-modules table. Every job therefore funnels
# through `Fabrik.CompileWorker`, a single GenServer that processes
# one request at a time.
#
# To keep that queue bounded under runaway concurrency, an atomic
# counter caps the number of in-flight + pending jobs. When the cap
# is exceeded the daemon replies with `retryable: true` immediately
# and the client (`fabrik elixir-compile`) falls back to spawning
# `elixirc` directly for that one action. The cap defaults to the
# BEAM scheduler count - the same value fabrik's runner uses for its
# `ELIXIR_COMPILE_SLOT` pool - so the daemon and the scheduler agree
# on how many concurrent compiles the host can absorb. Overrides come
# from the `FABRIK_ELIXIR_DAEMON_MAX_QUEUE` env variable, set at
# daemon-start time, and should be matched on the fabrik side with the
# equivalent `with_slot_limit` if you tune one.

defmodule Fabrik.CompileWorker do
  @moduledoc false
  use GenServer

  def start_link(_opts) do
    GenServer.start_link(__MODULE__, nil, name: __MODULE__)
  end

  # Called from each per-connection serve process. `:infinity` because
  # the compile itself owns the timeout via the client's socket
  # read_timeout; bounding it here would create a second deadline that
  # could fire before the client gives up.
  def compile(request) do
    GenServer.call(__MODULE__, {:compile, request}, :infinity)
  end

  @impl true
  def init(_), do: {:ok, nil}

  @impl true
  def handle_call({:compile, request}, _from, state) do
    {:reply, Fabrik.Compiler.do_compile(request), state}
  end
end

defmodule Fabrik.Compiler do
  @moduledoc false

  # The counters table is created with size 1 in main/1 and shared
  # across all serve processes via `:persistent_term`. Atomic increments
  # make the cap check thread-safe without serializing the I/O paths.
  @counter_key {__MODULE__, :inflight}
  @max_key {__MODULE__, :max_queue}

  def init_throttle(max_queue) do
    counters = :counters.new(1, [:atomics])
    :persistent_term.put(@counter_key, counters)
    :persistent_term.put(@max_key, max_queue)
  end

  defp counters_ref, do: :persistent_term.get(@counter_key)
  defp max_queue, do: :persistent_term.get(@max_key)

  # Returns :ok when the slot was acquired, {:busy, current} otherwise.
  # Either way the caller must call `release/0` if (and only if) it
  # got :ok, mirroring the standard try-acquire pattern.
  defp acquire do
    ref = counters_ref()
    :counters.add(ref, 1, 1)
    current = :counters.get(ref, 1)

    if current > max_queue() do
      :counters.sub(ref, 1, 1)
      {:busy, current - 1}
    else
      :ok
    end
  end

  defp release do
    :counters.sub(counters_ref(), 1, 1)
  end

  # One per-connection process per TCP accept. It buffers bytes,
  # decodes line-delimited JSON requests, applies backpressure, and
  # forwards admitted jobs to the serializing worker. The actual
  # compile happens inside the worker so multiple connections never
  # race on the global code path.
  def serve(socket, buf \\ "") do
    case :binary.match(buf, "\n") do
      {pos, _} ->
        <<line::binary-size(pos), "\n", rest::binary>> = buf
        reply = dispatch(line)
        :ok = :socket.send(socket, reply <> "\n")
        serve(socket, rest)

      :nomatch ->
        case :socket.recv(socket, 0, :infinity) do
          {:ok, more} -> serve(socket, buf <> more)
          _ -> :ok
        end
    end
  end

  defp dispatch(line) do
    try do
      request = :json.decode(line)
      run_with_backpressure(request)
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

  defp run_with_backpressure(request) do
    case acquire() do
      :ok ->
        try do
          Fabrik.CompileWorker.compile(request)
        after
          release()
        end

      {:busy, current} ->
        encode(%{
          "v" => 1,
          "id" => Map.get(request, "id", 0),
          "ok" => false,
          "error" =>
            "fabrik elixir-daemon at capacity (#{current}/#{max_queue()} in-flight jobs); " <>
              "client should fall back to direct elixirc spawn for this action",
          "retryable" => true
        })
    end
  end

  # The actual compile. Runs inside the CompileWorker, so the code
  # path mutations are safe: only one of these is in flight at a time.
  def do_compile(req) do
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
    # starts from a known code path. Safe to mutate globally because
    # the worker enforces one-at-a-time execution.
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

    # Cap pending+in-flight jobs to a multiple of scheduler count so
    # one host saturating the daemon doesn't ripple into FD exhaustion
    # or unbounded mailbox growth. Configurable via env for cases that
    # legitimately want more headroom.
    max_queue = read_max_queue()
    Fabrik.Compiler.init_throttle(max_queue)

    # Start the serializing worker before we accept any traffic so
    # the first request never races a half-initialized GenServer.
    # Linking it to this process means a worker crash takes the
    # daemon down loudly instead of hanging future requests.
    {:ok, _pid} = Fabrik.CompileWorker.start_link([])

    {:ok, listen} = :socket.open(:local, :stream)
    :ok = :socket.bind(listen, %{family: :local, path: socket_path})
    # Default backlog (5) is too small once several fabrik invocations
    # connect simultaneously; the kernel refuses the overflow with
    # ECONNREFUSED. 128 matches the usual SOMAXCONN on Linux/macOS and
    # is more than any realistic build's fan-out.
    :ok = :socket.listen(listen, 128)
    IO.puts(:stderr, "fabrik elixir-daemon: listening on #{socket_path} (max_queue=#{max_queue})")
    accept_loop(listen)
  end

  defp accept_loop(listen) do
    case :socket.accept(listen) do
      {:ok, sock} ->
        # Per-connection processes handle I/O concurrently; they all
        # serialize on Fabrik.CompileWorker for the actual compile.
        spawn(fn -> Fabrik.Compiler.serve(sock) end)
        accept_loop(listen)

      {:error, _} ->
        :ok
    end
  end

  defp read_max_queue do
    default = :erlang.system_info(:schedulers_online)

    case System.get_env("FABRIK_ELIXIR_DAEMON_MAX_QUEUE") do
      nil ->
        default

      value ->
        case Integer.parse(value) do
          {n, _} when n > 0 -> n
          _ -> default
        end
    end
  end
end

Fabrik.CompilerServer.main(System.argv())
