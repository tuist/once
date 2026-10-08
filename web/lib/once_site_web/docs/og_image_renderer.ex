defmodule OnceSiteWeb.Docs.OgImageRenderer do
  @moduledoc """
  Build-time renderer that screenshots OG card HTML to a JPG using a headless
  Chrome pool (Browse + BrowseChrome). Rendering is best-effort: if
  Chrome cannot start, `start/0` returns `:error` and the generation task skips
  images rather than failing the build.
  """

  alias BrowseChrome.CDP
  alias BrowseChrome.Chrome
  alias OnceSiteWeb.Docs.OgImage

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
    with {:ok, path} <- Briefly.create(extname: ".html") do
      render_file(pool, path, html)
    end
  rescue
    exception -> {:error, {exception.__struct__, Exception.message(exception)}}
  catch
    :exit, reason -> {:error, {:exit, reason}}
  end

  defp render_file(pool, path, html) do
    with :ok <- File.write(path, html) do
      Browse.checkout(pool, &render_browser(&1, path))
    end
  after
    File.rm(path)
  end

  defp render_browser(%Browse{state: browser}, path) do
    with {:ok, ws_url} <- Chrome.ws_url(browser) do
      CDP.with_session(ws_url, &capture(&1, path))
    end
  end

  defp capture(session, path) do
    # Chrome resets emulation when the CDP connection closes. Keep setup and
    # capture on one connection and use CSS pixels for the advertised dimensions.
    with {:ok, _} <-
           CDP.command(session, "Emulation.setDeviceMetricsOverride", %{
             width: OgImage.width(),
             height: OgImage.height(),
             deviceScaleFactor: 1,
             mobile: false
           }),
         :ok <- CDP.navigate(session, "file://#{path}"),
         :ok <- wait_for_fonts(session),
         {:ok, data} <- CDP.capture_screenshot(session, "jpeg", 95) do
      Base.decode64(data)
    end
  end

  defp wait_for_fonts(session) do
    case CDP.command(session, "Runtime.evaluate", %{
           expression: "document.fonts.ready.then(() => true)",
           awaitPromise: true,
           returnByValue: true
         }) do
      {:ok, %{"result" => %{"value" => true}}} -> :ok
      {:ok, result} -> {:error, {:font_load_failed, result}}
      {:error, _} = error -> error
    end
  end

  @doc "Stop the browser pool."
  def stop(%{pid: pid}) do
    if Process.alive?(pid), do: GenServer.stop(pid, :normal, 5_000)
  catch
    :exit, _ -> :ok
  end
end
