defmodule StruoSiteWeb.ConnCase do
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
      use StruoSiteWeb, :verified_routes

      import Phoenix.ConnTest
      import Plug.Conn
      import StruoSiteWeb.ConnCase

      # The default endpoint for testing
      @endpoint StruoSiteWeb.Endpoint

      # Import conveniences for testing with connections
    end
  end

  setup do
    {:ok, conn: Phoenix.ConnTest.build_conn()}
  end
end
