defmodule OnceSiteWeb.PageHTML do
  @moduledoc """
  This module contains pages rendered by PageController.

  See the `page_html` directory for all templates available.
  """
  use OnceSiteWeb, :html

  embed_templates "page_html/*"

  @icons_path Path.join([File.cwd!(), "deps", "noora", "lib", "noora", "icons"])
  @icon_package @icons_path |> Path.join("package.svg") |> File.read!() |> String.trim()
  @icon_reload @icons_path |> Path.join("reload.svg") |> File.read!() |> String.trim()
  @icon_server @icons_path |> Path.join("server.svg") |> File.read!() |> String.trim()
  @icon_github @icons_path |> Path.join("brand-github.svg") |> File.read!() |> String.trim()

  @doc false
  def github_icon, do: @icon_github

  @doc false
  def features do
    [
      %{
        title: "Build once",
        icon: @icon_package,
        link: "/docs/guide/scripted/caching",
        details:
          "Give every action explicit inputs, outputs, and environment. Results are content-addressed, so a build only ever runs when something actually changed."
      },
      %{
        title: "Reuse everywhere",
        icon: @icon_reload,
        link: "/docs/guide/scripted",
        details:
          "Restore outputs instantly from a local or shared cache across developers, coding agents, CI, and machines. Same inputs, same result, no rebuild."
      },
      %{
        title: "Run anywhere",
        icon: @icon_server,
        link: "/docs/guide/infrastructure/remote-execution",
        details:
          "Send only the declared inputs to a fresh local or hosted sandbox, run the action, and retrieve just the declared outputs."
      }
    ]
  end

  @doc false
  def terminals do
    [
      %{
        label: "iOS",
        icon: "apple",
        command: "once build apps/ios/App",
        target: "apple_application",
        cache: "9f21c4…",
        result: "hit",
        duration: "0.8s"
      },
      %{
        label: "Android",
        icon: "android",
        command: "once build apps/android/App",
        target: "android_binary",
        cache: "1c8ea7…",
        result: "miss",
        duration: "41s"
      },
      %{
        label: "Rust",
        icon: "rust",
        command: "once build crates/engine",
        target: "rust_binary",
        cache: "b3f9d0…",
        result: "hit",
        duration: "0.1s"
      },
      %{
        label: "Zig",
        icon: "zig",
        command: "once build zig/app",
        target: "zig_binary",
        cache: "77aa02…",
        result: "hit",
        duration: "0.3s"
      }
    ]
  end

  @doc false
  def languages do
    [
      %{name: "Swift", icon: "swift", link: "/docs/guide/graph/swift-packages"},
      %{name: "Apple", icon: "apple", link: "/docs/guide/graph/apple"},
      %{name: "Kotlin", icon: "kotlin", link: "/docs/guide/graph/kotlin"},
      %{name: "Android", icon: "android", link: "/docs/guide/graph/android"},
      %{name: "Rust", icon: "rust", link: "/docs/guide/graph/rust"},
      %{name: "Go", icon: "go", link: "/docs/guide/graph/go"},
      %{name: "C / C++", icon: "cplusplus", link: "/docs/guide/graph/c"},
      %{name: "Elixir", icon: "elixir", link: "/docs/guide/graph/elixir"},
      %{name: "Ruby", icon: "ruby", link: "/docs/guide/sdk/ruby"},
      %{name: "JavaScript", icon: "javascript", link: "/docs/guide/sdk/javascript"},
      %{name: "Zig", icon: "zig", link: "/docs/guide/graph/zig"},
      %{name: "React Native", icon: "react-native", link: "/docs/guide/graph/react-native"}
    ]
  end
end
