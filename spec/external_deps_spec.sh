#shellcheck shell=bash
# End-to-end specs for target-side external dependency edges.

fabrik_with_elixir() {
  PATH="$WORKSPACE/bin:$PATH" "$FABRIK_BIN" -C "$WORKSPACE" "$@"
}

Describe 'external dependency consumption'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'lets a Rust binary import and consume a cargo graph dependency'
    mkdir -p "$WORKSPACE/app/src" "$WORKSPACE/__fabrik__/external/cargo/dep_message/src"
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "cargo"
ecosystem = "rust"
manifest = "Cargo.toml"
lockfile = "Cargo.lock"
EOF
    cat > "$WORKSPACE/app/fabrik.toml" <<'EOF'
[[rust.binary]]
name = "app"
srcs = ["src/main.rs"]
deps = [{ cargo = "dep_message" }]
EOF
    cat > "$WORKSPACE/app/src/main.rs" <<'EOF'
fn main() {
    println!("{}", dep_message::message());
}
EOF
    cat > "$WORKSPACE/__fabrik__/external/cargo/fabrik.toml" <<'EOF'
[[rust.library]]
name = "dep_message"
srcs = ["dep_message/src/lib.rs"]
EOF
    cat > "$WORKSPACE/__fabrik__/external/cargo/dep_message/src/lib.rs" <<'EOF'
pub fn message() -> &'static str {
    "rust dependency consumed"
}
EOF

    fabrik build app/app >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/app/app"
    The status should be success
    The path "$WORKSPACE/.fabrik/out/app/app" should be file
    The stdout should equal 'rust dependency consumed'
  End

  It 'lets an Elixir binary import and consume a mix graph dependency'
    elixir_missing() { ! command -v elixir >/dev/null 2>&1 || ! command -v elixirc >/dev/null 2>&1; }
    Skip if 'elixir is not installed' elixir_missing
    elixir_missing && return 0

    mkdir -p "$WORKSPACE/app/lib" "$WORKSPACE/__fabrik__/external/mix/external_dep/lib"
    stage_elixir_workspace_bin
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "mix"
ecosystem = "elixir"
manifest = "mix.exs"
lockfile = "mix.lock"
EOF
    cat > "$WORKSPACE/app/fabrik.toml" <<'EOF'
[[elixir.binary]]
name = "app"
entry = "App"
srcs = ["lib/app.ex"]
deps = [{ mix = "external_dep" }]
EOF
    cat > "$WORKSPACE/app/lib/app.ex" <<'EOF'
defmodule App do
  require ExternalDep

  def main(_args), do: IO.puts(ExternalDep.message())
end
EOF
    cat > "$WORKSPACE/__fabrik__/external/mix/fabrik.toml" <<'EOF'
[[elixir.library]]
name = "external_dep"
srcs = ["external_dep/lib/external_dep.ex"]
EOF
    cat > "$WORKSPACE/__fabrik__/external/mix/external_dep/lib/external_dep.ex" <<'EOF'
defmodule ExternalDep do
  defmacro message do
    quote do
      "elixir dependency consumed"
    end
  end
end
EOF

    fabrik_with_elixir build app/app >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/app/app"
    The status should be success
    The path "$WORKSPACE/.fabrik/out/app/app" should be file
    The stdout should equal 'elixir dependency consumed'
  End

  It 'lets a Swift command line app import and consume a SwiftPM graph dependency'
    Skip if 'apple toolchain is not available' apple_toolchain_unavailable
    apple_toolchain_unavailable && return 0

    mkdir -p "$WORKSPACE/app" "$WORKSPACE/__fabrik__/external/swiftpm"
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "swiftpm"
ecosystem = "swift"
manifest = "Package.swift"
lockfile = "Package.resolved"
EOF
    cat > "$WORKSPACE/app/fabrik.toml" <<'EOF'
[[apple.macos_command_line_application]]
name = "app"
module_name = "App"
minimum_os = "13.0"
srcs = ["main.swift"]
deps = [{ swiftpm = { product = "ExternalKit", package = "external-kit" } }]
EOF
    cat > "$WORKSPACE/app/main.swift" <<'EOF'
import ExternalKit

print(externalMessage())
EOF
    cat > "$WORKSPACE/__fabrik__/external/swiftpm/fabrik.toml" <<'EOF'
[[apple.swift_library]]
name = "ExternalKit"
module_name = "ExternalKit"
minimum_os = "13.0"
srcs = ["ExternalKit.swift"]
EOF
    cat > "$WORKSPACE/__fabrik__/external/swiftpm/ExternalKit.swift" <<'EOF'
public func externalMessage() -> String {
    "swift dependency consumed"
}
EOF

    fabrik build app/app >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/app/app"
    The status should be success
    The path "$WORKSPACE/.fabrik/out/app/app" should be file
    The stdout should equal 'swift dependency consumed'
  End

  It 'lets a Go binary import and consume a go module dependency'
    go_missing() { ! command -v go >/dev/null 2>&1; }
    Skip if 'go is not installed' go_missing
    go_missing && return 0

    mkdir -p "$WORKSPACE/app" "$WORKSPACE/lib"
    cat > "$WORKSPACE/fabrik.toml" <<'EOF'
[[deps]]
name = "go"
ecosystem = "go"
manifest = "app/go.mod"
EOF
    cat > "$WORKSPACE/app/fabrik.toml" <<'EOF'
[[go.binary]]
name = "app"
srcs = ["go.mod", "main.go"]
deps = [{ go = "example.com/lib" }]
EOF
    cat > "$WORKSPACE/app/go.mod" <<'EOF'
module example.com/app

go 1.22

require example.com/lib v0.0.0

replace example.com/lib => ../lib
EOF
    cat > "$WORKSPACE/app/main.go" <<'EOF'
package main

import (
	"fmt"

	"example.com/lib"
)

func main() {
	fmt.Println(lib.Message())
}
EOF
    cat > "$WORKSPACE/lib/go.mod" <<'EOF'
module example.com/lib

go 1.22
EOF
    cat > "$WORKSPACE/lib/message.go" <<'EOF'
package lib

func Message() string {
	return "go dependency consumed"
}
EOF

    fabrik build app/app >/dev/null 2>&1
    When call "$WORKSPACE/.fabrik/out/app/app"
    The status should be success
    The path "$WORKSPACE/.fabrik/out/app/app" should be file
    The stdout should equal 'go dependency consumed'
  End
End
