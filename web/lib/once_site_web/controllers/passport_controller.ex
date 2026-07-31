defmodule OnceSiteWeb.PassportController do
  use OnceSiteWeb, :controller

  alias OnceSite.Passport
  alias OnceSite.Passport.GitHubApp
  alias OnceSiteWeb.Docs.OgImageRenderer
  alias OnceSiteWeb.PassportOgImage

  @cache OnceSite.Passport.Cache
  @cache_ttl :timer.hours(24)

  def index(conn, _params) do
    conn
    |> assign(:page_title, "Passport directory")
    |> assign(:meta_description, "Public repository compatibility records from Once Passport.")
    |> assign(:repositories, Passport.list_public_repositories())
    |> put_view(OnceSiteWeb.PageHTML)
    |> render(:passport_index)
  end

  def show(conn, %{"account" => account, "repository" => repository}) do
    case Passport.fetch_or_create_public_repository(account, repository) do
      {:ok, passport} ->
        render_passport(conn, passport)

      :error ->
        send_resp(conn, 404, "Not found")
    end
  end

  defp render_passport(conn, %{scans: []} = passport) do
    conn
    |> assign(:page_title, "#{passport.github_account}/#{passport.github_repository} is indexing")
    |> assign(:account, passport.github_account)
    |> assign(:repository, passport.github_repository)
    |> assign(:github_description, passport.github_description)
    |> put_view(OnceSiteWeb.PageHTML)
    |> render(:passport_indexing)
  end

  defp render_passport(conn, passport) do
    conn
    |> assign(:page_title, Passport.title(passport))
    |> assign(:meta_description, Passport.description(passport))
    |> assign(:canonical_url, OnceSiteWeb.Endpoint.url() <> Passport.public_url(passport))
    |> assign(:og_image, PassportOgImage.url(passport))
    |> assign(:og_image_alt, PassportOgImage.social_alt(passport))
    |> assign(Passport.page_attributes(passport))
    |> put_view(OnceSiteWeb.PageHTML)
    |> render(:passport)
  end

  def og_image(conn, params) do
    revision = params["revision"]

    with :ok <- PassportOgImage.verify(params),
         {:ok, passport} <-
           Passport.fetch_public_repository(params["account"], params["repository"]),
         %{scans: [%{commit_sha: ^revision} | _]} <- passport,
         {:ok, image} <- cached_image(passport) do
      conn
      |> put_resp_content_type("image/jpeg")
      |> put_resp_header("cache-control", "public, max-age=86400, stale-while-revalidate=604800")
      |> send_resp(200, image)
    else
      {:error, _reason} -> send_resp(conn, 503, "Passport image is temporarily unavailable")
      :error -> send_resp(conn, 404, "Not found")
      _ -> send_resp(conn, 404, "Not found")
    end
  end

  def integration(conn, %{"account" => account, "repository" => repository}) do
    case Passport.fetch_public_repository(account, repository) do
      {:ok, passport} ->
        conn
        |> assign(
          :page_title,
          "Integrate #{passport.github_account}/#{passport.github_repository}"
        )
        |> assign(:repository_url, Passport.public_url(passport))
        |> assign(:account, passport.github_account)
        |> assign(:repository, passport.github_repository)
        |> assign(:install_url, GitHubApp.install_url())
        |> put_view(OnceSiteWeb.PageHTML)
        |> render(:integration)

      :error ->
        send_resp(conn, 404, "Not found")
    end
  end

  def create_integration(conn, %{"account" => account, "repository" => repository}) do
    with {:ok, passport} <- Passport.fetch_public_repository(account, repository),
         {:ok, _request} <- Passport.request_integration(passport) do
      redirect_to_installation(conn, passport)
    else
      {:error, _changeset} ->
        conn
        |> put_flash(:error, "We could not add this repository to the integration queue.")
        |> redirect(to: "/github.com/#{account}/#{repository}/integrate")

      :error ->
        send_resp(conn, 404, "Not found")
    end
  end

  defp cached_image(passport) do
    key = PassportOgImage.cache_key(passport)

    case Cachex.fetch(@cache, key, fn _key -> cacheable_image(passport) end) do
      {:ok, image} -> {:ok, image}
      {:commit, image} -> {:ok, image}
      {:ignore, reason} -> {:error, reason}
      {:error, reason} -> {:error, reason}
    end
  end

  defp cacheable_image(passport) do
    case render_image(passport) do
      {:ok, image} -> {:commit, image, expire: @cache_ttl}
      {:error, reason} -> {:ignore, reason}
    end
  end

  defp render_image(passport) do
    with {:ok, renderer} <- OgImageRenderer.start(1) do
      try do
        OgImageRenderer.render(renderer, PassportOgImage.render_html(passport))
      after
        OgImageRenderer.stop(renderer)
      end
    end
  end

  defp redirect_to_installation(conn, passport) do
    case GitHubApp.install_url() do
      nil ->
        conn
        |> put_flash(
          :info,
          "Your request is saved. Configure the GitHub App install URL to grant access."
        )
        |> redirect(to: Passport.public_url(passport))

      url ->
        redirect(conn, external: url)
    end
  end
end
