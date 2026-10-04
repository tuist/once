#shellcheck shell=bash

verify_documented_greeting() (
  cd "$WORKSPACE" || return
  mkdir -p scripts
  awk '
    /^#!\/usr\/bin\/env -S once exec -- bash$/ { copying = 1 }
    copying && /^```/ { exit }
    copying { print }
  ' "$REPO_ROOT/web/priv/docs/guide/getting-started.md" > scripts/greet.sh
  [ -s scripts/greet.sh ] || return 1
  chmod +x scripts/greet.sh
  PATH="$(dirname "$ONCE_BIN"):$PATH"
  export PATH

  printf 'hello from Once\n' > message.txt
  ./scripts/greet.sh > first.txt 2>&1 || return
  grep -q 'cache miss' first.txt || return
  ./scripts/greet.sh > second.txt 2>&1 || return
  grep -q 'cache hit' second.txt || return

  rm build/greeting.txt
  ./scripts/greet.sh > restored.txt 2>&1 || return
  grep -q 'cache hit' restored.txt || return
  [ "$(<build/greeting.txt)" = 'hello from Once' ] || return 1

  printf 'hello again\n' > message.txt
  ./scripts/greet.sh > changed.txt 2>&1 || return
  grep -q 'cache miss' changed.txt || return
  [ "$(<build/greeting.txt)" = 'hello again' ] || return 1
  printf 'documented greeting reuses, restores, and invalidates correctly\n'
)

Describe 'getting started documentation'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  It 'runs the published script through its Once shebang'
    When call verify_documented_greeting
    The status should be success
    The stdout should equal 'documented greeting reuses, restores, and invalidates correctly'
  End
End
