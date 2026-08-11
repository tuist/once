alias OnceSite.Passport.Repository
alias OnceSite.Passport.Scan
alias OnceSite.Passport.Graph
alias OnceSite.Passport.IntegrationRequest
alias OnceSite.Repo

import Ecto.Query

profiles = [
  %{
    repository: "once",
    github_description: "Build once. Reuse everywhere.",
    score: 92,
    weekly_hours: "8.4",
    commit_sha: "0000000",
    summary:
      "Once can make the repository's builds, tests, and generated artifacts reusable across local development and continuous integration."
  },
  %{
    repository: "tuist",
    github_description: "The open source platform for developers to build and test apps.",
    score: 88,
    weekly_hours: "11.2",
    commit_sha: "1111111",
    summary:
      "Once can reuse the repository's command-line builds and tests while keeping execution isolated in a declared environment."
  },
  %{
    repository: "xcodeproj",
    github_description: "Read, modify, and write Xcode project files.",
    indexing?: true,
    score: 84,
    weekly_hours: "5.6",
    commit_sha: "2222222",
    summary:
      "Once can cache the repository's dependency resolution, compilation, and test actions when their inputs are unchanged."
  },
  %{
    repository: "xcodegen",
    github_description: "A tool for generating your Xcode project.",
    score: 86,
    weekly_hours: "6.8",
    commit_sha: "3333333",
    summary: "Once can cache project generation and reuse the generated project when inputs are unchanged."
  },
  %{
    repository: "xcbeautify",
    github_description: "A little beautifier tool for Xcode build output.",
    score: 79,
    weekly_hours: "3.2",
    commit_sha: "4444444",
    summary: "Once can reuse test and release actions while preserving their declared toolchain inputs."
  },
  %{
    repository: "tuistenv",
    github_description: "A tool for installing and managing Tuist versions.",
    score: 81,
    weekly_hours: "4.1",
    commit_sha: "5555555",
    summary: "Once can cache installation and verification actions with explicit environment inputs."
  },
  %{
    repository: "registry",
    github_description: "The Tuist registry for package and project metadata.",
    score: 76,
    weekly_hours: "2.5",
    commit_sha: "6666666",
    summary: "Once can reuse validation and generated metadata actions across contributors."
  }
]

graph = %{
  "summary" => %{
    "actions" => 4,
    "cached_actions" => 3,
    "critical_path_ms" => 42_800,
    "artifact_bytes" => 18_400_000,
    "remote_executable" => 4
  },
  "nodes" => [
    %{"id" => "resolve", "title" => "Resolve packages", "command" => "mise run deps:resolve", "duration_ms" => 4_200, "memory_mib" => 512, "cache" => "hit", "executor" => "local"},
    %{"id" => "build", "title" => "Build workspace", "command" => "cargo build --workspace", "duration_ms" => 18_600, "memory_mib" => 2048, "cache" => "hit", "executor" => "remote"},
    %{"id" => "test", "title" => "Test workspace", "command" => "cargo test --workspace", "duration_ms" => 20_000, "memory_mib" => 2048, "cache" => "miss", "executor" => "remote"},
    %{"id" => "package", "title" => "Package release", "command" => "cargo build --release", "duration_ms" => 6_800, "memory_mib" => 1024, "cache" => "hit", "executor" => "remote"}
  ],
  "edges" => [
    %{"source" => "resolve", "target" => "build"},
    %{"source" => "build", "target" => "test"},
    %{"source" => "build", "target" => "package"}
  ]
}

graph_analysis = %{
  "observed_commits" => 24,
  "candidates" => [
    %{
      "target" => "Build workspace",
      "reason" => "Changed in 17 of the last 24 default-branch commits",
      "dependent_targets" => 3,
      "invalidated_actions" => 46,
      "recommendation" => "Extract stable workspace units so changes do not invalidate every downstream target."
    },
    %{
      "target" => "Test workspace",
      "reason" => "Changed in 11 of the last 24 default-branch commits",
      "dependent_targets" => 2,
      "invalidated_actions" => 22,
      "recommendation" => "Declare narrower test inputs so unrelated source changes retain cached test results."
    }
  ]
}

Enum.each(profiles, fn profile ->
  repository =
    Repo.insert!(
      Repository.changeset(%Repository{}, %{
        github_account: "tuist",
        github_repository: profile.repository,
        github_description: profile.github_description,
        default_branch: "main",
        public: true,
        open_source: true
      }),
      on_conflict:
        {:replace, [:github_description, :default_branch, :public, :open_source, :updated_at]},
      conflict_target: [:github_account, :github_repository],
      returning: true
    )

  repository =
    if repository.id do
      repository
    else
      Repo.get_by!(Repository, github_account: "tuist", github_repository: profile.repository)
    end

  if profile[:indexing?] do
    Repo.delete_all(from(scan in Scan, where: scan.repository_id == ^repository.id))
  else
    Repo.insert!(
      Scan.changeset(%Scan{}, %{
        repository_id: repository.id,
        commit_sha: profile.commit_sha,
        status: :compatible,
        compatibility_score: profile.score,
        estimated_weekly_hours: profile.weekly_hours,
        features: ["cache", "remote_execution", "memory_scheduling"],
        details: %{"summary" => profile.summary},
        checked_at: ~U[2026-07-31 00:00:00Z]
      }),
      on_conflict: :nothing,
      conflict_target: [:repository_id, :commit_sha]
    )

    Repo.insert!(
      Graph.changeset(%Graph{}, %{
        repository_id: repository.id,
        commit_sha: profile.commit_sha,
        branch: "main",
        graph: graph,
        analysis: graph_analysis,
        checked_at: ~U[2026-07-31 00:00:00Z]
      }),
      on_conflict: :nothing,
      conflict_target: [:repository_id, :branch, :commit_sha]
    )

    status =
      case profile.repository do
        "tuist" -> :integrating
        "registry" -> :awaiting_access
        _ -> :queued
      end

    Repo.insert!(
      IntegrationRequest.changeset(%IntegrationRequest{}, %{
        repository_id: repository.id,
        status: status,
        requested_at: ~U[2026-07-30 00:00:00Z],
        queued_at: if(status == :awaiting_access, do: nil, else: ~U[2026-07-30 00:00:00Z]),
        started_at: if(status == :integrating, do: ~U[2026-07-31 00:00:00Z], else: nil),
        share_boost: if(profile.repository == "once", do: 2, else: 0)
      }),
      on_conflict: :nothing,
      conflict_target: [:repository_id]
    )
  end
end)
