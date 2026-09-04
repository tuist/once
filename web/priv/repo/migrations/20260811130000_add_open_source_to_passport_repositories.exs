defmodule OnceSite.Repo.Migrations.AddOpenSourceToPassportRepositories do
  use Ecto.Migration

  def change do
    alter table(:passport_repositories) do
      add(:open_source, :boolean, null: false, default: false)
    end

    create(index(:passport_repositories, [:public, :open_source]))
  end
end
