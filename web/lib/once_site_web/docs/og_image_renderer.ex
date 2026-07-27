defmodule OnceSiteWeb.Docs.OgImageRenderer do
  @moduledoc """
  Build-time renderer that screenshots OG card HTML to a JPG using a headless
  Chrome pool (Browse + BrowseChrome, via Carta). Rendering is best-effort: if
  Chrome cannot start, `start/0` returns `:error` and the generation task skips
  images rather than failing the build.
  """

  @pool __MODULE__.Pool

  @doc "Start the browser pool. Returns `{:ok, renderer}` or `{:error, reason}`."
  def start(pool_size \\ 2) do
    case Browse.start_link(@pool, implementation: BrowseChrome.Browser, pool_size: pool_size) do
      {:ok, pid} -> {:ok, %{pid: pid, pool: @pool}}
      {:error, reason} -> {:error, reason}
    end
  end

  @doc "Render `html` to a JPEG binary."
  def render(%{pool: pool}, html) do
    Carta.render(pool, html, width: 1920, height: 1080, quality: 90)
  rescue
    exception -> {:error, {exception.__struct__, Exception.message(exception)}}
  catch
    :exit, reason -> {:error, {:exit, reason}}
  end

  @doc "Stop the browser pool."
  def stop(%{pid: pid}) do
    if Process.alive?(pid), do: GenServer.stop(pid, :normal, 5_000)
  catch
    :exit, _ -> :ok
  end
end
