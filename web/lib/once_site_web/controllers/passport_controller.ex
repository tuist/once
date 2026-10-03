defmodule OnceSiteWeb.PassportController do
  use OnceSiteWeb, :controller

  alias OnceSite.Passport
  alias OnceSite.Passport.GitHubApp
  alias OnceSite.Passport.GitHubOAuth
  alias OnceSiteWeb.Docs.OgImageRenderer
  alias OnceSiteWeb.PassportOgImage
  alias OnceSiteWeb.ZeroToOnceOgImage

  @cache OnceSite.Passport.Cache
  @cache_ttl :timer.hours(24)

  def github_login(conn, _params) do
    if GitHubOAuth.configured?() do
      state = Base.url_encode64(:crypto.strong_rand_bytes(32), padding: false)

      conn
      |> put_session(:zero_to_once_oauth_state, state)
      |> redirect(external: GitHubOAuth.authorize_url(state))
    else
      conn
      |> put_flash(:error, "GitHub login is not configured yet.")
      |> redirect(to: "/zero-to-once/")
    end
  end

  def github_callback(conn, %{"code" => code, "state" => state}) do
    with ^state <- get_session(conn, :zero_to_once_oauth_state),
         {:ok, token} <- GitHubOAuth.exchange_code(code),
         {:ok, repositories} <- GitHubOAuth.repositories(token) do
      cache_key = Base.url_encode64(:crypto.strong_rand_bytes(24), padding: false)

      {:ok, _} =
        Cachex.put(@cache, {:zero_to_once_repositories, cache_key}, repositories,
          ttl: :timer.minutes(10)
        )

      conn
      |> delete_session(:zero_to_once_oauth_state)
      |> put_session(:zero_to_once_repository_key, cache_key)
      |> redirect(to: "/zero-to-once/")
    else
      _ ->
        conn
        |> delete_session(:zero_to_once_oauth_state)
        |> put_flash(:error, "We could not load your repositories from GitHub.")
        |> redirect(to: "/zero-to-once/")
    end
  end

  def github_callback(conn, _params) do
    conn
    |> put_flash(:error, "GitHub did not return an authorization code.")
    |> redirect(to: "/zero-to-once/")
  end

  def index(conn, _params) do
    conn
    |> assign(:page_title, "Zero-to-Once")
    |> assign(:meta_description, "A public queue for bringing open source repositories to Once.")
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
    |> assign(:share_url, share_url(passport))
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
      {:error, _reason} -> send_resp(conn, 503, "Zero-to-Once image is temporarily unavailable")
      :error -> send_resp(conn, 404, "Not found")
      _ -> send_resp(conn, 404, "Not found")
    end
  end

  def campaign_og_image(conn, params) do
    with :ok <- ZeroToOnceOgImage.verify(params),
         {:ok, image} <- cached_campaign_image() do
      conn
      |> put_resp_content_type("image/jpeg")
      |> put_resp_header("cache-control", "public, max-age=86400, stale-while-revalidate=604800")
      |> send_resp(200, image)
    else
      _ -> send_resp(conn, 404, "Not found")
    end
  end

  def integration(conn, %{"account" => account, "repository" => repository}) do
    case Passport.fetch_repository(account, repository) do
      {:ok, passport} ->
        conn
        |> assign(
          :page_title,
          "Integrate #{passport.github_account}/#{passport.github_repository}"
        )
        |> assign(:repository_url, Passport.public_url(passport))
        |> assign(:account, passport.github_account)
        |> assign(:repository, passport.github_repository)
        |> assign(:integration, Passport.integration_attributes(passport))
        |> assign(:install_url, GitHubApp.install_url())
        |> put_view(OnceSiteWeb.PageHTML)
        |> render(:integration)

      :error ->
        send_resp(conn, 404, "Not found")
    end
  end

  def create_integration(conn, %{"account" => account, "repository" => repository}) do
    with {:ok, passport} <- Passport.fetch_repository(account, repository),
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

  def share(conn, %{"account" => account, "repository" => repository}) do
    with {:ok, project} <- Passport.fetch_public_repository(account, repository),
         {:ok, _request} <- Passport.share_project_page(project) do
      conn
      |> put_flash(:info, "Your repository received a community boost in the Zero-to-Once queue.")
      |> redirect(to: Passport.public_url(project))
    else
      :error -> send_resp(conn, 404, "Not found")
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

  defp cached_campaign_image do
    case Cachex.fetch(@cache, :zero_to_once_campaign_og_image, fn _key ->
           case render_campaign_image() do
             {:ok, image} -> {:commit, image, expire: @cache_ttl}
             {:error, reason} -> {:ignore, reason}
           end
         end) do
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

  defp render_campaign_image do
    with {:ok, renderer} <- OgImageRenderer.start(1) do
      try do
        OgImageRenderer.render(renderer, ZeroToOnceOgImage.render_html())
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
          "Your Zero-to-Once request is saved. Configure the GitHub App install URL to grant access."
        )
        |> redirect(to: Passport.public_url(passport))

      url ->
        redirect(conn, external: url)
    end
  end

  defp share_url(passport) do
    project_url = OnceSiteWeb.Endpoint.url() <> Passport.public_url(passport)
    message = "#{passport.github_account}/#{passport.github_repository} is joining Zero-to-Once"

    "https://x.com/intent/post?" <> URI.encode_query(%{"text" => message, "url" => project_url})
  end
end
