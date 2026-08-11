defmodule OnceSite.MCP.Tools.GetPassport do
  @moduledoc false
  @behaviour EMCP.Tool

  alias OnceSite.Passport

  @impl true
  def name, do: "get_zero_to_once_project"

  @impl true
  def description,
    do:
      "Get a public repository's current compatibility report, graph, and invalidation candidates."

  @impl true
  def input_schema do
    %{
      "type" => "object",
      "properties" => %{
        "account" => %{"type" => "string"},
        "repository" => %{"type" => "string"}
      },
      "required" => ["account", "repository"]
    }
  end

  @impl true
  def annotations,
    do: %{title: "Get Zero-to-Once project", readOnlyHint: true, openWorldHint: false}

  @impl true
  def call(_conn, %{"account" => account, "repository" => repository}) do
    case Passport.fetch_public_repository(account, repository) do
      {:ok, %{scans: []}} -> EMCP.Tool.error("This Zero-to-Once project is still indexing.")
      {:ok, passport} -> passport |> Passport.page_attributes() |> response()
      :error -> EMCP.Tool.error("Project not found.")
    end
  end

  defp response(data),
    do:
      EMCP.Tool.response(%{"type" => "text", "text" => JSON.encode!(json_data(data))})
      |> Map.put("structuredContent", json_data(data))

  defp json_data(data) do
    Map.update!(data, :features, fn features ->
      Enum.map(features, fn {name, description} ->
        %{name: name, description: description}
      end)
    end)
  end
end
