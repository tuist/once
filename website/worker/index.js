const wantsMarkdown = (request) => {
  const accept = (request.headers.get("accept") || "").toLowerCase();
  if (accept.includes("text/markdown")) return true;
  if (accept.includes("text/x-markdown")) return true;
  return false;
};

const mdUrlFor = (urlString) => {
  const url = new URL(urlString);
  url.pathname = url.pathname.endsWith("/")
    ? `${url.pathname}index.md`
    : `${url.pathname}.md`;
  return url;
};

const linkHeader = (request) => {
  const url = new URL(request.url);
  const md = mdUrlFor(request.url).toString();
  return [
    `<${md}>; rel="alternate"; type="text/markdown"`,
    `<${url.origin}/sitemap.xml>; rel="sitemap"; type="application/xml"`,
    `<${url.origin}/llms.txt>; rel="describedby"; type="text/plain"`,
  ].join(", ");
};

const isHtmlPath = (pathname) =>
  pathname.endsWith("/") ||
  pathname.endsWith(".html") ||
  !/\.[a-z0-9]+$/i.test(pathname);

const withLinkHeader = (response, request) => {
  if (!isHtmlPath(new URL(request.url).pathname)) return response;
  const headers = new Headers(response.headers);
  headers.set("Link", linkHeader(request));
  headers.set("Vary", "Accept");
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
};

export default {
  async fetch(request, env) {
    if (wantsMarkdown(request)) {
      const mdUrl = mdUrlFor(request.url);
      const mdReq = new Request(mdUrl.toString(), {
        method: request.method,
        headers: request.headers,
      });
      const mdResp = await env.ASSETS.fetch(mdReq);
      if (mdResp.ok) {
        const headers = new Headers(mdResp.headers);
        headers.set("Content-Type", "text/markdown; charset=utf-8");
        headers.set("Vary", "Accept");
        return new Response(mdResp.body, {
          status: mdResp.status,
          statusText: mdResp.statusText,
          headers,
        });
      }
    }

    const response = await env.ASSETS.fetch(request);
    return withLinkHeader(response, request);
  },
};
