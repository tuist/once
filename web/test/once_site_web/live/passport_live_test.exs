defmodule OnceSiteWeb.PassportLiveTest do
  use OnceSiteWeb.ConnCase

  import Phoenix.LiveViewTest

  alias OnceSite.Passport.Repository
  alias OnceSite.Repo

  test "lists Passport repositories with URL-driven filters and pagination", %{conn: conn} do
    for repository <- ["once", "tuist", "xcodeproj", "xcodegen"] do
      Repo.insert!(
        Repository.changeset(%Repository{}, %{
          github_account: "tuist",
          github_repository: repository,
          github_description: "#{repository} description",
          default_branch: "main",
          public: true
        })
      )
    end

    {:ok, _view, html} = live(conn, "/passports/")

    assert html =~ "Passport directory"
    assert html =~ "Filter repositories"
    assert html =~ "tuist/once"
    assert html =~ ~s(data-part="page-button")

    {:ok, _view, filtered_html} =
      live(conn, "/passports/?filter_github_repository_op=%3D~&filter_github_repository_val=once")

    assert filtered_html =~ "tuist/once"
    refute filtered_html =~ "tuist/tuist"
  end
end
