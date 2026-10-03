defmodule OnceSite.Repo.Migrations.CreatePassportIntegrationRequests do
  use Ecto.Migration

  def change do
    create table(:passport_integration_requests, primary_key: false) do
      add(:id, :uuid, primary_key: true)

      add(
        :repository_id,
        references(:passport_repositories, type: :uuid, on_delete: :delete_all),
        null: false
      )

      add(:status, :string, null: false)
      add(:github_installation_id, :bigint)
      add(:requested_at, :utc_datetime, null: false)
      add(:access_granted_at, :utc_datetime)
      add(:queued_at, :utc_datetime)
      add(:started_at, :utc_datetime)

      timestamps(type: :utc_datetime)
    end

    create(unique_index(:passport_integration_requests, [:repository_id]))
    create(unique_index(:passport_integration_requests, [:github_installation_id]))
    create(index(:passport_integration_requests, [:status, :queued_at]))
  end
end
