defmodule OnceSite.Blog.Post do
  @moduledoc false

  alias OnceSite.Blog.Authors

  @enforce_keys [:slug, :title, :description, :authors, :date, :body, :reading_minutes]
  defstruct [:slug, :title, :description, :authors, :date, :body, :reading_minutes]

  @authors_by_id Authors.all() |> Map.new(&{&1.id, &1})

  @type t :: %__MODULE__{
          slug: String.t(),
          title: String.t(),
          description: String.t(),
          authors: [OnceSite.Blog.Author.t()],
          date: Date.t(),
          body: String.t(),
          reading_minutes: pos_integer()
        }

  @doc false
  def build(filename, attributes, body) do
    [year, month_day_slug] =
      filename
      |> Path.rootname()
      |> Path.split()
      |> Enum.take(-2)

    [month, day, slug] = String.split(month_day_slug, "-", parts: 3)
    date = Date.from_iso8601!("#{year}-#{month}-#{day}")

    authors = build_authors(Map.fetch!(attributes, :authors))

    struct!(__MODULE__, %{
      slug: slug,
      title: Map.fetch!(attributes, :title),
      description: Map.fetch!(attributes, :description),
      authors: authors,
      date: date,
      body: body,
      reading_minutes: reading_minutes(body)
    })
  end

  defp reading_minutes(body) do
    body
    |> String.replace(~r/<[^>]+>/, " ")
    |> String.split(~r/\s+/, trim: true)
    |> length()
    |> Kernel./(220)
    |> ceil()
    |> max(1)
  end

  defp build_authors([_ | _] = author_ids) do
    Enum.map(author_ids, &Map.fetch!(@authors_by_id, &1))
  end

  defp build_authors([]), do: raise(ArgumentError, "a blog post must have at least one author")
end
