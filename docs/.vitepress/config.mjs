import { defineConfig } from "vitepress";

const targetText = (label, icon) =>
  `<span class="sidebar-target-link sidebar-target-link-${icon}"><span class="sidebar-target-icon" aria-hidden="true"></span><span>${label}</span></span>`;

export default defineConfig({
  title: "Fabrik",
  titleTemplate: ":title | Fabrik",
  description:
    "Fabrik gives developers and agents one execution layer that maps software operations onto cache and compute infrastructure.",
  cleanUrls: true,
  lastUpdated: true,
  sitemap: {
    hostname: "https://fabrik.run",
  },
  head: [
    ["link", { rel: "icon", type: "image/png", href: "/favicon.png" }],
    ["meta", { name: "theme-color", content: "#3c4858" }],
  ],
  themeConfig: {
    logo: "/logo.png",
    search: {
      provider: "local",
    },
    editLink: {
      pattern: "https://github.com/tuist/fabrik/edit/main/docs/:path",
      text: "Edit this page on GitHub",
    },
    nav: [
      { text: "Docs", link: "/guide/what-is-fabrik" },
      {
        text: "Links",
        items: [
          {
            text: "Releases",
            link: "https://github.com/tuist/fabrik/releases",
          },
          {
            text: "Issues",
            link: "https://github.com/tuist/fabrik/issues",
          },
        ],
      },
    ],
    sidebar: {
      "/": [
        {
          text: "Documentation",
          items: [
            {
              text: "Introduction",
              items: [
                { text: "What is Fabrik", link: "/guide/what-is-fabrik" },
              ],
            },
            {
              text: "Native Rules",
              items: [
                { text: targetText("Rust", "rust"), link: "/targets/rust" },
                {
                  text: targetText("Apple and Swift", "swift"),
                  link: "/targets/apple",
                },
                { text: targetText("Go", "go"), link: "/targets/go" },
                {
                  text: targetText("Elixir", "elixir"),
                  link: "/targets/elixir",
                },
              ],
            },
            {
              text: "Scripts",
              items: [
                { text: "Overview", link: "/targets/actions" },
                {
                  text: "Script Files",
                  link: "/guide/cacheable-scripts",
                },
                { text: "Script Rules", link: "/targets/scripts" },
              ],
            },
          ],
        },
      ],
    },
    socialLinks: [
      { icon: "github", link: "https://github.com/tuist/fabrik" },
    ],
    footer: {
      message: "Released under the MIT License.",
      copyright: "Copyright © Tuist GmbH",
    },
  },
});
