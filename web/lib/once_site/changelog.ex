defmodule OnceSite.Changelog do
  @moduledoc """
  Loads changelog entries from `priv/changelog/*.md`.

  Each file has YAML-ish frontmatter (`title`, `date`) followed by a markdown
  body. Entries are returned newest first and are used by the changelog page and
  the RSS/Atom feeds.
  """

  defmodule Entry do
    @moduledoc false
    defstruct [:slug, :title, :date, :html, :summary]
  end

  @mdex_options [
    extension: [autolink: true, table: true, strikethrough: true, tasklist: true],
    render: [unsafe: true],
    syntax_highlight: nil
  ]

  @doc "All changelog entries, newest first."
  @spec entries() :: [Entry.t()]
  def entries do
    dir()
    |> Path.join("*.md")
    |> Path.wildcard()
    |> Enum.map(&load/1)
    |> Enum.sort_by(& &1.date, {:desc, Date})
  end

  @doc "A single entry by slug, or `nil`."
  @spec get_entry(String.t()) :: Entry.t() | nil
  def get_entry(slug), do: Enum.find(entries(), &(&1.slug == slug))

  @doc "The date of the most recent entry, or today when there are none."
  def last_updated do
    case entries() do
      [%Entry{date: date} | _] -> date
      [] -> Date.utc_today()
    end
  end

  defp dir, do: Application.app_dir(:once_site, "priv/changelog")

  defp load(file) do
    {frontmatter, body} = split(File.read!(file))

    %Entry{
      slug: file |> Path.basename(".md"),
      title: Map.get(frontmatter, "title", "Update"),
      date: parse_date(Map.get(frontmatter, "date")),
      html: MDEx.to_html!(body, @mdex_options),
      summary: summary(body)
    }
  end

  defp split("---\n" <> rest) do
    case String.split(rest, "\n---", parts: 2) do
      [frontmatter, body] -> {parse_frontmatter(frontmatter), String.trim_leading(body, "\n")}
      _ -> {%{}, "---\n" <> rest}
    end
  end

  defp split(content), do: {%{}, content}

  defp parse_frontmatter(text) do
    text
    |> String.split("\n", trim: true)
    |> Enum.map(&String.split(&1, ":", parts: 2))
    |> Enum.reduce(%{}, fn
      [key, value], acc -> Map.put(acc, String.trim(key), String.trim(value))
      _, acc -> acc
    end)
  end

  defp parse_date(nil), do: Date.utc_today()

  defp parse_date(value) do
    case Date.from_iso8601(String.trim(value)) do
      {:ok, date} -> date
      _ -> Date.utc_today()
    end
  end

  defp summary(body) do
    body
    |> String.split("\n\n", trim: true)
    |> List.first("")
    |> String.replace(~r/[`*_>#\[\]()]/, "")
    |> String.replace(~r/\s+/, " ")
    |> String.trim()
    |> String.slice(0, 200)
  end
end
