export const site = {
  description:
    "Once makes repository automation graph-aware, cacheable, observable, and remotely executable.",
  nav: [
    { text: "Get Started", link: "/guide/getting-started" },
    { text: "Guides", link: "/guide/scripted/" },
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
      { text: "Manifest", link: "/reference/manifest" },
      {
        text: "Command Line",
        collapsed: false,
        items: [
          { text: "Overview", link: "/reference/cli/" },
          { text: "auth", link: "/reference/cli/auth" },
          { text: "build", link: "/reference/cli/build" },
          { text: "cache", link: "/reference/cli/cache" },
          { text: "edit", link: "/reference/cli/edit" },
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
        text: "Modules",
        collapsed: false,
        items: [{ text: "Overview", link: "/reference/modules/" }],
      },
      {
        text: "Target Kinds",
        collapsed: false,
        items: [
          { text: "Overview", link: "/reference/prelude/" },
          {
            text: "Apple",
            collapsed: true,
            items: [
              { text: "apple_library", link: "/reference/prelude/apple_library" },
              { text: "swift_macro", link: "/reference/prelude/swift_macro" },
              { text: "apple_framework", link: "/reference/prelude/apple_framework" },
              { text: "apple_application", link: "/reference/prelude/apple_application" },
              { text: "apple_test_bundle", link: "/reference/prelude/apple_test_bundle" },
              { text: "swift_package_dependencies", link: "/reference/prelude/swift_package_dependencies" },
              { text: "swift_package_pin", link: "/reference/prelude/swift_package_pin" },
            ],
          },
          {
            text: "Android",
            collapsed: true,
            items: [
              { text: "android_resource", link: "/reference/prelude/android_resource" },
              { text: "android_library", link: "/reference/prelude/android_library" },
              { text: "android_local_test", link: "/reference/prelude/android_local_test" },
              { text: "android_instrumentation_test", link: "/reference/prelude/android_instrumentation_test" },
              { text: "android_binary", link: "/reference/prelude/android_binary" },
            ],
          },
          {
            text: "Cross-platform",
            collapsed: true,
            items: [
              { text: "swift_android_library", link: "/reference/prelude/swift_android_library" },
              { text: "kotlin_apple_framework", link: "/reference/prelude/kotlin_apple_framework" },
            ],
          },
          {
            text: "Kotlin",
            collapsed: true,
            items: [
              { text: "kotlin_jvm_library", link: "/reference/prelude/kotlin_jvm_library" },
              { text: "kotlin_jvm_binary", link: "/reference/prelude/kotlin_jvm_binary" },
              { text: "kotlin_jvm_test", link: "/reference/prelude/kotlin_jvm_test" },
            ],
          },
          {
            text: "C and C++",
            collapsed: true,
            items: [
              { text: "c_library", link: "/reference/prelude/c_library" },
            ],
          },
          {
            text: "Elixir",
            collapsed: true,
            items: [
              { text: "mix_dependencies", link: "/reference/prelude/mix_dependencies" },
              { text: "mix_package", link: "/reference/prelude/mix_package" },
              { text: "elixir_library", link: "/reference/prelude/elixir_library" },
              { text: "elixir_test", link: "/reference/prelude/elixir_test" },
            ],
          },
          {
            text: "Python",
            collapsed: true,
            items: [
              { text: "pytest_test", link: "/reference/prelude/pytest_test" },
            ],
          },
          {
            text: "Ruby",
            collapsed: true,
            items: [
              { text: "rspec_test", link: "/reference/prelude/rspec_test" },
              { text: "minitest_test", link: "/reference/prelude/minitest_test" },
            ],
          },
          {
            text: "JavaScript",
            collapsed: true,
            items: [
              { text: "vitest_test", link: "/reference/prelude/vitest_test" },
              { text: "jest_test", link: "/reference/prelude/jest_test" },
            ],
          },
          {
            text: "Rust",
            collapsed: true,
            items: [
              { text: "cargo_dependencies", link: "/reference/prelude/cargo_dependencies" },
              { text: "rust_library", link: "/reference/prelude/rust_library" },
              { text: "rust_mobile_library", link: "/reference/prelude/rust_mobile_library" },
              { text: "rust_binary", link: "/reference/prelude/rust_binary" },
              { text: "rust_test", link: "/reference/prelude/rust_test" },
              { text: "rust_crate", link: "/reference/prelude/rust_crate" },
              { text: "rust_proc_macro", link: "/reference/prelude/rust_proc_macro" },
            ],
          },
          {
            text: "Zig",
            collapsed: true,
            items: [
              { text: "zig_dependencies", link: "/reference/prelude/zig_dependencies" },
              { text: "zig_package", link: "/reference/prelude/zig_package" },
              { text: "zig_library", link: "/reference/prelude/zig_library" },
              { text: "zig_c_library", link: "/reference/prelude/zig_c_library" },
              { text: "zig_binary", link: "/reference/prelude/zig_binary" },
              { text: "zig_static_library", link: "/reference/prelude/zig_static_library" },
              { text: "zig_shared_library", link: "/reference/prelude/zig_shared_library" },
              { text: "zig_test", link: "/reference/prelude/zig_test" },
              { text: "zig_configure", link: "/reference/prelude/zig_configure" },
              { text: "zig_configure_binary", link: "/reference/prelude/zig_configure_binary" },
              { text: "zig_configure_test", link: "/reference/prelude/zig_configure_test" },
            ],
          },
        ],
      },
      {
        text: "Memory",
        collapsed: false,
        items: [{ text: "Overview", link: "/reference/memory/" }],
      },
      {
        text: "Model Context Protocol",
        collapsed: false,
        items: [
          { text: "Overview", link: "/reference/mcp/" },
          { text: "Tools", link: "/reference/mcp/tools" },
        ],
      },
    ],
    "/": [
      {
        text: "Start Here",
        collapsed: false,
        items: [
          { text: "Why Once", link: "/guide/why" },
          { text: "Getting Started", link: "/guide/getting-started" },
          { text: "Coding Harnesses", link: "/guide/harness" },
        ],
      },
      {
        text: "Scripted Automation",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/scripted/" },
          { text: "Caching", link: "/guide/scripted/caching" },
          { text: "Manual Cache Access", link: "/guide/scripted/runtime" },
        ],
      },
      {
        text: "Typed Graph",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/graph/" },
          { text: "Ecosystems", link: "/guide/graph/ecosystems" },
          { text: "Testing and Scheduling", link: "/guide/graph/testing" },
          { text: "Apple", link: "/guide/graph/apple" },
          { text: "Swift Packages", link: "/guide/graph/swift-packages" },
          { text: "Android", link: "/guide/graph/android" },
          { text: "C and C++", link: "/guide/graph/c" },
          { text: "Elixir", link: "/guide/graph/elixir" },
          { text: "Kotlin", link: "/guide/graph/kotlin" },
          { text: "Rust", link: "/guide/graph/rust" },
          { text: "Zig", link: "/guide/graph/zig" },
        ],
      },
      {
        text: "Infrastructure",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/infrastructure/" },
          { text: "Remote Execution", link: "/guide/infrastructure/remote-execution" },
          { text: "Microsandbox", link: "/guide/infrastructure/microsandbox" },
          { text: "E2B", link: "/guide/infrastructure/e2b" },
          { text: "Daytona", link: "/guide/infrastructure/daytona" },
          { text: "Tuist", link: "/guide/infrastructure/tuist" },
        ],
      },
      {
        text: "Memory",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/memory/" },
          { text: "Evidence", link: "/guide/memory/evidence" },
        ],
      },
      {
        text: "Language Libraries",
        collapsed: false,
        items: [
          { text: "Overview", link: "/guide/sdk/" },
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
          { text: "C", link: "/guide/sdk/c" },
        ],
      },
    ],
  },
  footer: {
    message: "Released under an open-source license.",
    copyright: "Copyright © Tuist GmbH",
  },
};
