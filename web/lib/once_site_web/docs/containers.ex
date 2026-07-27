defmodule OnceSiteWeb.Docs.Containers do
  @moduledoc """
  Rewrites VitePress `:::` containers into HTML that survives MDEx.

    * `::: tip Title` ... `:::` becomes a `noora-alert-src` placeholder div whose
      inner content stays markdown; `OnceSiteWeb.Docs.HTML.build_alerts/1` later
      turns it into a real Noora alert.
    * `::: code-group` becomes the Noora code-group markup (tabbed panels), each
      panel holding a `<pre><code>` that the code-window wrapper picks up.
  """

  @icons_path Path.join([File.cwd!(), "deps", "noora", "lib", "noora", "icons"])
  @copy_icon @icons_path |> Path.join("copy.svg") |> File.read!() |> String.trim()
  @copy_check_icon @icons_path |> Path.join("copy-check.svg") |> File.read!() |> String.trim()

  @statuses %{
    "tip" => "success",
    "info" => "information",
    "note" => "information",
    "details" => "information",
    "warning" => "warning",
    "danger" => "error",
    "caution" => "error"
  }

  @doc "Translate `:::` containers in `markdown` to HTML blocks."
  @spec preprocess(String.t()) :: String.t()
  def preprocess(markdown) do
    markdown
    |> String.split("\n")
    |> walk(:normal, [])
    |> Enum.reverse()
    |> Enum.join("\n")
  end

  defp walk([], _state, out), do: out

  defp walk([line | rest], :normal, out) do
    trimmed = String.trim_trailing(line)

    cond do
      trimmed == "::: code-group" ->
        walk(rest, {:code_group, []}, out)

      captures = Regex.run(~r/^:::\s+(\w+)\s*(.*)$/, trimmed) ->
        [_, type, title] = captures

        if Map.has_key?(@statuses, type) do
          walk(rest, {:alert, Map.fetch!(@statuses, type), String.trim(title), []}, out)
        else
          walk(rest, :normal, [line | out])
        end

      true ->
        walk(rest, :normal, [line | out])
    end
  end

  defp walk([line | rest], {:alert, status, title, acc}, out) do
    if String.trim_trailing(line) == ":::" do
      walk(rest, :normal, [render_alert(status, title, Enum.reverse(acc)) | out])
    else
      walk(rest, {:alert, status, title, [line | acc]}, out)
    end
  end

  defp walk([line | rest], {:code_group, acc}, out) do
    if String.trim_trailing(line) == ":::" do
      walk(rest, :normal, [render_code_group(Enum.reverse(acc)) | out])
    else
      walk(rest, {:code_group, [line | acc]}, out)
    end
  end

  defp render_alert(status, title, lines) do
    "\n<div class=\"noora-alert-src\" data-status=\"#{status}\" data-title=\"#{escape(title)}\">\n\n" <>
      Enum.join(lines, "\n") <>
      "\n\n</div>\n"
  end

  defp render_code_group(lines) do
    blocks = parse_fences(lines, nil, [])

    tabs =
      blocks
      |> Enum.with_index()
      |> Enum.map_join("", fn {{_lang, label, _code}, index} ->
        selected = if index == 0, do: ~s( data-selected="true"), else: ""
        ~s(<button data-part="tab" data-index="#{index}"#{selected}>#{escape(label)}</button>)
      end)

    panels =
      blocks
      |> Enum.with_index()
      |> Enum.map_join("", fn {{lang, _label, code}, index} ->
        hidden = if index == 0, do: "", else: ~s( data-hidden="true")
        body = Enum.join(code, "\n")
        source = body |> escape() |> String.replace("\n", "&#10;")

        # Embed a real markdown fence (surrounded by blank lines) so MDEx renders
        # and syntax-highlights it; the surrounding div passes through as raw HTML.
        ~s(<div data-part="panel" data-index="#{index}"#{hidden}><template data-part="copy-source">#{source}</template>\n\n```#{lang}\n#{body}\n```\n\n</div>)
      end)

    copy_button =
      ~s(<button data-part="copy" aria-label="Copy code"><span data-part="copy-icon">#{@copy_icon}</span><span data-part="copy-check-icon">#{@copy_check_icon}</span></button>)

    ~s(\n<div class="code-group"><div data-part="header"><div data-part="tabs">) <>
      tabs <>
      ~s(</div>) <>
      copy_button <>
      ~s(</div><div data-part="panels">) <>
      panels <>
      ~s(</div></div>\n)
  end

  defp parse_fences([], nil, acc), do: Enum.reverse(acc)

  defp parse_fences([], {lang, label, code}, acc),
    do: Enum.reverse([{lang, label, Enum.reverse(code)} | acc])

  defp parse_fences([line | rest], nil, acc) do
    case Regex.run(~r/^```(\S+)?\s*(?:\[([^\]]*)\])?\s*$/, line) do
      [_, lang] -> parse_fences(rest, {lang, lang, []}, acc)
      [_, lang, label] -> parse_fences(rest, {lang, fallback(label, lang), []}, acc)
      _ -> parse_fences(rest, nil, acc)
    end
  end

  defp parse_fences([line | rest], {lang, label, code}, acc) do
    if String.trim_trailing(line) == "```" do
      parse_fences(rest, nil, [{lang, label, Enum.reverse(code)} | acc])
    else
      parse_fences(rest, {lang, label, [line | code]}, acc)
    end
  end

  defp fallback("", lang), do: lang
  defp fallback(label, _lang), do: label

  defp escape(text), do: text |> Phoenix.HTML.html_escape() |> Phoenix.HTML.safe_to_string()
end
