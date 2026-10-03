defmodule OnceSite.Passport.Scan do
  @moduledoc false

  use OnceSite.Schema

  import Ecto.Changeset

  schema "passport_scans" do
    field(:commit_sha, :string)
    field(:status, Ecto.Enum, values: [:compatible, :incompatible, :failed])
    field(:compatibility_score, :integer)
    field(:estimated_weekly_hours, :decimal)
    field(:features, {:array, :string}, default: [])
    field(:details, :map, default: %{})
    field(:checked_at, :utc_datetime)

    belongs_to(:repository, OnceSite.Passport.Repository)

    timestamps(type: :utc_datetime)
  end

  def changeset(scan, attrs) do
    scan
    |> cast(attrs, [
      :repository_id,
      :commit_sha,
      :status,
      :compatibility_score,
      :estimated_weekly_hours,
      :features,
      :details,
      :checked_at
    ])
    |> validate_required([:repository_id, :commit_sha, :status, :checked_at])
    |> validate_number(:compatibility_score,
      greater_than_or_equal_to: 0,
      less_than_or_equal_to: 100
    )
  end
end
