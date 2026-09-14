defmodule OnceSite.Repo.Migrations.AddGithubDescriptionToPassportRepositories do
  use Ecto.Migration

  def change do
    alter table(:passport_repositories) do
      add(:github_description, :text)
    end
  end
end
