defmodule OnceSiteWeb.Docs.HTMLTest do
  use ExUnit.Case, async: true

  alias OnceSiteWeb.Docs.HTML

  test "wrap_code_blocks preserves whitespace between highlighting spans" do
    # Simulates the Lumis multi-theme output: one span per token with literal
    # spaces between them. String processing must keep those spaces (a Floki
    # round-trip would drop the whitespace-only text nodes).
    highlighted =
      ~s(<pre class="lumis"><code class="language-bash"><span>once</span> <span>query</span> <span>evidence</span></code></pre>)

    html = HTML.wrap_code_blocks(highlighted)

    assert html =~ ~s(<span>once</span> <span>query</span> <span>evidence</span>)
    assert html =~ ~s(data-part="code-window")
    # Copy source is the plain text with spaces intact.
    assert html =~ ~s(<template data-part="copy-source">once query evidence</template>)
  end

  test "add_heading_anchors rewrites MDEx heading anchors" do
    mdex = ~s(<h2 id="my-section">My Section<a href="#my-section" class="anchor"></a></h2>)

    html = HTML.add_heading_anchors(mdex)

    assert html =~ ~s(<a class="heading-anchor" id="my-section" href="#my-section">)
    assert html =~ ~s(<span data-part="heading-text">My Section</span>)
  end

  test "rewrite_links prefixes internal links with /docs" do
    assert HTML.rewrite_links(~s(<a href="/guide/why">x</a>)) =~ ~s(href="/docs/guide/why")
    assert HTML.rewrite_links(~s(<a href="/reference">x</a>)) =~ ~s(href="/docs/reference")
    assert HTML.rewrite_links(~s(<a href="https://x.com">x</a>)) =~ ~s(href="https://x.com")
  end

  test "wrap_tables leaves code windows untouched" do
    html =
      HTML.wrap_tables(
        ~s(<div data-part="code"><code><span>a</span>  <span>b</span></code></div>)
      )

    assert html =~ ~s(<span>a</span>  <span>b</span>)
  end
end
