export const site = {
  description:
    "Once makes repository automation graph-aware, cacheable, observable, and remotely executable.",
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
          { text: "mcp", link: "/reference/cli/mcp" },
          { text: "query", link: "/reference/cli/query" },
          { text: "run", link: "/reference/cli/run" },
          { text: "runtime", link: "/reference/cli/runtime" },
          { text: "test", link: "/reference/cli/test" },
          { text: "toolchain", link: "/reference/cli/toolchain" },
        ],
      },
      {
        text: "Prelude",
        collapsed: false,
        items: [
          { text: "Overview", link: "/reference/prelude/" },
          { text: "Starlark rules", link: "/reference/prelude/starlark-rules" },
          { text: "apple_library", link: "/reference/prelude/apple_library" },
          { text: "swift_macro", link: "/reference/prelude/swift_macro" },
          { text: "apple_framework", link: "/reference/prelude/apple_framework" },
          { text: "apple_application", link: "/reference/prelude/apple_application" },
          { text: "apple_test_bundle", link: "/reference/prelude/apple_test_bundle" },
          { text: "cargo_dependencies", link: "/reference/prelude/cargo_dependencies" },
          { text: "rust_library", link: "/reference/prelude/rust_library" },
          { text: "rust_binary", link: "/reference/prelude/rust_binary" },
          { text: "rust_crate", link: "/reference/prelude/rust_crate" },
          { text: "rust_proc_macro", link: "/reference/prelude/rust_proc_macro" },
        ],
      },
      {
        text: "MCP",
        collapsed: false,
        items: [
          { text: "Overview", link: "/reference/mcp/" },
          { text: "Tools", link: "/reference/mcp/tools" },
        ],
      },
    ],
    "/": [
      { text: "Why", link: "/guide/why" },
      {
        text: "Graph",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/graph/" },
          { text: "Apple", link: "/guide/graph/apple" },
          { text: "Rust", link: "/guide/graph/rust" },
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
