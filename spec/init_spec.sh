#shellcheck shell=bash
# End-to-end specs for `fabrik init`.

Describe 'fabrik init'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  rust_project() {
    fabrik init rust-app \
      --path rust-app \
      --set project_name=hello \
      --set library_name=greeting \
      --set greeting_subject=Rust >/dev/null 2>&1
  }

  elixir_project() {
    fabrik init elixir-app \
      --path elixir-app \
      --set binary_name=hello \
      --set library_name=greeting \
      --set entry_module=Hello \
      --set library_module=Greeting >/dev/null 2>&1
  }

  go_project() {
    fabrik init go-app \
      --path go-app \
      --set target_name=app \
      --set module_path=example.com/app >/dev/null 2>&1
  }

  apple_macos_project() {
    fabrik init macos-cli \
      --path macos-app \
      --set library_name=Greeter \
      --set binary_name=hello \
      --set module_name=Hello \
      --set minimum_os=15.0 >/dev/null 2>&1
  }

  apple_ios_project() {
    fabrik init ios-app \
      --path ios-app \
      --set app_name=Demo \
      --set bundle_id=dev.fabrik.demo \
      --set minimum_os=17.0 >/dev/null 2>&1
  }

  It 'lists vendored templates'
    When call fabrik init --templates
    The status should be success
    The stdout should include 'rust-app'
    The stdout should include 'elixir-app'
    The stdout should include 'go-app'
    The stdout should include 'macos-cli'
    The stdout should include 'ios-app'
  End

  It 'emits structured template selection data for agents'
    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" init
    The status should not equal 0
    The stdout should include '"status":"select_template"'
    The stdout should include '"id":"rust-app"'
    The stdout should include '"prompts":'
  End

  It 'emits structured prompt requirements for agents'
    When call "$FABRIK_BIN" --format json -C "$WORKSPACE" init go-app --set target_name=app
    The status should not equal 0
    The stdout should include '"status":"needs_input"'
    The stdout should include '"name":"module_path"'
  End

  It 'creates a Rust project that builds and tests'
    rust_project
    When call fabrik test rust-app/greeting_test
    The status should be success
    The stdout should include 'test result: ok'
    The stderr should include 'build 2 nodes'
  End

  It 'creates an Elixir project that builds and runs'
    elixir_missing() { ! command -v elixir >/dev/null 2>&1 || ! command -v elixirc >/dev/null 2>&1; }
    Skip if 'elixir is not installed' elixir_missing
    elixir_missing && return 0

    elixir_project
    fabrik build elixir-app/hello >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/elixir-app/hello"
    The status should be success
    The stdout should equal 'Hello, Elixir, from Fabrik'
  End

  It 'creates a Go project that builds and runs'
    go_missing() { ! command -v go >/dev/null 2>&1; }
    Skip if 'go is not installed' go_missing
    go_missing && return 0

    go_project
    fabrik build go-app/app >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/go-app/app"
    The status should be success
    The stdout should equal 'Hello from Fabrik'
  End

  It 'creates a macOS Apple project with valid targets'
    apple_macos_project
    When call fabrik targets
    The status should be success
    The stdout should include 'macos-app/Greeter'
    The stdout should include 'macos-app/hello'
  End

  It 'creates an iOS simulator Apple project with valid targets'
    apple_ios_project
    When call fabrik targets
    The status should be success
    The stdout should include 'apple_simulator_app ios-app/Demo'
  End
End
