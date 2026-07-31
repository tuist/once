defmodule OnceSite.MCP.Server do
  @moduledoc false

  def server do
    EMCP.Server.new(
      name: "once-passport",
      version: "0.1.0",
      title: "Once Passport",
      description: "Query public Once Passport compatibility records and repository graphs.",
      tools: [OnceSite.MCP.Tools.ListPassports, OnceSite.MCP.Tools.GetPassport]
    )
  end
end
