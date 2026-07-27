defmodule OnceSite.Blog do
  @moduledoc """
  Compiles the blog's Markdown posts into the application.
  """

  use NimblePublisher,
    build: OnceSite.Blog.Post,
    from: Application.app_dir(:once_site, "priv/posts/**/*.md"),
    as: :posts,
    html_converter: OnceSite.Blog.MarkdownConverter

  alias OnceSite.Blog.Post

  @page_size 2
  @pagination_options [
    default_limit: @page_size,
    filtering: false,
    max_limit: @page_size,
    ordering: false,
    pagination_types: [:page],
    replace_invalid_params: true
  ]

  @posts Enum.sort_by(@posts, & &1.date, {:desc, Date})

  @spec all_posts() :: [Post.t()]
  def all_posts, do: @posts

  @spec get_post(String.t()) :: Post.t() | nil
  def get_post(slug), do: Enum.find(all_posts(), &(&1.slug == slug))

  @spec paginate(map()) :: {[Post.t()], Flop.Meta.t()}
  def paginate(params \\ %{}) do
    posts = all_posts()

    pagination_params =
      params
      |> Map.take(["page"])
      |> Map.put("page_size", @page_size)

    {:ok, flop} = Flop.validate(pagination_params, @pagination_options)
    flop = clamp_page(flop, length(posts))
    meta = Flop.meta(posts, flop, count: length(posts))
    page = Enum.slice(posts, meta.current_offset, meta.page_size)

    {page, meta}
  end

  @spec last_updated() :: Date.t()
  def last_updated do
    all_posts()
    |> Enum.map(& &1.date)
    |> List.first(Date.utc_today())
  end

  defp clamp_page(flop, count) do
    total_pages = max(ceil(count / @page_size), 1)
    Flop.set_page(flop, min(flop.page || 1, total_pages))
  end
end
