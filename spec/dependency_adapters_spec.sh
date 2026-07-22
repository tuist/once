#shellcheck shell=bash
# End-to-end specs for locked dependency adapters and local consumers.

Describe 'locked dependency adapters'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  copy_dependency_example() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/$1/." "$WORKSPACE/"
    rm -rf "$WORKSPACE/.once"
  }

  rust_adapter_tools_unavailable() {
    ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1
  }

  enable_cargo_snapshot() {
    host_triple=$(rustc --version --verbose | sed -n 's/^host: //p') || return
    [ -n "$host_triple" ] || return 1
    sed -i.bak \
      "s/\"host_triple\": \"[^\"]*\"/\"host_triple\": \"$host_triple\"/" \
      "$WORKSPACE/cargo-metadata.json" || return
    sed -i.bak '/lockfile = "Cargo.lock"/a\
metadata_file = "cargo-metadata.json"' "$WORKSPACE/once.toml"
  }

  elixir_adapter_tools_unavailable() {
    ! command -v mix >/dev/null 2>&1 || ! command -v elixir >/dev/null 2>&1
  }

  zig_adapter_tools_unavailable() {
    ! command -v zig >/dev/null 2>&1
  }

  swift_adapter_tools_unavailable() {
    [ "$(uname -s)" != "Darwin" ] || ! command -v swift >/dev/null 2>&1 || ! command -v xcrun >/dev/null 2>&1
  }

  resolve_build_and_run_rust_consumer() {
    enable_cargo_snapshot || return
    once --format json query targets > "$WORKSPACE/targets.json" || return
    once --format json build apps/hello/hello > "$WORKSPACE/build.json" || return
    "$WORKSPACE/.once/out/apps/hello/hello/hello"
  }

  prepare_counted_live_cargo() {
    real_cargo=$(command -v cargo) || return
    mkdir -p "$WORKSPACE/tools"
    printf '#!/bin/sh\nprintf "metadata\\n" >> "%s"\nexec "%s" "$@"\n' \
      "$WORKSPACE/cargo-invocations.log" \
      "$real_cargo" \
      > "$WORKSPACE/tools/cargo"
    chmod +x "$WORKSPACE/tools/cargo"
    mv "$WORKSPACE/cargo-metadata.json" "$WORKSPACE/cargo-metadata.snapshot.json"
    sed -i.bak '/cargo-metadata.json/d' "$WORKSPACE/once.toml"
  }

  run_rust_consumer_with_counted_live_resolution() {
    prepare_counted_live_cargo || return

    PATH="$WORKSPACE/tools:$PATH" once --format json run apps/hello/hello \
      > "$WORKSPACE/run.json" || return
    [ "$(wc -l < "$WORKSPACE/cargo-invocations.log" | tr -d ' ')" -eq 1 ]
  }

  validate_with_counted_live_cargo_resolution() {
    prepare_counted_live_cargo || return

    PATH="$WORKSPACE/tools:$PATH" once --format json query validate-workspace \
      > "$WORKSPACE/validation.json" || return
    [ "$(wc -l < "$WORKSPACE/cargo-invocations.log" | tr -d ' ')" -eq 1 ]
  }

  resolve_build_and_run_elixir_consumer() {
    once --format json query targets > "$WORKSPACE/targets.json" || return
    once --format json build greeting > "$WORKSPACE/build.json" || return
    elixir \
      -pa "$WORKSPACE/.once/out/mix-locked-greeting/mix/prod/lib/locked_greeting/ebin" \
      -pa "$WORKSPACE/.once/out/local_helper/ebin" \
      -pa "$WORKSPACE/.once/out/greeting/ebin" \
      -e 'IO.write(Greeting.message("Once"))'
  }

  resolve_build_and_run_zig_consumer() {
    once --format json query targets > "$WORKSPACE/targets.json" || return
    once --format json build hello > "$WORKSPACE/build.json" || return
    "$WORKSPACE/.once/out/hello/hello"
  }

  resolve_build_and_run_swift_consumer() {
    once --format json query targets > "$WORKSPACE/targets.json" || return
    once --format json build Consumer > "$WORKSPACE/build.json" || return
    "$WORKSPACE/.once/out/Consumer/Consumer.app/Consumer"
  }

  replace_swift_vendor_scratch() {
    mkdir -p "$WORKSPACE/Vendor/Greeting/VendorScratch"
    printf 'old\n' > "$WORKSPACE/Vendor/Greeting/VendorScratch/old.swift"
    sed -i.bak '/graph_file =/a\
vendor_path = "Vendor/Greeting/VendorScratch"' "$WORKSPACE/once.toml"
    once --format json build Consumer > "$WORKSPACE/first-build.json" || return
    rm "$WORKSPACE/Vendor/Greeting/VendorScratch/old.swift"
    printf 'new\n' > "$WORKSPACE/Vendor/Greeting/VendorScratch/new.swift"
    once --format json build Consumer > "$WORKSPACE/second-build.json" || return
  }

  It 'loads Cargo packages and links them into a local Rust binary'
    Skip if 'Rust toolchain unavailable on this host' rust_adapter_tools_unavailable
    copy_dependency_example rust-binary-with-crate

    When call resolve_build_and_run_rust_consumer
    The status should be success
    The stdout should equal '42'
    The contents of file "$WORKSPACE/targets.json" should include '"kind":"rust_crate"'
    The contents of file "$WORKSPACE/targets.json" should include '"id":"itoa-1.0.14"'
    The contents of file "$WORKSPACE/build.json" should include '"target":"apps/hello/hello"'
  End

  It 'loads Cargo packages through the live locked metadata path'
    Skip if 'Rust toolchain unavailable on this host' rust_adapter_tools_unavailable
    copy_dependency_example rust-binary-with-crate
    mv "$WORKSPACE/cargo-metadata.json" "$WORKSPACE/cargo-metadata.snapshot.json"
    sed -i.bak '/metadata_file =/d' "$WORKSPACE/once.toml"

    When call once --format json query targets
    The status should be success
    The stdout should include '"kind":"rust_crate"'
    The stdout should include '"id":"itoa-1.0.14"'
  End

  It 'loads the live Cargo graph once when running a local Rust binary'
    Skip if 'Rust toolchain unavailable on this host' rust_adapter_tools_unavailable
    copy_dependency_example rust-binary-with-crate

    When call run_rust_consumer_with_counted_live_resolution
    The status should be success
    The contents of file "$WORKSPACE/cargo-invocations.log" should equal 'metadata'
    The contents of file "$WORKSPACE/run.json" should include '"target":"apps/hello/hello"'
  End

  It 'loads the live Cargo graph once when validating the workspace'
    Skip if 'Rust toolchain unavailable on this host' rust_adapter_tools_unavailable
    copy_dependency_example rust-binary-with-crate

    When call validate_with_counted_live_cargo_resolution
    The status should be success
    The contents of file "$WORKSPACE/cargo-invocations.log" should equal 'metadata'
    The contents of file "$WORKSPACE/validation.json" should include '"valid":true'
  End

  It 'loads Mix packages and links them into a local Elixir library'
    Skip if 'Elixir toolchain unavailable on this host' elixir_adapter_tools_unavailable
    copy_dependency_example elixir-library-with-mix-dependency

    When call resolve_build_and_run_elixir_consumer
    The status should be success
    The stdout should equal 'Hello, Once!'
    The contents of file "$WORKSPACE/targets.json" should include '"kind":"mix_package"'
    The contents of file "$WORKSPACE/targets.json" should include '"id":"mix-locked-greeting"'
    The contents of file "$WORKSPACE/build.json" should include '"target":"greeting"'
  End

  It 'loads Mix packages through the live locked project path'
    Skip if 'Elixir toolchain unavailable on this host' elixir_adapter_tools_unavailable
    copy_dependency_example elixir-library-with-mix-dependency
    mv "$WORKSPACE/mix-dependencies.json" "$WORKSPACE/mix-dependencies.snapshot.json"
    sed -i.bak '/graph_file =/d' "$WORKSPACE/once.toml"

    When call once --format json query targets
    The status should be success
    The stdout should include '"kind":"mix_package"'
    The stdout should include '"id":"mix-locked-greeting"'
  End

  It 'loads Zig packages and links them into a local Zig binary'
    Skip if 'Zig toolchain unavailable on this host' zig_adapter_tools_unavailable
    copy_dependency_example zig-binary-with-package

    When call resolve_build_and_run_zig_consumer
    The status should be success
    The stderr should equal '42'
    The contents of file "$WORKSPACE/targets.json" should include '"kind":"zig_package"'
    The contents of file "$WORKSPACE/targets.json" should include 'math_x45_1_x46_0_x46_0'
    The contents of file "$WORKSPACE/build.json" should include '"target":"hello"'
  End

  It 'loads Swift packages and links them into a local macOS application'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_adapter_tools_unavailable
    copy_dependency_example swift-package-dependencies-minimal

    When call resolve_build_and_run_swift_consumer
    The status should be success
    The stdout should equal 'Hello from a locked local package'
    The contents of file "$WORKSPACE/targets.json" should include '"kind":"swift_package_pin"'
    The contents of file "$WORKSPACE/targets.json" should include '"id":"swiftpm-greeting-local"'
    The contents of file "$WORKSPACE/build.json" should include '"target":"Consumer"'
  End

  It 'loads Swift packages through the live locked package path'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_adapter_tools_unavailable
    copy_dependency_example swift-package-dependencies-minimal
    mv "$WORKSPACE/swiftpm-dependencies.json" "$WORKSPACE/swiftpm-dependencies.snapshot.json"
    sed -i.bak '/graph_file =/d' "$WORKSPACE/once.toml"

    When call once --format json query targets
    The status should be success
    The stdout should include '"kind":"swift_package_pin"'
    The stdout should include '"id":"swiftpm-greeting-local"'
  End

  It 'replaces vendored Swift scratch state between builds'
    Skip if 'Apple Swift toolchain unavailable on this host' swift_adapter_tools_unavailable
    copy_dependency_example swift-package-dependencies-minimal

    When call replace_swift_vendor_scratch
    The status should be success
    The path "$WORKSPACE/.once/out/Packages/swiftpm/old.swift" should not be exist
    The path "$WORKSPACE/.once/out/Packages/swiftpm/new.swift" should be file
  End
End
