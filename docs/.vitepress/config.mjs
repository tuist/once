import { defineConfig } from "vitepress";

export default defineConfig({
  title: "Once",
  titleTemplate: false,
  description:
    "Once makes project scripts cacheable, observable, and remotely executable.",
  cleanUrls: true,
  lastUpdated: true,
  sitemap: {
    hostname: "https://once.tuist.dev",
  },
  head: [
    ["link", { rel: "icon", type: "image/png", href: "/favicon.png" }],
    ["meta", { name: "theme-color", content: "#3c4858" }],
  ],
  themeConfig: {
    logo: "/nav-logo.png",
    search: {
      provider: "local",
    },
    editLink: {
      pattern: "https://github.com/tuist/once/edit/main/docs/:path",
      text: "Edit this page on GitHub",
    },
    nav: [
      { text: "Docs", link: "/" },
      {
        text: "Links",
        items: [
          {
            text: "Releases",
            link: "https://github.com/tuist/once/releases",
          },
          {
            text: "Issues",
            link: "https://github.com/tuist/once/issues",
          },
        ],
      },
    ],
    sidebar: {
      "/": [
        { text: "What is Once", link: "/guide/what-is-once" },
        { text: "Caching", link: "/guide/cacheable-scripts" },
        { text: "Runtime", link: "/guide/runtime" },
      ],
    },
    socialLinks: [{ icon: "github", link: "https://github.com/tuist/once" }],
    footer: {
      message: "Released under the MIT License.",
      copyright: "Copyright © Tuist GmbH",
    },
  },
});
