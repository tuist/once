defmodule OnceSite.MCP.Tools.GetPassport do
  @moduledoc false
  @behaviour EMCP.Tool

  alias OnceSite.Passport

  @impl true
  def name, do: "get_passport"

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
  def annotations, do: %{title: "Get Passport", readOnlyHint: true, openWorldHint: false}

  @impl true
  def call(_conn, %{"account" => account, "repository" => repository}) do
    case Passport.fetch_public_repository(account, repository) do
      {:ok, %{scans: []}} -> EMCP.Tool.error("This Passport is still indexing.")
      {:ok, passport} -> passport |> Passport.page_attributes() |> response()
      :error -> EMCP.Tool.error("Passport not found.")
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
