defmodule OnceSiteWeb.Docs.HTML do
  @moduledoc """
  HTML post-processing for rendered documentation.

  Works on the HTML **string** (not a parsed Floki tree) for everything that
  touches code, because parsing and re-serializing collapses the significant
  whitespace between syntax-highlighting spans. Only table wrapping uses Floki,
  and it first swaps code-window contents for placeholders so they are never
  parsed. This mirrors the reference docs pipeline.
  """

  @icons_path Path.join([File.cwd!(), "deps", "noora", "lib", "noora", "icons"])
  @copy_icon @icons_path |> Path.join("copy.svg") |> File.read!() |> String.trim()
  @copy_check_icon @icons_path |> Path.join("copy-check.svg") |> File.read!() |> String.trim()
  @status_icons %{
    "success" => @icons_path |> Path.join("circle-check.svg") |> File.read!() |> String.trim(),
    "information" =>
      @icons_path |> Path.join("alert-circle.svg") |> File.read!() |> String.trim(),
    "warning" => @icons_path |> Path.join("alert-triangle.svg") |> File.read!() |> String.trim(),
    "error" => @icons_path |> Path.join("alert-triangle.svg") |> File.read!() |> String.trim()
  }

  @code_block_regex ~r/<pre[^>]*><code(?:[^>]*class="language-(\w+)")?[^>]*>(.*?)<\/code><\/pre>/s
  @alert_regex ~r/<div class="noora-alert-src" data-status="([^"]*)" data-title="([^"]*)">(.*?)<\/div>/s
  @heading_regex ~r/<h([2-4]) id="([^"]*)">(.*?)<a[^>]*class="anchor"[^>]*><\/a><\/h\1>/s
  @code_window_content_regex ~r/(<div data-part="code"><code>)(.*?)(<\/code><\/div>)/s

  @doc "Turn `noora-alert-src` placeholders into Noora alert markup."
  def build_alerts(html) do
    Regex.replace(@alert_regex, html, fn _full, status, title, body ->
      icon = Map.get(@status_icons, status, @status_icons["information"])

      ~s(<div class="noora-alert" data-type="primary" data-status="#{status}" data-size="large">) <>
        ~s(<div data-part="icon">#{icon}</div>) <>
        ~s(<div data-part="column"><span data-part="title">#{title}</span>) <>
        ~s(<div data-part="description">#{body}</div></div></div>)
    end)
  end

  @doc "Wrap `<pre><code>` blocks in the Noora code-window chrome."
  def wrap_code_blocks(html) do
    Regex.replace(@code_block_regex, html, fn _full, language, code ->
      ~s(<div data-part="code-window"><div data-part="bar">) <>
        ~s(<div data-part="language">#{language}</div>) <>
        ~s(<div data-part="copy"><span data-part="copy-icon">#{@copy_icon}</span>) <>
        ~s(<span data-part="copy-check-icon">#{@copy_check_icon}</span></div></div>) <>
        ~s(<template data-part="copy-source">#{copy_source(code)}</template>) <>
        ~s(<div data-part="code"><code>#{code}</code></div></div>)
    end)
  end

  @doc "Give headings a hover `#` anchor link."
  def add_heading_anchors(html) do
    Regex.replace(@heading_regex, html, fn _full, level, id, text ->
      ~s(<h#{level} id="#{id}"><a class="heading-anchor" id="#{id}" href="##{id}">) <>
        ~s(<span data-part="heading-text">#{text}</span><span data-part="hash">#</span></a></h#{level}>)
    end)
  end

  @doc "Rewrite internal `/guide` and `/reference` links under `/docs`."
  def rewrite_links(html) do
    Regex.replace(~r/href="(\/(?:guide|reference)(?:\/[^"]*)?)"/, html, ~s(href="/docs\\1"))
  end

  @doc "Wrap `<table>` nodes with the NooraTable scroll structure (Floki, code protected)."
  def wrap_tables(html) do
    {html, protected} = protect_code_contents(html)

    {tree, _index} =
      html
      |> Floki.parse_fragment!()
      |> Floki.traverse_and_update(0, &wrap_table_node/2)

    tree
    |> Floki.raw_html()
    |> restore(protected)
  end

  # --- code source (for the copy button) ---

  defp copy_source(code) do
    code
    |> then(&Regex.replace(~r/<[^>]*>/, &1, ""))
    |> decode_entities()
    |> String.trim()
    |> html_escape()
  end

  defp decode_entities(text) do
    Regex.replace(~r/&(?:#x?[0-9A-Fa-f]+|[A-Za-z][A-Za-z0-9]+);/, text, fn entity ->
      case Floki.Entities.decode(entity) do
        {:ok, decoded} -> decoded
        _ -> entity
      end
    end)
  end

  defp html_escape(text), do: text |> Phoenix.HTML.html_escape() |> Phoenix.HTML.safe_to_string()

  # --- table wrapping with code protection ---

  defp protect_code_contents(html) do
    {html, protected, _index} =
      Regex.scan(@code_window_content_regex, html, return: :index)
      |> Enum.reverse()
      |> Enum.reduce({html, [], 0}, fn [_full, _open, {c_start, c_len}, _close],
                                       {html, protected, index} ->
        content = binary_part(html, c_start, c_len)
        placeholder = "__ONCE_DOCS_CODE_#{index}__"

        html =
          binary_part(html, 0, c_start) <>
            placeholder <>
            binary_part(html, c_start + c_len, byte_size(html) - c_start - c_len)

        {html, [{placeholder, content} | protected], index + 1}
      end)

    {html, protected}
  end

  defp restore(html, protected) do
    Enum.reduce(protected, html, fn {placeholder, content}, html ->
      String.replace(html, placeholder, content)
    end)
  end

  defp wrap_table_node({"table", attrs, children}, index) do
    {
      {"div",
       [{"id", "docs-table-#{index}"}, {"class", "noora-table"}, {"phx-hook", "NooraTable"}],
       [
         {"div", [{"data-part", "scroll-container"}], [{"table", attrs, children}]},
         {"div", [{"data-part", "scrollbar"}, {"aria-hidden", "true"}],
          [{"div", [{"data-part", "scrollbar-content"}], []}]},
         {"div", [{"data-part", "overlay-scrollbar"}, {"aria-hidden", "true"}],
          [{"div", [{"data-part", "overlay-thumb"}], []}]}
       ]},
      index + 1
    }
  end

  defp wrap_table_node(node, index), do: {node, index}
end
