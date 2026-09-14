defmodule OnceSite.MCP.Server do
  @moduledoc false

  def server do
    EMCP.Server.new(
      name: "zero-to-once",
      version: "0.1.0",
      title: "Zero-to-Once",
      description: "Query public Zero-to-Once repository reports and repository graphs.",
      tools: [OnceSite.MCP.Tools.ListPassports, OnceSite.MCP.Tools.GetPassport]
    )
  end
end
