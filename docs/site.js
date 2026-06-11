export const site = {
  description:
    "Once makes project automation cacheable, observable, and remotely executable.",
  nav: [
    { text: "Docs", link: "/" },
    { text: "Reference", link: "/reference/" },
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
    "/reference/": [
      { text: "Reference", link: "/reference/" },
      {
        text: "CLI",
        collapsed: false,
        items: [
          { text: "Overview", link: "/reference/cli/" },
          { text: "auth", link: "/reference/cli/auth" },
          { text: "build", link: "/reference/cli/build" },
          { text: "cache", link: "/reference/cli/cache" },
          { text: "exec", link: "/reference/cli/exec" },
          { text: "query", link: "/reference/cli/query" },
          { text: "run", link: "/reference/cli/run" },
          { text: "runtime", link: "/reference/cli/runtime" },
          { text: "test", link: "/reference/cli/test" },
          { text: "toolchain", link: "/reference/cli/toolchain" },
        ],
      },
    ],
    "/": [
      { text: "Why", link: "/guide/why" },
      {
        text: "Scripts",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/scripts/" },
          { text: "Caching", link: "/guide/scripts/caching" },
          { text: "Runtime", link: "/guide/scripts/runtime" },
        ],
      },
      {
        text: "Graph",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/graph/" },
          { text: "Apple", link: "/guide/graph/apple" },
        ],
      },
      {
        text: "SDK",
        collapsed: false,
        items: [
          {
            text: '<span class="sidebar-target-link sidebar-target-link-rust"><span class="sidebar-target-icon"></span><span>Rust</span></span>',
            link: "/guide/sdk/rust",
          },
          {
            text: '<span class="sidebar-target-link sidebar-target-link-swift"><span class="sidebar-target-icon"></span><span>Swift</span></span>',
            link: "/guide/sdk/swift",
          },
          {
            text: '<span class="sidebar-target-link sidebar-target-link-ruby"><span class="sidebar-target-icon"></span><span>Ruby</span></span>',
            link: "/guide/sdk/ruby",
          },
          {
            text: '<span class="sidebar-target-link sidebar-target-link-javascript"><span class="sidebar-target-icon"></span><span>JavaScript</span></span>',
            link: "/guide/sdk/javascript",
          },
        ],
      },
    ],
  },
  footer: {
    message: "Released under the MIT License.",
    copyright: "Copyright © Tuist GmbH",
  },
};
