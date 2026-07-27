defmodule OnceSiteWeb.ChangelogController do
  use OnceSiteWeb, :controller

  alias OnceSite.Changelog
  alias OnceSiteWeb.Endpoint

  def index(conn, _params) do
    conn
    |> assign(:page_title, "Changelog")
    |> assign(:meta_description, "The latest user-facing changes to Once.")
    |> assign(:og_image, "/images/og/changelog.jpg")
    |> assign(:entries, Changelog.entries())
    |> render(:index)
  end

  def show(conn, %{"slug" => slug}) do
    case Changelog.get_entry(slug) do
      nil ->
        conn
        |> put_status(:not_found)
        |> put_view(OnceSiteWeb.ErrorHTML)
        |> render(:"404")

      entry ->
        conn
        |> assign(:page_title, entry.title)
        |> assign(:meta_description, entry.summary)
        |> assign(:og_image, "/images/og/changelog-#{entry.slug}.jpg")
        |> assign(:entry, entry)
        |> render(:show)
    end
  end

  def rss(conn, _params) do
    conn
    |> put_resp_content_type("application/rss+xml")
    |> send_resp(200, rss_feed(Changelog.entries()))
  end

  def atom(conn, _params) do
    conn
    |> put_resp_content_type("application/atom+xml")
    |> send_resp(200, atom_feed(Changelog.entries()))
  end

  # --- feed builders ---

  defp rss_feed(entries) do
    items =
      Enum.map_join(entries, "\n", fn entry ->
        """
            <item>
              <title>#{escape(entry.title)}</title>
              <link>#{abs_url("/changelog##{entry.slug}")}</link>
              <guid isPermaLink="true">#{abs_url("/changelog##{entry.slug}")}</guid>
              <pubDate>#{rfc822(entry.date)}</pubDate>
              <description>#{escape(entry.summary)}</description>
              <content:encoded><![CDATA[#{entry.html}]]></content:encoded>
            </item>\
        """
      end)

    """
    <?xml version="1.0" encoding="UTF-8"?>
    <rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:atom="http://www.w3.org/2005/Atom">
      <channel>
        <title>Once changelog</title>
        <link>#{abs_url("/changelog")}</link>
        <atom:link href="#{abs_url("/changelog/feed.xml")}" rel="self" type="application/rss+xml" />
        <description>The latest user-facing changes to Once.</description>
        <language>en</language>
    #{items}
      </channel>
    </rss>
    """
  end

  defp atom_feed(entries) do
    updated =
      entries
      |> List.first()
      |> then(&if &1, do: iso8601(&1.date), else: iso8601(Date.utc_today()))

    entries_xml =
      Enum.map_join(entries, "\n", fn entry ->
        """
          <entry>
            <title>#{escape(entry.title)}</title>
            <link href="#{abs_url("/changelog##{entry.slug}")}" />
            <id>#{abs_url("/changelog##{entry.slug}")}</id>
            <updated>#{iso8601(entry.date)}</updated>
            <summary>#{escape(entry.summary)}</summary>
            <content type="html"><![CDATA[#{entry.html}]]></content>
          </entry>\
        """
      end)

    """
    <?xml version="1.0" encoding="UTF-8"?>
    <feed xmlns="http://www.w3.org/2005/Atom">
      <title>Once changelog</title>
      <link href="#{abs_url("/changelog")}" />
      <link href="#{abs_url("/changelog/feed.atom")}" rel="self" />
      <id>#{abs_url("/changelog")}</id>
      <updated>#{updated}</updated>
    #{entries_xml}
    </feed>
    """
  end

  defp abs_url(path), do: Endpoint.url() <> path

  defp escape(text) do
    text
    |> to_string()
    |> String.replace("&", "&amp;")
    |> String.replace("<", "&lt;")
    |> String.replace(">", "&gt;")
  end

  # RSS pubDate wants RFC 822; use midnight UTC for the entry's date.
  defp rfc822(date) do
    {:ok, datetime} = DateTime.new(date, ~T[00:00:00], "Etc/UTC")
    Calendar.strftime(datetime, "%a, %d %b %Y %H:%M:%S +0000")
  end

  defp iso8601(date), do: Date.to_iso8601(date) <> "T00:00:00Z"
end
