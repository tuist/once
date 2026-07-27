defmodule OnceSite.Blog.Authors do
  @moduledoc false

  alias OnceSite.Blog.Author

  @authors %{
    "pedro" => %Author{
      id: "pedro",
      name: "Pedro Piñera",
      email: "pedro@pepicrft.me"
    }
  }

  @spec all() :: [Author.t()]
  def all, do: Map.values(@authors)

  @spec fetch!(String.t()) :: Author.t()
  def fetch!(id) do
    case Map.fetch(@authors, id) do
      {:ok, author} -> author
      :error -> raise ArgumentError, "unknown blog author #{inspect(id)}"
    end
  end

  @spec avatar_url(Author.t(), pos_integer()) :: String.t()
  def avatar_url(%Author{} = author, size \\ 96) when is_integer(size) and size > 0 do
    hash =
      author.email
      |> String.trim()
      |> String.downcase()
      |> then(&:crypto.hash(:sha256, &1))
      |> Base.encode16(case: :lower)

    query = URI.encode_query(%{d: "retro", r: "g", s: min(size, 2048)})
    "https://gravatar.com/avatar/#{hash}?#{query}"
  end
end
