defmodule OnceSite.Repo.Migrations.CreatePassportGraphs do
  use Ecto.Migration

  def change do
    create table(:passport_graphs, primary_key: false) do
      add(:id, :uuid, primary_key: true)

      add(:repository_id, references(:passport_repositories, type: :uuid, on_delete: :delete_all),
        null: false
      )

      add(:commit_sha, :string, null: false)
      add(:branch, :string, null: false)
      add(:graph, :map, null: false, default: %{})
      add(:analysis, :map, null: false, default: %{})
      add(:checked_at, :utc_datetime, null: false)
      timestamps(type: :utc_datetime)
    end

    create(unique_index(:passport_graphs, [:repository_id, :branch, :commit_sha]))
    create(index(:passport_graphs, [:repository_id, :checked_at]))
  end
end
