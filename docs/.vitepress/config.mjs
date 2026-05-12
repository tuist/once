import { defineConfig } from "vitepress";

export default defineConfig({
  title: "Fabrik",
  titleTemplate: ":title | Fabrik",
  description:
    "A polyglot, agent-native build system with content-addressed actions, structured declarations, and explicit runtime semantics.",
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
      { text: "Guide", link: "/guide/what-is-fabrik" },
      { text: "Targets", link: "/targets/rust" },
      { text: "Reference", link: "/reference/design" },
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
      "/guide/": [
        {
          text: "Introduction",
          items: [
            { text: "What is Fabrik", link: "/guide/what-is-fabrik" },
          ],
        },
        {
          text: "Concepts",
          items: [
            { text: "Project layout", link: "/guide/project-layout" },
            { text: "Target IDs", link: "/guide/target-ids" },
            { text: "Cache and execution", link: "/guide/cache-and-execution" },
            { text: "Agent-native features", link: "/guide/agent-native" },
          ],
        },
      ],
      "/targets/": [
        {
          text: "Targets",
          items: [
            { text: "Rust", link: "/targets/rust" },
            { text: "Apple and Swift", link: "/targets/apple" },
            { text: "Tasks", link: "/targets/tasks" },
          ],
        },
      ],
      "/reference/": [
        {
          text: "Reference",
          items: [
            { text: "Rules and targets", link: "/reference/rules" },
            { text: "Runtime targets", link: "/reference/runtime" },
            { text: "Design", link: "/reference/design" },
            { text: "Roadmap", link: "/reference/roadmap" },
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
