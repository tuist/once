defmodule OnceSiteWeb.Docs.OgImageTest do
  use ExUnit.Case, async: true

  alias OnceSiteWeb.Docs.OgImage

  test "uses a 2:1 social card ratio" do
    assert OgImage.width() == 1920
    assert OgImage.height() == 1080
  end

  test "slug_to_filename joins segments and appends .jpg" do
    assert OgImage.slug_to_filename(["guide", "why"]) == "guide-why.jpg"
    assert OgImage.slug_to_filename(["reference", "cli", "exec"]) == "reference-cli-exec.jpg"
    assert OgImage.slug_to_filename([]) == "index.jpg"
  end

  test "render_html embeds the title, category, and data URIs" do
    html =
      OgImage.render_html(
        title: "Getting Started",
        description: "Install Once.",
        category: "Guide",
        fonts_dir: Application.app_dir(:once_site, "priv/static/fonts"),
        logo_path: Application.app_dir(:once_site, "priv/static/docs/nav-logo.png")
      )

    assert html =~ "<!DOCTYPE html>"
    assert html =~ "Getting Started"
    assert html =~ "Install Once."
    assert html =~ "Guide"
    assert html =~ "data:font/woff2;base64,"
    assert html =~ "data:image/png;base64,"
    assert html =~ "width: 1920px"
    assert html =~ "height: 1080px"
  end

  test "render_html truncates overly long titles" do
    long = String.duplicate("a", 100)
    html = OgImage.render_html(title: long, fonts_dir: fonts(), logo_path: logo())

    assert html =~ "..."
  end

  test "render_html embeds multiple author avatars" do
    avatar = "data:image/png;base64,avatar"

    html =
      OgImage.render_html(
        title: "A post",
        category: "One, Two",
        avatars: [avatar, avatar],
        fonts_dir: fonts(),
        logo_path: logo()
      )

    assert html =~ ~s(data-part="author-meta")
    assert length(Regex.scan(~r/<img data-part="author-avatar"/, html)) == 2
    assert html =~ avatar
  end

  defp fonts, do: Application.app_dir(:once_site, "priv/static/fonts")
  defp logo, do: Application.app_dir(:once_site, "priv/static/docs/nav-logo.png")
end
