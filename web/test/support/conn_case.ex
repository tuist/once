defmodule OnceSiteWeb.ConnCase do
  @moduledoc """
  This module defines the test case to be used by
  tests that require setting up a connection.

  Such tests rely on `Phoenix.ConnTest` and also
  import other functionality to make it easier
  to build common data structures.
  """

  use ExUnit.CaseTemplate

  using do
    quote do
      use OnceSiteWeb, :verified_routes

      import OnceSiteWeb.ConnCase
      import Phoenix.ConnTest
      import Plug.Conn
      # The default endpoint for testing
      @endpoint OnceSiteWeb.Endpoint

      # Import conveniences for testing with connections
    end
  end

  setup do
    {:ok, conn: Phoenix.ConnTest.build_conn()}
  end
end
