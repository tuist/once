defmodule StruoSite.Registry do
  @moduledoc """
  Static seed data for the public Struo registry.

  The marketing site can render this now while the package format and backing
  storage settle.
  """

  @entries [
    %{
      kind: "script",
      name: "release/package",
      summary: "Builds release archives through an annotated mise task.",
      command: "struo exec mise/tasks/release/package.sh",
      tags: ["release", "cacheable"]
    },
    %{
      kind: "script",
      name: "release/publish",
      summary: "Publishes signed release artifacts after packaging completes.",
      command: "struo exec mise/tasks/release/publish.sh",
      tags: ["release", "remote-ready"]
    },
    %{
      kind: "script",
      name: "rust/workspace-check",
      summary: "Runs the workspace Rust checks as a reusable cached command.",
      command: "struo exec -- cargo check --workspace",
      tags: ["rust", "workspace"]
    }
  ]

  def list_entries, do: @entries
end
