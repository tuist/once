defmodule OnceSite.Blog.Feed do
  @moduledoc false

  alias OnceSite.Blog
  alias OnceSiteWeb.Endpoint

  @spec rss([Blog.Post.t()]) :: String.t()
  def rss(posts) do
    items =
      Enum.map_join(posts, "\n", fn post ->
        creators =
          Enum.map_join(post.authors, "\n", fn author ->
            "      <dc:creator>#{escape(author.name)}</dc:creator>"
          end)

        """
            <item>
              <title>#{escape(post.title)}</title>
              <link>#{post_url(post)}</link>
              <guid isPermaLink="true">#{post_url(post)}</guid>
              <pubDate>#{rfc822(post.date)}</pubDate>
              <description>#{escape(post.description)}</description>
        #{creators}
              <content:encoded><![CDATA[#{cdata(post.body)}]]></content:encoded>
            </item>\
        """
      end)

    """
    <?xml version="1.0" encoding="UTF-8"?>
    <rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:atom="http://www.w3.org/2005/Atom" xmlns:dc="http://purl.org/dc/elements/1.1/">
      <channel>
        <title>Once blog</title>
        <link>#{absolute_url("/blog")}</link>
        <atom:link href="#{absolute_url("/blog/feed.xml")}" rel="self" type="application/rss+xml" />
        <description>Ideas and updates from the people building Once.</description>
        <language>en</language>
        <lastBuildDate>#{rfc822(Blog.last_updated())}</lastBuildDate>
    #{items}
      </channel>
    </rss>
    """
  end

  @spec atom([Blog.Post.t()]) :: String.t()
  def atom(posts) do
    entries =
      Enum.map_join(posts, "\n", fn post ->
        authors =
          Enum.map_join(post.authors, "\n", fn author ->
            """
                <author>
                  <name>#{escape(author.name)}</name>
                </author>\
            """
          end)

        """
          <entry>
            <title>#{escape(post.title)}</title>
            <link href="#{post_url(post)}" />
            <id>#{post_url(post)}</id>
            <published>#{iso8601(post.date)}</published>
            <updated>#{iso8601(post.date)}</updated>
            <summary>#{escape(post.description)}</summary>
        #{authors}
            <content type="html"><![CDATA[#{cdata(post.body)}]]></content>
          </entry>\
        """
      end)

    """
    <?xml version="1.0" encoding="UTF-8"?>
    <feed xmlns="http://www.w3.org/2005/Atom">
      <title>Once blog</title>
      <subtitle>Ideas and updates from the people building Once.</subtitle>
      <link href="#{absolute_url("/blog")}" />
      <link href="#{absolute_url("/blog/feed.atom")}" rel="self" type="application/atom+xml" />
      <id>#{absolute_url("/blog")}</id>
      <updated>#{iso8601(Blog.last_updated())}</updated>
    #{entries}
    </feed>
    """
  end

  defp post_url(post), do: absolute_url("/blog/#{post.slug}")
  defp absolute_url(path), do: Endpoint.url() <> path

  defp escape(text) do
    text
    |> to_string()
    |> String.replace("&", "&amp;")
    |> String.replace("<", "&lt;")
    |> String.replace(">", "&gt;")
    |> String.replace("\"", "&quot;")
    |> String.replace("'", "&apos;")
  end

  defp cdata(html), do: String.replace(html, "]]>", "]]]]><![CDATA[>")

  defp rfc822(date) do
    {:ok, datetime} = DateTime.new(date, ~T[00:00:00], "Etc/UTC")
    Calendar.strftime(datetime, "%a, %d %b %Y %H:%M:%S +0000")
  end

  defp iso8601(date), do: Date.to_iso8601(date) <> "T00:00:00Z"
end
