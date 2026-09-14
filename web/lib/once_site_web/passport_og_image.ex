defmodule OnceSiteWeb.PassportOgImage do
  @moduledoc false

  alias OnceSite.Passport
  alias OnceSiteWeb.Docs.OgImage

  @token_salt "zero-to-once-og-image"

  def render_html(repository) do
    attributes = Passport.page_attributes(repository)

    OgImage.render_html(
      title: "#{attributes.account}/#{attributes.repository}",
      description:
        "Zero-to-Once · #{attributes.score}/100 compatible · #{attributes.estimated_savings}",
      category: "Sandbox checked",
      subtitle: "Zero-to-Once",
      fonts_dir: "priv/static/fonts",
      logo_path: "priv/static/docs/nav-logo.png"
    )
  end

  def cache_key(repository), do: {repository.id, repository.scans |> hd() |> Map.fetch!(:id)}

  def url(repository) do
    scan = hd(repository.scans)
    content = content(repository, scan.commit_sha)

    "/og/zero-to-once.jpg?" <>
      URI.encode_query(%{
        "account" => repository.github_account,
        "repository" => repository.github_repository,
        "revision" => scan.commit_sha,
        "hash" => Phoenix.Token.sign(OnceSiteWeb.Endpoint, @token_salt, content)
      })
  end

  def verify(%{
        "account" => account,
        "repository" => repository,
        "revision" => revision,
        "hash" => hash
      }) do
    with {:ok, content} <- Phoenix.Token.verify(OnceSiteWeb.Endpoint, @token_salt, hash),
         true <- content == content(account, repository, revision) do
      :ok
    else
      _ -> :error
    end
  end

  def verify(_), do: :error

  def social_alt(repository), do: Passport.description(repository)

  defp content(repository, revision),
    do: content(repository.github_account, repository.github_repository, revision)

  defp content(account, repository, revision), do: "#{account}:#{repository}:#{revision}"
end
