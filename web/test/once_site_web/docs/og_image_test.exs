defmodule OnceSiteWeb.Docs.OgImageTest do
  use ExUnit.Case, async: true

  alias OnceSiteWeb.Docs.OgImage

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
  end

  test "render_html truncates overly long titles" do
    long = String.duplicate("a", 100)
    html = OgImage.render_html(title: long, fonts_dir: fonts(), logo_path: logo())

    assert html =~ "..."
  end

  defp fonts, do: Application.app_dir(:once_site, "priv/static/fonts")
  defp logo, do: Application.app_dir(:once_site, "priv/static/docs/nav-logo.png")
end
