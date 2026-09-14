defmodule OnceSite.Repo.Migrations.AddShareBoostToPassportIntegrationRequests do
  use Ecto.Migration

  def change do
    alter table(:passport_integration_requests) do
      add(:share_boost, :integer, null: false, default: 0)
    end

    create(index(:passport_integration_requests, [:share_boost, :requested_at]))
  end
end
