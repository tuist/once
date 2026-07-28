defmodule OnceSite.Blog.Author do
  @moduledoc false

  @enforce_keys [:id, :name, :email]
  defstruct [:id, :name, :email]

  @type t :: %__MODULE__{
          id: String.t(),
          name: String.t(),
          email: String.t()
        }
end
