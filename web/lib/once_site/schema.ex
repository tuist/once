defmodule OnceSite.Schema do
  @moduledoc false

  defmacro __using__(_opts) do
    quote do
      use Ecto.Schema

      @primary_key {:id, UUIDv7, autogenerate: true}
      @foreign_key_type UUIDv7
    end
  end
end
