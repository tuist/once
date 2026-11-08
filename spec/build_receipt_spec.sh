#shellcheck shell=bash

Describe 'build receipts with uninspectable host directories'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  exercise_failed_tree_fingerprint() {
    observed="$SPEC_ROOT/external"
    nested="$observed/deep"
    depth=0
    while [ "$depth" -lt 66 ]; do
      nested="$nested/d"
      depth=$((depth + 1))
    done
    mkdir -p "$nested" "$WORKSPACE/modules"
    cat > "$WORKSPACE/once.toml" <<TOML
[modules]
paths = ["modules/*.star"]
[[target]]
name = "entries"
kind = "host_entries"
[target.attrs]
directory = "$observed"
TOML
    cat > "$WORKSPACE/modules/entries.star" <<'STAR'
def _entries(ctx):
    out = declare_output("entries.txt")
    write_path(out, ",".join(host_read_dir(ctx["attr"]["directory"])))
    return {"out": out}

host_entries = target_kind(
    attrs = [attr("directory", "string", required = True)],
    capabilities = [capability("build", ["default"])],
    impl = _entries,
)
STAR
    for attempt in 1 2 3; do
      once build entries --quiet >/dev/null || return
      cat "$WORKSPACE/.once/out/entries/entries.txt"
      printf '\n'
    done
    mkdir "$observed/new"
    for attempt in 1 2; do
      once build entries --quiet >/dev/null || return
      cat "$WORKSPACE/.once/out/entries/entries.txt"
      printf '\n'
    done
    mkdir "$observed/last"
    once build entries --quiet >/dev/null || return
    cat "$WORKSPACE/.once/out/entries/entries.txt"
  }

  It 'revalidates observable entries when recursive traversal keeps failing'
    When call exercise_failed_tree_fingerprint
    The status should be success
    The output should equal 'deep
deep
deep
deep,new
deep,new
deep,last,new'
  End
End
