defmodule OnceSiteWeb.Docs.Cache do
  @moduledoc """
  Caches rendered documentation pages keyed by file path and modification time.

  Rendering markdown through MDEx and Floki is not free, so each page is
  rendered once and stored in an ETS table. The cached entry carries the file's
  mtime; when the file changes on disk (e.g. while editing docs in dev) the page
  is re-rendered transparently, so no manual invalidation is needed.
  """
  use GenServer

  alias OnceSiteWeb.Docs.Markdown

  @table __MODULE__

  def start_link(opts),
    do: GenServer.start_link(__MODULE__, :ok, Keyword.put(opts, :name, __MODULE__))

  @doc "Return the rendered page for `file`, rendering and caching on a miss."
  @spec get(Path.t()) :: Markdown.t()
  def get(file) do
    mtime = mtime(file)

    case :ets.lookup(@table, file) do
      [{^file, ^mtime, page}] -> page
      _ -> GenServer.call(__MODULE__, {:render, file, mtime})
    end
  end

  @impl true
  def init(:ok) do
    :ets.new(@table, [:named_table, :set, :public, read_concurrency: true])
    {:ok, %{}}
  end

  @impl true
  def handle_call({:render, file, mtime}, _from, state) do
    # Re-check under the lock so concurrent misses render only once.
    page =
      case :ets.lookup(@table, file) do
        [{^file, ^mtime, page}] ->
          page

        _ ->
          page = file |> File.read!() |> Markdown.render()
          :ets.insert(@table, {file, mtime, page})
          page
      end

    {:reply, page, state}
  end

  defp mtime(file) do
    case File.stat(file, time: :posix) do
      {:ok, %File.Stat{mtime: mtime}} -> mtime
      _ -> 0
    end
  end
end
