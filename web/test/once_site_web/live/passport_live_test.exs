defmodule OnceSiteWeb.PassportLiveTest do
  use OnceSiteWeb.ConnCase

  import Phoenix.LiveViewTest

  alias OnceSite.Passport.Repository
  alias OnceSite.Repo

  test "lists Zero-to-Once projects with URL-driven filters and pagination", %{conn: conn} do
    for repository <- ["once", "tuist", "xcodeproj", "xcodegen"] do
      Repo.insert!(
        Repository.changeset(%Repository{}, %{
          github_account: "tuist",
          github_repository: repository,
          github_description: "#{repository} description",
          default_branch: "main",
          public: true,
          open_source: true
        })
      )
    end

    {:ok, _view, html} = live(conn, "/zero-to-once/")

    assert html =~ "Zero-to-Once"
    assert html =~ "Filter repositories"
    assert html =~ "tuist/once"
    assert html =~ ~s(data-part="page-button")

    {:ok, _view, filtered_html} =
      live(
        conn,
        "/zero-to-once/?filter_github_repository_op=%3D~&filter_github_repository_val=once"
      )

    assert filtered_html =~ "tuist/once"
    refute filtered_html =~ "tuist/tuist"
  end
end
