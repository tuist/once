defmodule OnceSite.MCP.Tools.ListPassports do
  @moduledoc false
  @behaviour EMCP.Tool

  alias OnceSite.Passport

  @impl true
  def name, do: "list_zero_to_once_projects"

  @impl true
  def description, do: "List public projects participating in Zero-to-Once."

  @impl true
  def input_schema do
    %{
      "type" => "object",
      "properties" => %{
        "page" => %{"type" => "integer", "minimum" => 1},
        "page_size" => %{"type" => "integer", "minimum" => 1, "maximum" => 24}
      }
    }
  end

  @impl true
  def annotations,
    do: %{title: "List Zero-to-Once projects", readOnlyHint: true, openWorldHint: false}

  @impl true
  def call(_conn, args) do
    {repositories, meta} = Passport.list_public_repositories(args)

    response(%{
      projects: Enum.map(repositories, &summary/1),
      page: meta.current_page,
      total_pages: meta.total_pages,
      total_count: meta.total_count
    })
  end

  defp summary(repository) do
    %{
      account: repository.github_account,
      repository: repository.github_repository,
      description: repository.github_description,
      status: if(repository.scans == [], do: "indexing", else: "indexed"),
      url: Passport.public_url(repository)
    }
  end

  defp response(data),
    do:
      EMCP.Tool.response(%{"type" => "text", "text" => JSON.encode!(data)})
      |> Map.put("structuredContent", data)
end
