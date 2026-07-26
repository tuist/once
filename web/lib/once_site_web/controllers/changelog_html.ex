defmodule OnceSiteWeb.ChangelogHTML do
  @moduledoc """
  Renders the changelog page.
  """
  use OnceSiteWeb, :html

  embed_templates "changelog_html/*"

  @doc false
  def format_date(date), do: Calendar.strftime(date, "%B %-d, %Y")
end
