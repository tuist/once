defmodule OnceSite.SchemaTest do
  use ExUnit.Case, async: true

  defmodule Parent do
    use OnceSite.Schema

    schema "parents" do
    end
  end

  defmodule Child do
    use OnceSite.Schema

    schema "children" do
      belongs_to(:parent, Parent)
    end
  end

  test "sets UUIDv7 as the primary key type" do
    assert Parent.__schema__(:primary_key) == [:id]
    assert Parent.__schema__(:type, :id) == UUIDv7
  end

  test "sets UUIDv7 as the foreign key type" do
    assert Child.__schema__(:type, :parent_id) == UUIDv7
  end
end
