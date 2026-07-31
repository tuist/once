defmodule OnceSite.Passport.GitHubClient do
  @moduledoc false

  @github_api "https://api.github.com"

  def repository(account, repository) do
    Req.get("#{@github_api}/repos/#{account}/#{repository}",
      headers: [{"accept", "application/vnd.github+json"}, {"user-agent", "once-passport"}]
    )
  end
end
