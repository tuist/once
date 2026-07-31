defmodule OnceSiteWeb.PageControllerTest do
  use OnceSiteWeb.ConnCase

  alias OnceSite.Passport.Repository
  alias OnceSite.Passport.Scan
  alias OnceSite.Repo

  test "GET /", %{conn: conn} do
    conn = get(conn, ~p"/")
    response = html_response(conn, 200)

    assert response =~ "Build once."
    assert response =~ "Natively supported"
    assert response =~ "Built with"
    assert response =~ "Join Discord"
    assert response =~ "https://discord.gg/fTpB5e3rRp"
    assert response =~ ~s(aria-label="GitHub")
    assert response =~ ~s(aria-label="Discord")
  end

  test "GET /github.com/:account/:repository renders a Passport profile", %{conn: conn} do
    repository =
      Repo.insert!(
        Repository.changeset(%Repository{}, %{
          github_account: "tuist",
          github_repository: "once",
          default_branch: "main",
          public: true
        })
      )

    Repo.insert!(
      Scan.changeset(%Scan{}, %{
        repository_id: repository.id,
        commit_sha: "f7aa39f123456",
        status: :compatible,
        compatibility_score: 92,
        estimated_weekly_hours: "8.4",
        features: ["cache", "remote_execution", "memory_scheduling"],
        details: %{"summary" => "A public Once compatibility profile."},
        checked_at: ~U[2026-07-31 00:00:00Z]
      })
    )

    conn = get(conn, "/github.com/tuist/once")
    response = html_response(conn, 200)

    assert response =~ "Once Passport"
    assert response =~ "Compatible"
    assert response =~ "/og/passport.jpg?"
    assert response =~ "8.4 developer hours each week"
  end
end
