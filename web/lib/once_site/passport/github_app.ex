defmodule OnceSite.Passport.GitHubApp do
  @moduledoc false

  def install_url do
    Application.get_env(:once_site, :github_app_install_url)
  end
end
