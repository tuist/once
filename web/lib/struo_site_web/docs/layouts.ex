defmodule StruoSiteWeb.Docs.Layouts do
  @moduledoc """
  Root layout for the documentation, loading the dedicated docs asset bundle
  (Noora styles and hooks) so the docs surface is styled independently of the
  marketing site.
  """
  use StruoSiteWeb, :html

  embed_templates "layouts/*"
end
