export const site = {
  description:
    "Once makes project automation cacheable, observable, and remotely executable.",
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
      { text: "Why", link: "/guide/why" },
      {
        text: "SDK",
        collapsed: false,
        items: [
          { text: "Rust", link: "/guide/sdk/rust" },
          { text: "Swift", link: "/guide/sdk/swift" },
        ],
      },
      {
        text: "Scripts",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/scripts/" },
          { text: "Caching", link: "/guide/scripts/caching" },
          { text: "Runtime", link: "/guide/scripts/runtime" },
        ],
      },
    ],
  },
  footer: {
    message: "Released under the MIT License.",
    copyright: "Copyright © Tuist GmbH",
  },
};
