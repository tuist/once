defmodule OnceSite.Passport.GitHubOAuth do
  @moduledoc false

  @github_authorize_url "https://github.com/login/oauth/authorize"
  @github_token_url "https://github.com/login/oauth/access_token"
  @github_api "https://api.github.com"

  def configured? do
    is_binary(client_id()) and is_binary(client_secret()) and is_binary(redirect_uri())
  end

  def authorize_url(state) do
    @github_authorize_url <>
      "?" <>
      URI.encode_query(%{
        "client_id" => client_id(),
        "redirect_uri" => redirect_uri(),
        "scope" => "read:user repo",
        "state" => state
      })
  end

  def exchange_code(code) do
    case Req.post(@github_token_url,
           form: [client_id: client_id(), client_secret: client_secret(), code: code],
           headers: [{"accept", "application/json"}]
         ) do
      {:ok, %{status: 200, body: %{"access_token" => token}}} -> {:ok, token}
      _ -> {:error, :token_exchange_failed}
    end
  end

  def repositories(token) do
    fetch_repositories(token, 1, [])
  end

  defp fetch_repositories(token, page, repositories) do
    case Req.get("#{@github_api}/user/repos",
           params: [
             affiliation: "owner,collaborator,organization_member",
             per_page: 100,
             page: page
           ],
           headers: headers(token)
         ) do
      {:ok, %{status: 200, body: page_repositories}} when is_list(page_repositories) ->
        repositories = repositories ++ page_repositories

        if length(page_repositories) == 100 do
          fetch_repositories(token, page + 1, repositories)
        else
          repositories
          |> Enum.reject(& &1["archived"])
          |> Enum.map(&repository_attributes/1)
          |> then(&{:ok, &1})
        end

      _ ->
        {:error, :repository_fetch_failed}
    end
  end

  defp repository_attributes(repository) do
    %{
      "full_name" => repository["full_name"],
      "description" => repository["description"],
      "default_branch" => repository["default_branch"],
      "private" => repository["private"],
      "open_source" => is_map(repository["license"])
    }
  end

  defp headers(token),
    do: [
      {"accept", "application/vnd.github+json"},
      {"authorization", "Bearer #{token}"},
      {"user-agent", "zero-to-once"}
    ]

  defp client_id, do: Application.get_env(:once_site, :github_oauth_client_id)
  defp client_secret, do: Application.get_env(:once_site, :github_oauth_client_secret)
  defp redirect_uri, do: Application.get_env(:once_site, :github_oauth_redirect_uri)
end
