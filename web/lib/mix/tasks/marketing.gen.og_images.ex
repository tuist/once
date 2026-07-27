defmodule Mix.Tasks.Marketing.Gen.OgImages do
  @shortdoc "Generate marketing Open Graph images"

  @moduledoc """
  Generates Open Graph images for the marketing pages, changelog, and blog.

  Cards are rendered to `priv/static/images/og/<page>.jpg` with headless Chrome.
  Like the docs task, it degrades gracefully when Chrome is unavailable.
  """
  use Mix.Task

  alias OnceSite.Blog
  alias OnceSite.Blog.Authors
  alias OnceSite.Changelog
  alias OnceSiteWeb.Docs.OgImage
  alias OnceSiteWeb.Docs.OgImageRenderer

  @output "priv/static/images/og"
  @fonts_dir "priv/static/fonts"
  @logo "priv/static/docs/nav-logo.png"

  @static_pages [
    %{
      file: "home.jpg",
      title: "Build once. Reuse everywhere.",
      description:
        "Cacheable, remotely executable, reusable repository automation across every language."
    },
    %{
      file: "changelog.jpg",
      title: "Changelog",
      description: "The latest user-facing changes to Once."
    },
    %{
      file: "blog.jpg",
      title: "Ideas for building once",
      description: "Ideas and updates from the people building Once.",
      subtitle: "Blog"
    }
  ]

  @impl true
  def run(_args) do
    {:ok, _} = Application.ensure_all_started(:briefly)
    {:ok, _} = Application.ensure_all_started(:inets)
    {:ok, _} = Application.ensure_all_started(:ssl)

    case OgImageRenderer.start() do
      {:ok, renderer} ->
        try do
          File.mkdir_p!(@output)
          pages = pages()
          avatars = fetch_avatars(pages)

          for page <- pages do
            html =
              OgImage.render_html(
                title: page.title,
                description: page.description,
                category: Map.get(page, :category),
                subtitle: Map.get(page, :subtitle),
                avatars:
                  page
                  |> Map.get(:avatars, [])
                  |> Enum.map(&Map.get(avatars, &1))
                  |> Enum.reject(&is_nil/1),
                fonts_dir: @fonts_dir,
                logo_path: @logo
              )

            case OgImageRenderer.render(renderer, html) do
              {:ok, jpeg} -> File.write!(Path.join(@output, page.file), jpeg)
              {:error, reason} -> Mix.shell().error("  failed #{page.file}: #{inspect(reason)}")
            end
          end

          Mix.shell().info("Generated #{length(pages)} marketing OG images.")
        after
          OgImageRenderer.stop(renderer)
        end

      {:error, reason} ->
        Mix.shell().error(
          "[marketing.gen.og_images] Chrome unavailable (#{inspect(reason)}); skipping."
        )
    end
  end

  @doc false
  def pages do
    changelog_pages =
      Enum.map(Changelog.entries(), fn entry ->
        %{file: "changelog-#{entry.slug}.jpg", title: entry.title, description: entry.summary}
      end)

    blog_pages =
      Enum.map(Blog.all_posts(), fn post ->
        %{
          file: "blog-#{post.slug}.jpg",
          title: post.title,
          description: post.description,
          subtitle: "Blog",
          category: post.authors |> Enum.map_join(", ", & &1.name),
          avatars: Enum.map(post.authors, &Authors.avatar_url(&1, 256))
        }
      end)

    @static_pages ++ changelog_pages ++ blog_pages
  end

  defp fetch_avatars(pages) do
    pages
    |> Enum.flat_map(&Map.get(&1, :avatars, []))
    |> Enum.uniq()
    |> Task.async_stream(&fetch_avatar/1, ordered: false, timeout: 10_000)
    |> Enum.reduce(%{}, fn
      {:ok, {url, data_uri}}, avatars ->
        Map.put(avatars, url, data_uri)

      {:exit, reason}, avatars ->
        Mix.shell().error("  failed to fetch author avatar: #{inspect(reason)}")
        avatars
    end)
  end

  defp fetch_avatar(url) do
    http_options = [
      autoredirect: true,
      connect_timeout: 5_000,
      timeout: 10_000,
      ssl: [
        verify: :verify_peer,
        cacerts: :public_key.cacerts_get(),
        customize_hostname_check: [
          match_fun: :public_key.pkix_verify_hostname_match_fun(:https)
        ]
      ]
    ]

    result =
      :httpc.request(
        :get,
        {String.to_charlist(url), []},
        http_options,
        body_format: :binary
      )

    case result do
      {:ok, {{_version, 200, _reason}, headers, body}} ->
        content_type =
          Enum.find_value(headers, "image/jpeg", fn
            {~c"content-type", value} -> List.to_string(value)
            _ -> nil
          end)

        {url, "data:#{content_type};base64,#{Base.encode64(body)}"}

      {:ok, {{_version, status, reason}, _headers, _body}} ->
        Mix.shell().error("  failed avatar #{url}: HTTP #{status} #{reason}")
        {url, nil}

      {:error, reason} ->
        Mix.shell().error("  failed avatar #{url}: #{inspect(reason)}")
        {url, nil}
    end
  end
end
