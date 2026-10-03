defmodule OnceSite.Repo.Migrations.CreatePassportRepositories do
  use Ecto.Migration

  def change do
    create table(:passport_repositories, primary_key: false) do
      add(:id, :uuid, primary_key: true)
      add(:github_account, :string, null: false)
      add(:github_repository, :string, null: false)
      add(:default_branch, :string, null: false)
      add(:public, :boolean, null: false, default: true)

      timestamps(type: :utc_datetime)
    end

    create(unique_index(:passport_repositories, [:github_account, :github_repository]))

    create table(:passport_scans, primary_key: false) do
      add(:id, :uuid, primary_key: true)

      add(:repository_id, references(:passport_repositories, type: :uuid, on_delete: :delete_all),
        null: false
      )

      add(:commit_sha, :string, null: false)
      add(:status, :string, null: false)
      add(:compatibility_score, :integer)
      add(:estimated_weekly_hours, :decimal, precision: 6, scale: 1)
      add(:features, {:array, :string}, null: false, default: [])
      add(:details, :map, null: false, default: %{})
      add(:checked_at, :utc_datetime, null: false)

      timestamps(type: :utc_datetime)
    end

    create(index(:passport_scans, [:repository_id, :checked_at]))
    create(unique_index(:passport_scans, [:repository_id, :commit_sha]))
  end
end
