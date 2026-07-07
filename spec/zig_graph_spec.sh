#shellcheck shell=bash
# End-to-end specs for C and Zig graph target kinds.

Describe 'C and Zig graph target kinds'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  write_native_toolchain_mocks() {
    mkdir -p "$WORKSPACE/bin"

    cat > "$WORKSPACE/bin/cc" <<'SH'
#!/bin/sh
set -eu
if [ "${1:-}" = "--version" ]; then
  printf '%s\n' 'cc mock'
  exit 0
fi
printf 'cc %s\n' "$*" >> "$PWD/cc-argv.log"
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
printf '%s\n' 'object' > "$out"
SH
    chmod +x "$WORKSPACE/bin/cc"

    cat > "$WORKSPACE/bin/cxx" <<'SH'
#!/bin/sh
set -eu
if [ "${1:-}" = "--version" ]; then
  printf '%s\n' 'cxx mock'
  exit 0
fi
printf 'cxx %s\n' "$*" >> "$PWD/cc-argv.log"
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
printf '%s\n' 'object' > "$out"
SH
    chmod +x "$WORKSPACE/bin/cxx"

    cat > "$WORKSPACE/bin/ar" <<'SH'
#!/bin/sh
set -eu
archive="${2:-}"
[ -n "$archive" ] || exit 2
mkdir -p "$(dirname "$archive")"
printf '%s\n' 'archive' > "$archive"
SH
    chmod +x "$WORKSPACE/bin/ar"

    cat > "$WORKSPACE/bin/zig" <<'SH'
#!/bin/sh
set -eu
cmd="${1:-}"
shift || true
printf '%s %s\n' "$cmd" "$*" >> "$PWD/zig-argv.log"
case "$cmd" in
  version)
    printf '%s\n' '0.15.1'
    exit 0
    ;;
  translate-c)
    printf '%s\n' 'pub const translated_c_header = true;'
    exit 0
    ;;
esac

emit_bin=''
emit_docs=''
emit_asm=''
emit_ir=''
emit_bc=''
for arg in "$@"; do
  case "$arg" in
    -femit-bin=*) emit_bin="${arg#-femit-bin=}" ;;
    -femit-docs=*) emit_docs="${arg#-femit-docs=}" ;;
    -femit-asm=*) emit_asm="${arg#-femit-asm=}" ;;
    -femit-llvm-ir=*) emit_ir="${arg#-femit-llvm-ir=}" ;;
    -femit-llvm-bc=*) emit_bc="${arg#-femit-llvm-bc=}" ;;
  esac
done

for output in "$emit_asm" "$emit_ir" "$emit_bc"; do
  if [ -n "$output" ]; then
    mkdir -p "$(dirname "$output")"
    printf '%s\n' "auxiliary output for $cmd" > "$output"
  fi
done

if [ -n "$emit_docs" ]; then
  mkdir -p "$emit_docs"
  printf '%s\n' "docs for $cmd" > "$emit_docs/index.html"
fi

if [ -n "$emit_bin" ]; then
  mkdir -p "$(dirname "$emit_bin")"
  case "$cmd" in
    build-exe)
      cat > "$emit_bin" <<'BIN'
#!/bin/sh
printf 'zig executable mock:%s\n' "$*"
BIN
      chmod +x "$emit_bin"
      ;;
    test)
      cat > "$emit_bin" <<'BIN'
#!/bin/sh
printf 'zig test mock:%s env:%s test_env:%s\n' "$*" "${ONCE_ZIG_E2E_ENV:-}" "${ONCE_ZIG_TEST_ENV:-}"
if [ "${1:-}" = "fail-test" ]; then
  exit 1
fi
BIN
      chmod +x "$emit_bin"
      ;;
    build-lib)
      printf '%s\n' 'zig library mock' > "$emit_bin"
      ;;
    *)
      printf '%s\n' "unexpected zig command: $cmd" >&2
      exit 1
      ;;
  esac
fi
SH
    chmod +x "$WORKSPACE/bin/zig"

    cat > "$WORKSPACE/bin/translate-c" <<'SH'
#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$PWD/translate-c-argv.log"
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
printf '%s\n' 'pub const standalone_translated_c_header = true;' > "$out"
SH
    chmod +x "$WORKSPACE/bin/translate-c"
  }

  write_zig_workspace() {
    write_native_toolchain_mocks
    mkdir -p "$WORKSPACE/data" "$WORKSPACE/include" "$WORKSPACE/src"
    printf '%s\n' 'runtime data' > "$WORKSPACE/data/message.txt"
    cat > "$WORKSPACE/include/native.h" <<'H'
int native_answer(void);
H
    cat > "$WORKSPACE/src/native.c" <<'C'
int native_answer(void) { return 42; }
C
    cat > "$WORKSPACE/src/native.cc" <<'CXX'
int native_cxx_answer() { return 43; }
CXX
    cat > "$WORKSPACE/src/math.zig" <<'ZIG'
pub const answer = 42;
ZIG
    cat > "$WORKSPACE/src/dash.zig" <<'ZIG'
pub const value = 1;
ZIG
    cat > "$WORKSPACE/src/underscore.zig" <<'ZIG'
pub const value = 2;
ZIG
    cat > "$WORKSPACE/src/main.zig" <<'ZIG'
const math = @import("math");
const c = @import("c");

pub fn main() void {
    _ = math.answer;
    _ = c.translated_c_header;
}
ZIG
    cat > "$WORKSPACE/src/main_csrc.zig" <<'ZIG'
pub fn main() void {}
ZIG
    cat > "$WORKSPACE/src/main_collision.zig" <<'ZIG'
const dash = @import("dash");
const underscore = @import("underscore");

pub fn main() void {
    _ = dash.value;
    _ = underscore.value;
}
ZIG
    cat > "$WORKSPACE/src/test.zig" <<'ZIG'
test "answer" {
    try @import("std").testing.expectEqual(@as(i32, 42), 42);
}
ZIG
    cat > "$WORKSPACE/src/runner.zig" <<'ZIG'
pub fn main() void {}
ZIG
    cat > "$WORKSPACE/once.toml" <<TOML
[[target]]
name = "native"
kind = "c_library"
srcs = ["src/native.c"]

[target.attrs]
hdrs = ["include/native.h"]
includes = ["include"]
compiler = "$WORKSPACE/bin/cc"
cxx_compiler = "$WORKSPACE/bin/cxx"
archiver = "$WORKSPACE/bin/ar"
archiver_identity = "ar mock v1"

[[target]]
name = "native_cxx"
kind = "c_library"
srcs = ["src/native.cc"]

[target.attrs]
compiler = "$WORKSPACE/bin/cc"
cxx_compiler = "$WORKSPACE/bin/cxx"
archiver = "$WORKSPACE/bin/ar"
archiver_identity = "ar mock v1"

[[target]]
name = "math"
kind = "zig_library"
srcs = ["src/math.zig"]

[target.attrs]
main = "src/math.zig"
import_name = "math"

[[target]]
name = "foo-bar"
kind = "zig_library"
srcs = ["src/dash.zig"]

[target.attrs]
main = "src/dash.zig"
import_name = "dash"

[[target]]
name = "foo_bar"
kind = "zig_library"
srcs = ["src/underscore.zig"]

[target.attrs]
main = "src/underscore.zig"
import_name = "underscore"

[[target]]
name = "app"
kind = "zig_binary"
srcs = ["src/**/*.zig"]
deps = ["./math", "./native"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/main.zig"
args = ["run-arg"]
data = ["data/message.txt"]

[[target]]
name = "app_from_module"
kind = "zig_binary"
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
args = ["module-root-arg"]

[[target]]
name = "app_with_csrcs"
kind = "zig_binary"
srcs = ["src/main_csrc.zig"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/main_csrc.zig"
csrcs = ["src/native.c"]
copts = ["-DLOCAL_C_SOURCE"]

[[target]]
name = "app_collision"
kind = "zig_binary"
srcs = ["src/main_collision.zig"]
deps = ["./foo-bar", "./foo_bar"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/main_collision.zig"

[[target]]
name = "app_release"
kind = "zig_configure_binary"
srcs = ["src/**/*.zig"]
deps = ["./math", "./native"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
zig_version = "0.15.1"
main = "src/main.zig"
mode = "release_fast"
threaded = "single"
zigopt = ["-flto"]
use_cc_common_link = 1
bootstrapped = 0
args = ["release-arg"]

[[target]]
name = "archive"
kind = "zig_static_library"
srcs = ["src/**/*.zig"]
deps = ["./math", "./native"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/math.zig"

[[target]]
name = "archive_release"
kind = "zig_configure"
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
mode = "release_safe"
output = "static"

[[target]]
name = "shared_release"
kind = "zig_configure"
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
mode = "release_safe"
output = "shared"
shared_lib_name = "libcustom_configured.so"

[[target]]
name = "shared"
kind = "zig_shared_library"
srcs = ["src/**/*.zig"]
deps = ["./math", "./native"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/math.zig"
android_abi = "arm64-v8a"

[[target]]
name = "shared_named"
kind = "zig_shared_library"
srcs = ["src/**/*.zig"]
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/math.zig"
shared_lib_name = "libcustom_shared.so"

[[target]]
name = "archive_aux"
kind = "zig_static_library"
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
emit_bin = false
emit_asm = true
emit_llvm_ir = true
emit_llvm_bc = true

[[target]]
name = "tests"
kind = "zig_test"
srcs = ["src/**/*.zig"]
deps = ["./math", "./native"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/test.zig"
args = ["test-arg"]
labels = ["unit"]

[[target]]
name = "tests_custom"
kind = "zig_test"
srcs = ["src/**/*.zig"]
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/test.zig"
test_runner = "src/runner.zig"
env_inherit = ["ONCE_ZIG_E2E_ENV"]
test_env = { ONCE_ZIG_TEST_ENV = "configured" }
args = ["custom-test-arg"]
labels = ["custom"]

[[target]]
name = "tests_fail"
kind = "zig_test"
srcs = ["src/**/*.zig"]
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/test.zig"
args = ["fail-test"]
labels = ["failure"]

[[target]]
name = "tests_release"
kind = "zig_configure_test"
srcs = ["src/**/*.zig"]
deps = ["./math", "./native"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
main = "src/test.zig"
mode = "debug"
threaded = "multi"
args = ["release-test-arg"]
labels = ["unit"]

[[target]]
name = "tests_from_module"
kind = "zig_test"
deps = ["./math"]

[target.attrs]
zig = "$WORKSPACE/bin/zig"
labels = ["module"]

[[target]]
name = "native_zig"
kind = "zig_c_library"
deps = ["./native"]

[target.attrs]
translate_c = "$WORKSPACE/bin/translate-c"
translate_c_identity = "translate-c mock v1"
use_standalone_translate_c = 1
mode = "debug"
TOML
  }

  write_provider_only_c_workspace() {
    mkdir -p "$WORKSPACE/include" "$WORKSPACE/vendor"
    cat > "$WORKSPACE/include/native.h" <<'H'
int native_answer(void);
H
    printf '%s\n' 'dynamic library placeholder' > "$WORKSPACE/vendor/mylib.so"
    cat > "$WORKSPACE/once.toml" <<TOML
[[target]]
name = "native_headers"
kind = "c_library"

[target.attrs]
hdrs = ["include/native.h"]
dynamic_libraries = ["vendor/mylib.so"]
compiler = "$WORKSPACE/bin/missing-cc"
cxx_compiler = "$WORKSPACE/bin/missing-cxx"
archiver = "$WORKSPACE/bin/missing-ar"
TOML
  }

  It 'discovers C and Zig target kinds through the public query surface'
    once --format json query target-kinds > "$WORKSPACE/target_kinds.json"

    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"c_library"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"c-library-minimal"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_library"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-library-minimal"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_binary"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-binary-with-library"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_c_library"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-c-library-with-c-provider"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_static_library"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-static-library-minimal"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_shared_library"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-shared-library-minimal"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_test"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-test-minimal"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_configure"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-configure-library-minimal"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_configure_binary"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-configure-binary-minimal"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"kind":"zig_configure_test"'
    The contents of file "$WORKSPACE/target_kinds.json" should include '"slug":"zig-configure-test-minimal"'
  End

  It 'describes C and Zig provider contracts through query schema'
    once --format json query schema c_library > "$WORKSPACE/c_library.json"
    once --format json query schema zig_library > "$WORKSPACE/zig_library.json"
    once --format json query schema zig_c_library > "$WORKSPACE/zig_c_library.json"
    once --format json query schema zig_binary > "$WORKSPACE/zig_binary.json"
    once --format json query schema zig_static_library > "$WORKSPACE/zig_static_library.json"
    once --format json query schema zig_shared_library > "$WORKSPACE/zig_shared_library.json"
    once --format json query schema zig_configure > "$WORKSPACE/zig_configure.json"
    once --format json query schema zig_test > "$WORKSPACE/zig_test.json"
    once --format json query schema zig_configure_binary > "$WORKSPACE/zig_configure_binary.json"
    once --format json query schema zig_configure_test > "$WORKSPACE/zig_configure_test.json"

    The contents of file "$WORKSPACE/c_library.json" should include '"providers":["c_provider","native_linkable","apple_linkable","android_native_library"]'
    The contents of file "$WORKSPACE/c_library.json" should include '"name":"archiver_identity"'
    The contents of file "$WORKSPACE/zig_library.json" should include '"providers":["zig_module"]'
    The contents of file "$WORKSPACE/zig_c_library.json" should include '"name":"translate_c_identity"'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"name":"run"'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"requires_outputs":["binary"]'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"name":"mode"'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"name":"threaded"'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"name":"zigopt"'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"name":"zig_version"'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"name":"use_cc_common_link"'
    The contents of file "$WORKSPACE/zig_binary.json" should include '"name":"use_standalone_translate_c"'
    The contents of file "$WORKSPACE/zig_static_library.json" should include '"c_provider"'
    The contents of file "$WORKSPACE/zig_static_library.json" should include '"apple_linkable"'
    The contents of file "$WORKSPACE/zig_shared_library.json" should include '"android_native_library"'
    The contents of file "$WORKSPACE/zig_configure.json" should include '"name":"output"'
    The contents of file "$WORKSPACE/zig_test.json" should include '"once_test_info"'
    The contents of file "$WORKSPACE/zig_test.json" should include '"requires_outputs":["binary"]'
    The contents of file "$WORKSPACE/zig_configure_binary.json" should include '"name":"run"'
    The contents of file "$WORKSPACE/zig_configure_test.json" should include '"once_test_info"'
  End

  It 'materializes C and Zig starter examples'
    once --format json query example c_library c-library-minimal > "$WORKSPACE/c_example.json"
    once --format json query example zig_library zig-library-minimal > "$WORKSPACE/zig_library_example.json"
    once --format json query example zig_binary zig-binary-with-library > "$WORKSPACE/zig_example.json"
    once --format json query example zig_c_library zig-c-library-with-c-provider > "$WORKSPACE/zig_c_example.json"
    once --format json query example zig_static_library zig-static-library-minimal > "$WORKSPACE/zig_static_example.json"
    once --format json query example zig_shared_library zig-shared-library-minimal > "$WORKSPACE/zig_shared_example.json"
    once --format json query example zig_test zig-test-minimal > "$WORKSPACE/zig_test_example.json"
    once --format json query example zig_configure zig-configure-library-minimal > "$WORKSPACE/zig_configure_library_example.json"
    once --format json query example zig_configure_binary zig-configure-binary-minimal > "$WORKSPACE/zig_configure_example.json"
    once --format json query example zig_configure_test zig-configure-test-minimal > "$WORKSPACE/zig_configure_test_example.json"

    The contents of file "$WORKSPACE/c_example.json" should include '"slug":"c-library-minimal"'
    The contents of file "$WORKSPACE/c_example.json" should include 'kind = \"c_library\"'
    The contents of file "$WORKSPACE/zig_library_example.json" should include 'kind = \"zig_library\"'
    The contents of file "$WORKSPACE/zig_example.json" should include '"slug":"zig-binary-with-library"'
    The contents of file "$WORKSPACE/zig_example.json" should include 'kind = \"zig_binary\"'
    The contents of file "$WORKSPACE/zig_c_example.json" should include '"slug":"zig-c-library-with-c-provider"'
    The contents of file "$WORKSPACE/zig_c_example.json" should include 'kind = \"zig_c_library\"'
    The contents of file "$WORKSPACE/zig_static_example.json" should include 'kind = \"zig_static_library\"'
    The contents of file "$WORKSPACE/zig_shared_example.json" should include 'kind = \"zig_shared_library\"'
    The contents of file "$WORKSPACE/zig_test_example.json" should include 'kind = \"zig_test\"'
    The contents of file "$WORKSPACE/zig_configure_library_example.json" should include 'kind = \"zig_configure\"'
    The contents of file "$WORKSPACE/zig_configure_example.json" should include '"slug":"zig-configure-binary-minimal"'
    The contents of file "$WORKSPACE/zig_configure_example.json" should include 'kind = \"zig_configure_binary\"'
    The contents of file "$WORKSPACE/zig_configure_test_example.json" should include 'kind = \"zig_configure_test\"'
  End

  It 'builds a C provider target'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json build native > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The path "$WORKSPACE/.once/out/native/libnative.a" should be file
    The contents of file "$WORKSPACE/.once/out/native/libnative.a" should include 'archive'
    The path "$WORKSPACE/.once/out/native/objects/src/native.c.o" should be file
    The contents of file "$WORKSPACE/.once/out/native/objects/src/native.c.o" should include 'object'
  End

  It 'builds a provider-only C target without probing C tools'
    write_provider_only_c_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json build native_headers > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    [ ! -e "$WORKSPACE/.once/out/native_headers/libnative_headers.a" ]
  End

  It 'builds a C++ provider target with the C++ compiler only when needed'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json build native_cxx > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The path "$WORKSPACE/.once/out/native_cxx/libnative_cxx.a" should be file
    The path "$WORKSPACE/.once/out/native_cxx/objects/src/native.cc.o" should be file
    The contents of file "$WORKSPACE/cc-argv.log" should include 'cxx -c src/native.cc'
  End

  It 'builds Zig binary, configured binary, static library, configured library, and shared library targets with C provider deps'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json build app > /dev/null && "$1" -C "$2" --format json build app_release > /dev/null && "$1" -C "$2" --format json build archive > /dev/null && "$1" -C "$2" --format json build archive_release > /dev/null && "$1" -C "$2" --format json build shared > /dev/null && "$1" -C "$2" --format json build shared_release > /dev/null && "$1" -C "$2" --format json build shared_named > /dev/null && "$1" -C "$2" --format json build native_zig > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The path "$WORKSPACE/.once/out/app/app" should be file
    The contents of file "$WORKSPACE/.once/out/app/app" should include 'zig executable mock'
    The path "$WORKSPACE/.once/out/app_release/app_release" should be file
    The contents of file "$WORKSPACE/.once/out/app_release/app_release" should include 'zig executable mock'
    The path "$WORKSPACE/.once/out/app/c_c.zig" should be file
    The contents of file "$WORKSPACE/.once/out/app/c_c.zig" should include 'translated_c_header'
    The path "$WORKSPACE/.once/out/app/app.docs/index.html" should be file
    The contents of file "$WORKSPACE/.once/out/app/app.docs/index.html" should include 'docs for build-exe'
    The path "$WORKSPACE/.once/out/archive/libarchive.a" should be file
    The contents of file "$WORKSPACE/.once/out/archive/libarchive.a" should include 'zig library mock'
    The path "$WORKSPACE/.once/out/archive_release/libarchive_release.a" should be file
    The contents of file "$WORKSPACE/.once/out/archive_release/libarchive_release.a" should include 'zig library mock'
    shared_library="$WORKSPACE/.once/out/shared/libshared.so"
    [ -f "$shared_library" ] || shared_library="$WORKSPACE/.once/out/shared/libshared.dylib"
    The path "$shared_library" should be file
    The contents of file "$shared_library" should include 'zig library mock'
    The path "$WORKSPACE/.once/out/shared_release/libcustom_configured.so" should be file
    The contents of file "$WORKSPACE/.once/out/shared_release/libcustom_configured.so" should include 'zig library mock'
    The path "$WORKSPACE/.once/out/shared_named/libcustom_shared.so" should be file
    The contents of file "$WORKSPACE/.once/out/shared_named/libcustom_shared.so" should include 'zig library mock'
    The path "$WORKSPACE/.once/out/native_zig/native_zig_c.zig" should be file
    The contents of file "$WORKSPACE/.once/out/native_zig/native_zig_c.zig" should include 'standalone_translated_c_header'
    The contents of file "$WORKSPACE/zig-argv.log" should include '-O ReleaseFast'
    The contents of file "$WORKSPACE/zig-argv.log" should include '-fsingle-threaded'
    The contents of file "$WORKSPACE/zig-argv.log" should include '-flto'
    The contents of file "$WORKSPACE/translate-c-argv.log" should include '--emulate=clang'
  End

  It 'builds Zig targets that use dependency roots, direct C sources, collision-safe modules, and auxiliary outputs'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json build app_from_module > /dev/null && "$1" -C "$2" --format json build app_with_csrcs > /dev/null && "$1" -C "$2" --format json build app_collision > /dev/null && "$1" -C "$2" --format json build archive_aux > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The path "$WORKSPACE/.once/out/app_from_module/app_from_module" should be file
    The contents of file "$WORKSPACE/.once/out/app_from_module/app_from_module" should include 'zig executable mock'
    The path "$WORKSPACE/.once/out/app_with_csrcs/app_with_csrcs" should be file
    The contents of file "$WORKSPACE/.once/out/app_with_csrcs/app_with_csrcs" should include 'zig executable mock'
    The path "$WORKSPACE/.once/out/app_collision/app_collision" should be file
    The contents of file "$WORKSPACE/.once/out/app_collision/app_collision" should include 'zig executable mock'
    The path "$WORKSPACE/.once/out/archive_aux/archive_aux.s" should be file
    The contents of file "$WORKSPACE/.once/out/archive_aux/archive_aux.s" should include 'auxiliary output for build-lib'
    The path "$WORKSPACE/.once/out/archive_aux/archive_aux.ll" should be file
    The contents of file "$WORKSPACE/.once/out/archive_aux/archive_aux.ll" should include 'auxiliary output for build-lib'
    The path "$WORKSPACE/.once/out/archive_aux/archive_aux.bc" should be file
    The contents of file "$WORKSPACE/.once/out/archive_aux/archive_aux.bc" should include 'auxiliary output for build-lib'
    The contents of file "$WORKSPACE/zig-argv.log" should include '-cflags -DLOCAL_C_SOURCE -- src/native.c'
    The contents of file "$WORKSPACE/zig-argv.log" should include 'dash=once_foo_x45_bar'
    The contents of file "$WORKSPACE/zig-argv.log" should include 'underscore=once_foo_ubar'
  End

  It 'runs a Zig binary target through once run'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json run app > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The path "$WORKSPACE/.once/out/app/run/run.json" should be file
    The path "$WORKSPACE/.once/out/app/run/stdout.log" should be file
    The contents of file "$WORKSPACE/.once/out/app/run/run.json" should include '"target":"app"'
    The contents of file "$WORKSPACE/.once/out/app/run/run.json" should include '"exit_code":0'
    The contents of file "$WORKSPACE/.once/out/app/run/stdout.log" should include 'zig executable mock:run-arg'
  End

  It 'runs a Zig test target through once test'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json test tests > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The path "$WORKSPACE/.once/out/tests/test/test_results.json" should be file
    The path "$WORKSPACE/.once/out/tests/test/zig-test.log" should be file
    The contents of file "$WORKSPACE/.once/out/tests/test/test_results.json" should include '"target":"tests"'
    The contents of file "$WORKSPACE/.once/out/tests/test/test_results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/.once/out/tests/test/zig-test.log" should include 'zig test mock:test-arg'
  End

  It 'runs a configured Zig test target through once test'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json test tests_release > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The path "$WORKSPACE/.once/out/tests_release/test/test_results.json" should be file
    The contents of file "$WORKSPACE/.once/out/tests_release/test/test_results.json" should include '"target":"tests_release"'
    The contents of file "$WORKSPACE/.once/out/tests_release/test/test_results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/.once/out/tests_release/test/zig-test.log" should include 'zig test mock:release-test-arg'
    The contents of file "$WORKSPACE/zig-argv.log" should include '-fno-single-threaded'
  End

  It 'runs a Zig test with custom runner and inherited environment'
    write_zig_workspace

    When call /bin/sh -c 'ONCE_ZIG_E2E_ENV=from-host "$1" -C "$2" --format json test tests_custom > /dev/null' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be success
    The contents of file "$WORKSPACE/.once/out/tests_custom/test/test_results.json" should include '"target":"tests_custom"'
    The contents of file "$WORKSPACE/.once/out/tests_custom/test/test_results.json" should include '"status":"passed"'
    The contents of file "$WORKSPACE/.once/out/tests_custom/test/zig-test.log" should include 'zig test mock:custom-test-arg env:from-host test_env:configured'
    The contents of file "$WORKSPACE/zig-argv.log" should include '--test-runner src/runner.zig'
  End

  It 'writes failed Zig test result artifacts'
    write_zig_workspace

    When call /bin/sh -c '"$1" -C "$2" --format json test tests_fail > /dev/null 2> "$2/tests_fail.stderr"' sh "$ONCE_BIN" "$WORKSPACE"
    The status should be failure
    The path "$WORKSPACE/.once/out/tests_fail/test/test_results.json" should be file
    The path "$WORKSPACE/.once/out/tests_fail/test/zig-test.log" should be file
    The path "$WORKSPACE/.once/out/tests_fail/test/native_results.txt" should be file
    The contents of file "$WORKSPACE/.once/out/tests_fail/test/test_results.json" should include '"target":"tests_fail"'
    The contents of file "$WORKSPACE/.once/out/tests_fail/test/test_results.json" should include '"status":"failed"'
    The contents of file "$WORKSPACE/.once/out/tests_fail/test/zig-test.log" should include 'zig test mock:fail-test'
    The contents of file "$WORKSPACE/.once/out/tests_fail/test/native_results.txt" should include 'exit: 1'
  End

  It 'discovers dependency-rooted Zig tests through once query tests'
    write_zig_workspace

    once --format json query tests > "$WORKSPACE/tests_query.json"

    The contents of file "$WORKSPACE/tests_query.json" should include '"id":"tests_from_module"'
    The contents of file "$WORKSPACE/tests_query.json" should include '"kind":"zig_test"'
    The contents of file "$WORKSPACE/tests_query.json" should include '"runner":"zig_test"'
    The contents of file "$WORKSPACE/tests_query.json" should include '"labels":["module"]'
  End
End
