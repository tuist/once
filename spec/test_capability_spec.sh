#shellcheck shell=bash
# End-to-end specs for generic test capability targets.

Describe 'test capability'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  create_shellspec_test_workspace() {
    shellspec_bin="shellspec"
    clean_path=""
    old_ifs="$IFS"
    IFS=:
    for path_entry in $PATH; do
      case "$path_entry" in
        /var/folders/*/T/shellspec.*|*/spec/support/bin)
          continue
          ;;
        */mise/installs/shellspec/*)
          shellspec_bin="$path_entry/shellspec"
          ;;
      esac
      if [ -z "$clean_path" ]; then
        clean_path="$path_entry"
      else
        clean_path="$clean_path:$path_entry"
      fi
    done
    IFS="$old_ifs"
    cp -R "$REPO_ROOT/fixtures/shellspec_test/." "$WORKSPACE/"
    sed -e "s|__SHELLSPEC_BIN__|$shellspec_bin|g" \
      -e "s|__CLEAN_PATH__|$clean_path|g" \
      -e "s|__ONCE_BIN__|$ONCE_BIN|g" \
      "$REPO_ROOT/fixtures/shellspec_test/once.toml" > "$WORKSPACE/once.toml"
  }

  create_rust_test_workspace() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/rust-test-minimal/." "$WORKSPACE/"
  }

  java_toolchain_unavailable() {
    command -v java >/dev/null 2>&1 || return 0
    command -v javac >/dev/null 2>&1 || return 0
    command -v jar >/dev/null 2>&1 || return 0
    return 1
  }

  create_android_local_test_workspace() {
    javac_bin="$(command -v javac)"
    jar_bin="$(command -v jar)"
    mkdir -p \
      "$WORKSPACE/android-sdk/build-tools/35.0.0" \
      "$WORKSPACE/android-sdk/platforms/android-35" \
      "$WORKSPACE/bin" \
      "$WORKSPACE/empty" \
      "$WORKSPACE/libs/greeting/src/main/kotlin/dev/once/greeting" \
      "$WORKSPACE/libs/greeting/src/test/kotlin/dev/once/greeting"
    "$jar_bin" cf "$WORKSPACE/android-sdk/platforms/android-35/android.jar" -C "$WORKSPACE/empty" .
    "$jar_bin" cf "$WORKSPACE/kotlin-stdlib.jar" -C "$WORKSPACE/empty" .
    cat > "$WORKSPACE/android-sdk/build-tools/35.0.0/aapt2" <<'SH'
#!/bin/sh
set -eu
case "${1:-}" in
  version)
    printf '%s\n' 'aapt2 mock'
    ;;
  link)
    out=''
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "-o" ]; then
        shift
        out="${1:-}"
      fi
      shift || break
    done
    [ -n "$out" ] || exit 2
    mkdir -p "$(dirname "$out")"
    : > "$out"
    ;;
  compile)
    out=''
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "-o" ]; then
        shift
        out="${1:-}"
      fi
      shift || break
    done
    [ -n "$out" ] || exit 2
    mkdir -p "$(dirname "$out")"
    : > "$out"
    ;;
  *)
    printf '%s\n' "unexpected aapt2 invocation: $*" >&2
    exit 1
    ;;
esac
SH
    chmod +x "$WORKSPACE/android-sdk/build-tools/35.0.0/aapt2"
    cat > "$WORKSPACE/bin/kotlinc" <<SH
#!/bin/sh
set -eu
case "\${1:-}" in
  -version)
    printf '%s\n' 'kotlinc mock'
    exit 0
    ;;
esac
out=''
classpath=''
source_list=''
while [ "\$#" -gt 0 ]; do
  case "\$1" in
    -d)
      shift
      out="\${1:-}"
      ;;
    -classpath)
      shift
      classpath="\${1:-}"
      ;;
    @*)
      source_list="\${1#@}"
      ;;
  esac
  shift || break
done
[ -n "\$out" ] || exit 2
[ -n "\$source_list" ] || exit 2
tmp="\$(mktemp -d)"
mkdir -p "\$tmp/dev/once/greeting"
if grep -q 'src/main' "\$source_list"; then
  cat > "\$tmp/dev/once/greeting/Greeting.java" <<'JAVA'
package dev.once.greeting;

public final class Greeting {
  private Greeting() {}

  public static String message() {
    return "hello";
  }
}
JAVA
  "$javac_bin" -classpath "\$classpath" -d "\$out" "\$tmp/dev/once/greeting/Greeting.java"
else
  cat > "\$tmp/dev/once/greeting/GreetingTest.java" <<'JAVA'
package dev.once.greeting;

public final class GreetingTest {
  public void testMessage() {
    if (!Greeting.message().equals("hello")) {
      throw new AssertionError("unexpected greeting");
    }
  }
}
JAVA
  "$javac_bin" -classpath "\$classpath" -d "\$out" "\$tmp/dev/once/greeting/GreetingTest.java"
fi
SH
    chmod +x "$WORKSPACE/bin/kotlinc"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[workspace]
include = ["libs/*/once.toml"]
TOML
    cat > "$WORKSPACE/libs/greeting/AndroidManifest.xml" <<'XML'
<manifest xmlns:android="http://schemas.android.com/apk/res/android" />
XML
    cat > "$WORKSPACE/libs/greeting/once.toml" <<TOML
[[target]]
name = "Greeting"
kind = "android_library"
srcs = ["src/main/**/*.kt"]

[target.attrs]
namespace = "dev.once.greeting"
manifest = "AndroidManifest.xml"
resource_files = []
android_sdk = "$WORKSPACE/android-sdk"
compile_sdk = 35
build_tools_version = "35.0.0"
kotlinc = "$WORKSPACE/bin/kotlinc"
kotlin_stdlib = "$WORKSPACE/kotlin-stdlib.jar"

[[target]]
name = "GreetingTests"
kind = "android_local_test"
srcs = ["src/test/**/*.kt"]
deps = ["./Greeting"]

[target.attrs]
android_sdk = "$WORKSPACE/android-sdk"
compile_sdk = 35
build_tools_version = "35.0.0"
kotlinc = "$WORKSPACE/bin/kotlinc"
kotlin_stdlib = "$WORKSPACE/kotlin-stdlib.jar"
labels = ["kotlin", "unit"]
TOML
    cat > "$WORKSPACE/libs/greeting/src/main/kotlin/dev/once/greeting/Greeting.kt" <<'KOTLIN'
package dev.once.greeting

object Greeting {
    fun message(): String = "hello"
}
KOTLIN
    cat > "$WORKSPACE/libs/greeting/src/test/kotlin/dev/once/greeting/GreetingTest.kt" <<'KOTLIN'
package dev.once.greeting

class GreetingTest {
    fun testMessage() {
        check(Greeting.message() == "hello")
    }
}
KOTLIN
  }

  create_android_instrumentation_test_workspace() {
    javac_bin="$(command -v javac)"
    jar_bin="$(command -v jar)"
    mkdir -p \
      "$WORKSPACE/android-sdk/build-tools/35.0.0" \
      "$WORKSPACE/android-sdk/platforms/android-35" \
      "$WORKSPACE/android-sdk/platform-tools" \
      "$WORKSPACE/bin" \
      "$WORKSPACE/empty" \
      "$WORKSPACE/libs/greeting/src/main/kotlin/dev/once/greeting" \
      "$WORKSPACE/libs/greeting/src/androidTest/kotlin/dev/once/greeting"
    "$jar_bin" cf "$WORKSPACE/android-sdk/platforms/android-35/android.jar" -C "$WORKSPACE/empty" .
    "$jar_bin" cf "$WORKSPACE/kotlin-stdlib.jar" -C "$WORKSPACE/empty" .
    cat > "$WORKSPACE/android-sdk/build-tools/35.0.0/aapt2" <<SH
#!/bin/sh
set -eu
case "\${1:-}" in
  version)
    printf '%s\n' 'aapt2 mock'
    ;;
  link)
    out=''
    while [ "\$#" -gt 0 ]; do
      if [ "\$1" = "-o" ]; then
        shift
        out="\${1:-}"
      fi
      shift || break
    done
    [ -n "\$out" ] || exit 2
    mkdir -p "\$(dirname "\$out")"
    "$jar_bin" cf "\$out" -C "$WORKSPACE/empty" .
    ;;
  compile)
    out=''
    while [ "\$#" -gt 0 ]; do
      if [ "\$1" = "-o" ]; then
        shift
        out="\${1:-}"
      fi
      shift || break
    done
    [ -n "\$out" ] || exit 2
    mkdir -p "\$(dirname "\$out")"
    : > "\$out"
    ;;
  *)
    printf '%s\n' "unexpected aapt2 invocation: \$*" >&2
    exit 1
    ;;
esac
SH
    chmod +x "$WORKSPACE/android-sdk/build-tools/35.0.0/aapt2"
    cat > "$WORKSPACE/android-sdk/build-tools/35.0.0/d8" <<'SH'
#!/bin/sh
set -eu
case "${1:-}" in
  --version)
    printf '%s\n' 'd8 mock'
    exit 0
    ;;
esac
out=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    shift
    out="${1:-}"
  fi
  shift || break
done
[ -n "$out" ] || exit 2
mkdir -p "$out"
printf 'dex\n' > "$out/classes.dex"
SH
    chmod +x "$WORKSPACE/android-sdk/build-tools/35.0.0/d8"
    cat > "$WORKSPACE/android-sdk/build-tools/35.0.0/zipalign" <<'SH'
#!/bin/sh
set -eu
input="${3:-}"
output="${4:-}"
[ -n "$input" ] && [ -n "$output" ] || exit 2
cp "$input" "$output"
SH
    chmod +x "$WORKSPACE/android-sdk/build-tools/35.0.0/zipalign"
    cat > "$WORKSPACE/android-sdk/build-tools/35.0.0/apksigner" <<'SH'
#!/bin/sh
set -eu
printf '%s\n' 'unexpected apksigner invocation' >&2
exit 1
SH
    chmod +x "$WORKSPACE/android-sdk/build-tools/35.0.0/apksigner"
    cat > "$WORKSPACE/android-sdk/platform-tools/adb" <<'SH'
#!/bin/sh
set -eu
case "${1:-}" in
  version)
    printf '%s\n' 'Android Debug Bridge version mock'
    exit 0
    ;;
  -s)
    shift 2
    ;;
esac
case "${1:-}" in
  wait-for-device)
    exit 0
    ;;
  install)
    exit 0
    ;;
  shell)
    shift
    if [ "${1:-}" = "am" ] && [ "${2:-}" = "instrument" ]; then
      cat <<'OUT'
INSTRUMENTATION_STATUS: class=dev.once.greeting.GreetingInstrumentedTest
INSTRUMENTATION_STATUS: test=useAppContext
INSTRUMENTATION_STATUS_CODE: 1
INSTRUMENTATION_STATUS: class=dev.once.greeting.GreetingInstrumentedTest
INSTRUMENTATION_STATUS: test=useAppContext
INSTRUMENTATION_STATUS_CODE: 0
INSTRUMENTATION_RESULT: stream=

OK (1 test)
INSTRUMENTATION_CODE: -1
OUT
      exit 0
    fi
    exit 0
    ;;
  *)
    printf '%s\n' "unexpected adb invocation: $*" >&2
    exit 1
    ;;
esac
SH
    chmod +x "$WORKSPACE/android-sdk/platform-tools/adb"
    cat > "$WORKSPACE/bin/kotlinc" <<SH
#!/bin/sh
set -eu
case "\${1:-}" in
  -version)
    printf '%s\n' 'kotlinc mock'
    exit 0
    ;;
esac
out=''
classpath=''
source_list=''
while [ "\$#" -gt 0 ]; do
  case "\$1" in
    -d)
      shift
      out="\${1:-}"
      ;;
    -classpath)
      shift
      classpath="\${1:-}"
      ;;
    @*)
      source_list="\${1#@}"
      ;;
  esac
  shift || break
done
[ -n "\$out" ] || exit 2
[ -n "\$source_list" ] || exit 2
tmp="\$(mktemp -d)"
mkdir -p "\$tmp/dev/once/greeting"
if grep -q 'src/main' "\$source_list"; then
  cat > "\$tmp/dev/once/greeting/Greeting.java" <<'JAVA'
package dev.once.greeting;

public final class Greeting {
  private Greeting() {}

  public static String message() {
    return "hello";
  }
}
JAVA
  "$javac_bin" -classpath "\$classpath" -d "\$out" "\$tmp/dev/once/greeting/Greeting.java"
else
  cat > "\$tmp/dev/once/greeting/GreetingInstrumentedTest.java" <<'JAVA'
package dev.once.greeting;

public final class GreetingInstrumentedTest {
  public void useAppContext() {}
}
JAVA
  "$javac_bin" -classpath "\$classpath" -d "\$out" "\$tmp/dev/once/greeting/GreetingInstrumentedTest.java"
fi
SH
    chmod +x "$WORKSPACE/bin/kotlinc"
    cat > "$WORKSPACE/once.toml" <<'TOML'
[workspace]
include = ["libs/*/once.toml"]
TOML
    cat > "$WORKSPACE/libs/greeting/AndroidManifest.xml" <<'XML'
<manifest xmlns:android="http://schemas.android.com/apk/res/android" />
XML
    cat > "$WORKSPACE/libs/greeting/AndroidTestManifest.xml" <<'XML'
<manifest xmlns:android="http://schemas.android.com/apk/res/android">
  <instrumentation
    android:name="androidx.test.runner.AndroidJUnitRunner"
    android:targetPackage="dev.once.greeting" />
</manifest>
XML
    cat > "$WORKSPACE/libs/greeting/once.toml" <<TOML
[[target]]
name = "GreetingApp"
kind = "android_binary"
srcs = ["src/main/**/*.kt"]

[target.attrs]
application_id = "dev.once.greeting"
namespace = "dev.once.greeting"
manifest = "AndroidManifest.xml"
resource_files = []
android_sdk = "$WORKSPACE/android-sdk"
compile_sdk = 35
build_tools_version = "35.0.0"
kotlinc = "$WORKSPACE/bin/kotlinc"
kotlin_stdlib = "$WORKSPACE/kotlin-stdlib.jar"
signing = "none"

[[target]]
name = "GreetingInstrumentationApk"
kind = "android_binary"
srcs = ["src/androidTest/**/*.kt"]

[target.attrs]
application_id = "dev.once.greeting.test"
namespace = "dev.once.greeting.test"
manifest = "AndroidTestManifest.xml"
resource_files = []
android_sdk = "$WORKSPACE/android-sdk"
compile_sdk = 35
build_tools_version = "35.0.0"
kotlinc = "$WORKSPACE/bin/kotlinc"
kotlin_stdlib = "$WORKSPACE/kotlin-stdlib.jar"
signing = "none"
instruments = "./GreetingApp"

[[target]]
name = "GreetingDeviceTests"
kind = "android_instrumentation_test"
deps = ["./GreetingApp", "./GreetingInstrumentationApk"]

[target.attrs]
android_sdk = "$WORKSPACE/android-sdk"
test_app = "./GreetingInstrumentationApk"
instrumentation_runner = "androidx.test.runner.AndroidJUnitRunner"
labels = ["android", "device"]
TOML
    cat > "$WORKSPACE/libs/greeting/src/main/kotlin/dev/once/greeting/Greeting.kt" <<'KOTLIN'
package dev.once.greeting

object Greeting {
    fun message(): String = "hello"
}
KOTLIN
    cat > "$WORKSPACE/libs/greeting/src/androidTest/kotlin/dev/once/greeting/GreetingInstrumentedTest.kt" <<'KOTLIN'
package dev.once.greeting

class GreetingInstrumentedTest {
    fun useAppContext() {
        check(Greeting.message() == "hello")
    }
}
KOTLIN
  }

  It 'discovers shellspec test targets through the generic test query'
    create_shellspec_test_workspace

    When call once --format json query tests
    The status should be success
    The stdout should include '"id":"cli_e2e"'
    The stdout should include '"kind":"shellspec_test"'
    The stdout should include '"runner":"shellspec"'
    The stdout should include '"results_path":".once/out/cli_e2e/test/test_results.json"'
  End

  It 'selects affected shellspec tests from changed inputs'
    create_shellspec_test_workspace

    When call once --format json query affected-tests --changed-path spec/cli_surface_spec.sh
    The status should be success
    The stdout should include '"id":"cli_e2e"'
    The stdout should include 'changed test input `spec/cli_surface_spec.sh`'
  End

  It 'runs affected shellspec tests through the MCP tool'
    create_shellspec_test_workspace
    mcp_request='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"once_run_tests","arguments":{"changed_paths":["spec/cli_surface_spec.sh"]}}}'

    When call /bin/sh -c 'printf "%s\n" "$1" | "$2" -C "$3" mcp --allow-run' sh "$mcp_request" "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The stdout should include 'cli_e2e'
    The stdout should include 'once.test_results.v1'
    The stdout should include '\"success\": true'
  End

  It 'runs shellspec tests and exposes normalized test results'
    create_shellspec_test_workspace

    When call once --format json test cli_e2e
    The status should be success
    The stdout should include '"target":"cli_e2e"'
    The stdout should include '"capability":"test"'
    The path "$WORKSPACE/.once/out/cli_e2e/test/test_results.json" should be file
  End

  It 'runs Rust test targets end to end'
    create_rust_test_workspace

    When call once --format json test hello_tests
    The status should be success
    The stdout should include '"target":"hello_tests"'
    The stdout should include '"capability":"test"'
    The path "$WORKSPACE/.once/out/hello_tests/test/test_results.json" should be file
    The contents of file "$WORKSPACE/.once/out/hello_tests/test/test_results.json" should include '"runner":{"type":"rust_libtest"'
    The contents of file "$WORKSPACE/.once/out/hello_tests/test/test_results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/.once/out/hello_tests/test/test_results.json" should include 'returns_greeting'
  End

  It 'runs Kotlin local test targets end to end'
    Skip if 'Java toolchain unavailable on this host' java_toolchain_unavailable
    create_android_local_test_workspace

    When call once --format json test libs/greeting/GreetingTests
    The status should be success
    The stdout should include '"target":"libs/greeting/GreetingTests"'
    The stdout should include '"capability":"test"'
    The path "$WORKSPACE/.once/out/libs/greeting/GreetingTests/test/test_results.json" should be file
    The contents of file "$WORKSPACE/.once/out/libs/greeting/GreetingTests/test/test_results.json" should include '"runner":{"type":"android_local"'
    The contents of file "$WORKSPACE/.once/out/libs/greeting/GreetingTests/test/test_results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/.once/out/libs/greeting/GreetingTests/test/test_results.json" should include 'GreetingTest.testMessage'
  End

  It 'runs Android instrumentation test targets end to end'
    Skip if 'Java toolchain unavailable on this host' java_toolchain_unavailable
    create_android_instrumentation_test_workspace

    When call once --format json test libs/greeting/GreetingDeviceTests
    The status should be success
    The stdout should include '"target":"libs/greeting/GreetingDeviceTests"'
    The stdout should include '"capability":"test"'
    The path "$WORKSPACE/.once/out/libs/greeting/GreetingDeviceTests/test/test_results.json" should be file
    The contents of file "$WORKSPACE/.once/out/libs/greeting/GreetingDeviceTests/test/test_results.json" should include '"runner":{"type":"android_instrumentation"'
    The contents of file "$WORKSPACE/.once/out/libs/greeting/GreetingDeviceTests/test/test_results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/.once/out/libs/greeting/GreetingDeviceTests/test/test_results.json" should include 'GreetingInstrumentedTest.useAppContext'
  End

  It 'queries normalized shellspec test results'
    create_shellspec_test_workspace
    once --format json test cli_e2e >/dev/null

    When call once --format json query test-results cli_e2e
    The status should be success
    The stdout should include '"schema":"once.test_results.v1"'
    The stdout should include '"target":"cli_e2e"'
    The stdout should include '"status":"passed"'
    The stdout should include 'spec/cli_surface_spec.sh::lists commands'
    The stdout should include 'spec/graph_query_spec.sh::lists declared targets'
  End
End
