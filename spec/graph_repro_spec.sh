#shellcheck shell=bash

Describe 'native graph reproductions'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  # A root manifest configures the module, the native project lives in `proj`,
  # and a vendored part sits in a directory discovery excludes but whose
  # manifest the resolver still reads.
  prepare_native_workspace() {
    cat > "$WORKSPACE/struo.toml" <<'TOML'
[modules]
paths = ["native.star"]

[[target]]
name = "other"
kind = "native_part"
TOML
    cat > "$WORKSPACE/native.star" <<'STAR'
def _resolve(ctx):
    names = []
    for path in sorted(ctx["files"].keys()):
        names += [line for line in ctx["files"][path].split("\n") if line]
    if "broken" in names:
        fail("the native resolver cannot read this project")
    if "unfetched" in names:
        _diagnostic_fail("native_sources_missing", "srcs", "the native sources are not fetched", "Fetch the sources, then retry")
    return {"targets": [{"name": name, "kind": "native_part", "srcs": ["src/" + name + ".txt"]} for name in names]}

native_part = target_kind()
native_workspace = target_kind(attrs = [attr("resolver_inputs", "list<string>", configurable = False)], resolver = _resolve)
native = native_project(target_kind = "native_workspace", target_name = "native", markers = ["project.native"], inputs = ["vendor/*/project.native"], exclude = ["vendor"], manifests = ["**/project.native"])
STAR
    mkdir -p "$WORKSPACE/proj/src" "$WORKSPACE/proj/vendor/shim"
    printf 'core\napp\n' > "$WORKSPACE/proj/project.native"
    printf 'shim\n' > "$WORKSPACE/proj/vendor/shim/project.native"
    printf 'proprietary core\n' > "$WORKSPACE/proj/src/core.txt"
    printf 'proprietary app\n' > "$WORKSPACE/proj/src/app.txt"
  }

  It 'exports manifests and empty placeholders that load to the same graph'
    prepare_native_workspace
    export_and_compare() {
      struo query targets --format json | jq -c 'map(.id) | sort' > "$SPEC_ROOT/original.json" || return
      struo query graph-repro --format json > "$SPEC_ROOT/export.json" || return
      repro="$WORKSPACE/.struo/graph-repro"
      (cd "$repro/workspace" && "$STRUO_BIN" query targets --format json) | jq -c 'map(.id) | sort' > "$SPEC_ROOT/replayed.json" || return
      cmp "$SPEC_ROOT/original.json" "$SPEC_ROOT/replayed.json" || return
      ! grep -r proprietary "$repro" || return
      [ ! -s "$repro/workspace/proj/src/core.txt" ] || return
      ! tar -xzOf "$repro/graph-repro.tar.gz" | grep proprietary || return
      jq -c '.' "$SPEC_ROOT/replayed.json"
      jq -c '[.copied, .issue.repository]' "$SPEC_ROOT/export.json"
    }
    When call export_and_compare
    The status should be success
    The line 1 of stdout should equal '["other","proj/app","proj/core","proj/native","proj/shim"]'
    The line 2 of stdout should equal '[["native.star","proj/project.native","proj/vendor/shim/project.native","struo.toml"],"tuist/struo"]'
  End

  It 'drafts an issue that carries the loading failure'
    prepare_native_workspace
    printf 'broken\n' > "$WORKSPACE/proj/project.native"
    draft_issue() {
      struo query graph-repro --format json > "$SPEC_ROOT/export.json" || return
      jq -r '.issue.title, .issue.url' "$SPEC_ROOT/export.json"
      cat "$WORKSPACE/.struo/graph-repro/ISSUE.md"
    }
    When call draft_issue
    The status should be success
    The line 1 of stdout should equal 'Native graph fails to load: native'
    The line 2 of stdout should start with 'https://github.com/tuist/struo/issues/new?title='
    The stdout should include 'the native resolver cannot read this project'
    The stdout should not include "$WORKSPACE"
  End

  It 'points a failed build of a target the resolver owns at the reproduction export'
    prepare_native_workspace
    printf 'broken\n' > "$WORKSPACE/proj/project.native"
    When call struo build proj/app --format json
    The status should be failure
    The stderr should include 'the native resolver cannot read this project'
    The stderr should include 'struo query graph-repro'
  End

  It 'reports a mistyped target outside a failed resolver as not found'
    prepare_native_workspace
    printf 'unfetched\n' > "$WORKSPACE/proj/project.native"
    When call struo build othr
    The status should be failure
    The stderr should include 'no target matches `othr`'
    The stderr should not include 'the native sources are not fetched'
  End

  It 'reports the failed resolver for a missing target it owns, without blaming Struo'
    prepare_native_workspace
    printf 'unfetched\n' > "$WORKSPACE/proj/project.native"
    When call struo build proj/app
    The status should be failure
    The stderr should include 'the native sources are not fetched'
    The stderr should include 'Fetch the sources, then retry'
    The stderr should not include 'struo query graph-repro'
  End

  It 'reports the failed resolver when testing everything finds no tests'
    prepare_native_workspace
    printf 'unfetched\n' > "$WORKSPACE/proj/project.native"
    When call struo test
    The status should be failure
    The stderr should include 'the native sources are not fetched'
  End

  It 'never empties an output directory it did not write'
    prepare_native_workspace
    mkdir -p "$SPEC_ROOT/out"
    printf '{}' > "$SPEC_ROOT/out/repro.json"
    printf 'keep' > "$SPEC_ROOT/out/notes.txt"
    export_into_foreign_directory() {
      struo query graph-repro --output "$SPEC_ROOT/out" && return 1
      cat "$SPEC_ROOT/out/notes.txt"
    }
    When call export_into_foreign_directory
    The stdout should equal 'keep'
    The stderr should include 'not a graph reproduction'
  End

  It 'records a broken module and recovers once it is fixed'
    prepare_native_workspace
    printf '[modules]\npaths = ["missing.star"]\n' > "$WORKSPACE/struo.toml"
    export_twice() {
      struo query graph-repro --format json | jq -r '.load_error != null' || return
      printf '[modules]\npaths = ["native.star"]\n' > "$WORKSPACE/struo.toml"
      struo query graph-repro --format json | jq -r '.target_count'
    }
    When call export_twice
    The status should be success
    The line 1 of stdout should equal 'true'
    The line 2 of stdout should equal '4'
  End
End
