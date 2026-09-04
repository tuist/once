defmodule OnceSiteWeb.ZeroToOnceOgImage do
  @moduledoc false

  alias OnceSiteWeb.Docs.OgImage

  @token_salt "zero-to-once-campaign-og-image"
  @content "zero-to-once-campaign"

  def url do
    "/og/zero-to-once-campaign.jpg?" <>
      URI.encode_query(%{
        "hash" => Phoenix.Token.sign(OnceSiteWeb.Endpoint, @token_salt, @content)
      })
  end

  def verify(%{"hash" => hash}) do
    case Phoenix.Token.verify(OnceSiteWeb.Endpoint, @token_salt, hash) do
      {:ok, @content} -> :ok
      _ -> :error
    end
  end

  def verify(_), do: :error

  def render_html do
    OgImage.render_html(
      title: "Zero-to-Once",
      description:
        "Bring your open source repository to Once. Share it, climb the queue, and build faster.",
      category: "Open source migration queue",
      subtitle: "Zero-to-Once",
      fonts_dir: "priv/static/fonts",
      logo_path: "priv/static/docs/nav-logo.png"
    )
  end
end
