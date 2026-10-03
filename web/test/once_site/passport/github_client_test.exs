defmodule OnceSite.Passport.GitHubClientTest do
  use ExUnit.Case, async: false

  import Mimic

  alias OnceSite.Passport.GitHubClient

  setup :verify_on_exit!

  test "fetches a repository through Req" do
    expect(Req, :get, fn "https://api.github.com/repos/tuist/once", options ->
      assert {"accept", "application/vnd.github+json"} in options[:headers]
      assert {"user-agent", "once-passport"} in options[:headers]
      {:ok, %Req.Response{status: 200, body: %{"default_branch" => "main"}}}
    end)

    assert {:ok, %Req.Response{status: 200}} = GitHubClient.repository("tuist", "once")
  end
end
