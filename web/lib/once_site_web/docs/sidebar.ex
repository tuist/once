defmodule OnceSiteWeb.Docs.Sidebar do
  @moduledoc """
  Documentation navigation trees, top navigation, and footer.

  Two trees are selected by URL prefix (`tab_for_slug/1`): the reference tree
  under `/docs/reference` and the guide tree everywhere else. Slugs are the full
  `/docs/...` paths, so no path rewriting is needed at render time.
  """

  defmodule Item do
    @moduledoc false
    defstruct [:label, :slug, :url, :icon, items: []]
  end

  defmodule Group do
    @moduledoc false
    defstruct [:label, items: []]
  end

  @doc "Top navigation shown in the docs header."
  def nav do
    [
      %{text: "Guide", href: "/docs/guide", external: false},
      %{text: "Reference", href: "/docs/reference", external: false},
      %{text: "Releases", href: "https://github.com/tuist/once/releases", external: true},
      %{text: "GitHub", href: "https://github.com/tuist/once", external: true}
    ]
  end

  @doc "Footer text."
  def footer do
    %{message: "Released under an open-source license.", copyright: "Copyright © Tuist GmbH"}
  end

  @doc "Which tab (and therefore tree) a slug belongs to."
  def tab_for_slug("/docs/reference" <> _), do: :reference
  def tab_for_slug(_), do: :guides

  def tree_for_tab(:reference), do: reference_tree()
  def tree_for_tab(_), do: guide_tree()

  def item_active?(%Item{slug: slug}, current_slug) when is_binary(slug), do: slug == current_slug
  def item_active?(%Item{}, _current_slug), do: false

  def item_or_children_active?(%Item{slug: slug, items: items}, current_slug) do
    slug == current_slug or Enum.any?(items, &item_or_children_active?(&1, current_slug))
  end

  def guide_tree do
    [
      %Group{
        label: "Start Here",
        items: [
          %Item{label: "Overview", slug: "/docs/guide"},
          %Item{label: "Why Once", slug: "/docs/guide/why"},
          %Item{label: "Getting Started", slug: "/docs/guide/getting-started"},
          %Item{label: "Coding Harnesses", slug: "/docs/guide/harness"}
        ]
      },
      %Group{
        label: "Scripted Automation",
        items: [
          %Item{label: "Overview", slug: "/docs/guide/scripted"},
          %Item{label: "Caching", slug: "/docs/guide/scripted/caching"},
          %Item{label: "Manual Cache Access", slug: "/docs/guide/scripted/runtime"}
        ]
      },
      %Group{
        label: "Typed Graph",
        items: [
          %Item{label: "Overview", slug: "/docs/guide/graph"},
          %Item{label: "Ecosystems", slug: "/docs/guide/graph/ecosystems"},
          %Item{label: "Configurations", slug: "/docs/guide/graph/configuration"},
          %Item{label: "Testing and Scheduling", slug: "/docs/guide/graph/testing"},
          %Item{label: "Linting", slug: "/docs/guide/graph/linting"},
          %Item{
            label: "Apple",
            slug: "/docs/guide/graph/apple",
            icon: "apple",
            items: [
              %Item{label: "Xcode Projects", slug: "/docs/guide/graph/apple/xcode"}
            ]
          },
          %Item{label: "Swift Packages", slug: "/docs/guide/graph/swift-packages", icon: "swift"},
          %Item{label: "Android", slug: "/docs/guide/graph/android", icon: "android"},
          %Item{label: "C and C++", slug: "/docs/guide/graph/c", icon: "cplusplus"},
          %Item{label: "CMake", slug: "/docs/guide/graph/cmake", icon: "cplusplus"},
          %Item{label: "Elixir", slug: "/docs/guide/graph/elixir", icon: "elixir"},
          %Item{label: "Kotlin", slug: "/docs/guide/graph/kotlin", icon: "kotlin"},
          %Item{label: "Go", slug: "/docs/guide/graph/go", icon: "go"},
          %Item{label: "Rust", slug: "/docs/guide/graph/rust", icon: "rust"},
          %Item{label: "Zig", slug: "/docs/guide/graph/zig", icon: "zig"},
          %Item{
            label: "React Native",
            slug: "/docs/guide/graph/react-native",
            icon: "react-native"
          }
        ]
      },
      %Group{
        label: "Local Execution",
        items: [
          %Item{label: "Memory Limits", slug: "/docs/guide/local-execution/memory-limits"},
          %Item{
            label: "Unchanged Builds",
            slug: "/docs/guide/local-execution/unchanged-builds"
          }
        ]
      },
      %Group{
        label: "Infrastructure",
        items: [
          %Item{label: "Overview", slug: "/docs/guide/infrastructure"},
          %Item{label: "Remote Execution", slug: "/docs/guide/infrastructure/remote-execution"},
          %Item{
            label: "Microsandbox",
            slug: "/docs/guide/infrastructure/microsandbox",
            icon: "microsandbox"
          },
          %Item{label: "E2B", slug: "/docs/guide/infrastructure/e2b", icon: "e2b"},
          %Item{
            label: "Daytona",
            slug: "/docs/guide/infrastructure/daytona",
            icon: "daytona"
          },
          %Item{label: "Tuist", slug: "/docs/guide/infrastructure/tuist", icon: "tuist"}
        ]
      },
      %Group{
        label: "Memory",
        items: [
          %Item{label: "Overview", slug: "/docs/guide/memory"},
          %Item{label: "Evidence", slug: "/docs/guide/memory/evidence"}
        ]
      },
      %Group{
        label: "Language Libraries",
        items: [
          %Item{label: "Overview", slug: "/docs/guide/sdk"},
          %Item{label: "Rust", slug: "/docs/guide/sdk/rust", icon: "rust"},
          %Item{label: "Swift", slug: "/docs/guide/sdk/swift", icon: "swift"},
          %Item{label: "Ruby", slug: "/docs/guide/sdk/ruby", icon: "ruby"},
          %Item{label: "JavaScript", slug: "/docs/guide/sdk/javascript", icon: "javascript"},
          %Item{label: "C", slug: "/docs/guide/sdk/c", icon: "c"}
        ]
      }
    ]
  end

  def reference_tree do
    [
      %Group{
        label: "References",
        items: [
          %Item{label: "Overview", slug: "/docs/reference"},
          %Item{label: "Manifest", slug: "/docs/reference/manifest"},
          %Item{
            label: "Modules",
            slug: "/docs/reference/modules",
            items: [
              %Item{
                label: "Custom Lint Targets",
                slug: "/docs/reference/modules/linting"
              }
            ]
          },
          %Item{label: "Memory", slug: "/docs/reference/memory"}
        ]
      },
      %Group{
        label: "Commands",
        items: [
          %Item{label: "Overview", slug: "/docs/reference/cli"},
          %Item{label: "auth", slug: "/docs/reference/cli/auth"},
          %Item{label: "build", slug: "/docs/reference/cli/build"},
          %Item{label: "cache", slug: "/docs/reference/cli/cache"},
          %Item{label: "edit", slug: "/docs/reference/cli/edit"},
          %Item{label: "exec", slug: "/docs/reference/cli/exec"},
          %Item{label: "lint", slug: "/docs/reference/cli/lint"},
          %Item{label: "mcp", slug: "/docs/reference/cli/mcp"},
          %Item{label: "query", slug: "/docs/reference/cli/query"},
          %Item{label: "run", slug: "/docs/reference/cli/run"},
          %Item{label: "runtime", slug: "/docs/reference/cli/runtime"},
          %Item{label: "test", slug: "/docs/reference/cli/test"},
          %Item{label: "toolchain", slug: "/docs/reference/cli/toolchain"}
        ]
      },
      %Group{
        label: "Target Kinds",
        items:
          [%Item{label: "Overview", slug: "/docs/reference/prelude"}] ++
            [
              target_group(
                "Linting",
                ~w(ruff_lint eslint_lint golangci_lint swiftlint_lint detekt_lint credo_lint rubocop_lint)
              ),
              target_group("Apple", ~w(apple_library swift_macro apple_framework apple_application
                apple_resource_bundle apple_thinned_package apple_test_bundle
                apple_xcframework_import swift_package_dependencies swift_package_pin)),
              target_group("Xcode", ~w(xcode_workspace)),
              target_group("Android", ~w(android_resource android_library android_local_test
                android_instrumentation_test android_binary)),
              target_group("Cross-platform", ~w(swift_android_library kotlin_apple_framework)),
              target_group("Kotlin", ~w(kotlin_jvm_library kotlin_jvm_binary kotlin_jvm_test)),
              target_group("C and C++", ~w(c_library)),
              target_group("CMake", ~w(cmake_project cmake_workspace cmake_target)),
              target_group("Elixir", ~w(mix_dependencies mix_package elixir_library elixir_test)),
              target_group("Python", ~w(pytest_test)),
              target_group("Ruby", ~w(rspec_test minitest_test)),
              target_group("JavaScript", ~w(vitest_test jest_test)),
              target_group(
                "Go",
                ~w(go_dependencies go_module go_source go_library go_binary go_test)
              ),
              target_group("Rust", ~w(cargo_dependencies rust_library rust_mobile_library
                rust_binary rust_test rust_crate rust_proc_macro)),
              target_group("Zig", ~w(zig_dependencies zig_package zig_library zig_c_library
                zig_binary zig_static_library zig_shared_library zig_test zig_configure
                zig_configure_binary zig_configure_test))
            ]
      },
      %Group{
        label: "Model Context Protocol",
        items: [
          %Item{label: "Overview", slug: "/docs/reference/mcp"},
          %Item{label: "Tools", slug: "/docs/reference/mcp/tools"}
        ]
      }
    ]
  end

  defp target_group(label, kinds) do
    %Item{
      label: label,
      items: Enum.map(kinds, &%Item{label: &1, slug: "/docs/reference/prelude/#{&1}"})
    }
  end
end
