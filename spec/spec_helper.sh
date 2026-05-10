# shellcheck shell=bash
# Shared shellspec helpers.

REPO_ROOT="$(cd "$SHELLSPEC_PROJECT_ROOT" && pwd)"
FABRIK_BIN="${REPO_ROOT}/target/release/fabrik"
export REPO_ROOT FABRIK_BIN

spec_helper_precheck() {
  if [ ! -x "$FABRIK_BIN" ]; then
    abort "fabrik binary missing at $FABRIK_BIN - run 'cargo build --release' first"
  fi
}

spec_helper_loaded() { :; }

setup_workspace() {
  WORKSPACE="$(mktemp -d -t fabrik-spec.XXXXXX)"
  export WORKSPACE
}

cleanup_workspace() {
  if [ -n "${WORKSPACE:-}" ] && [ -d "$WORKSPACE" ]; then
    rm -rf "$WORKSPACE"
    unset WORKSPACE
  fi
}

fabrik() {
  "$FABRIK_BIN" -C "$WORKSPACE" "$@"
}

copy_examples() {
  mkdir -p "$WORKSPACE/examples"
  cp -R "$REPO_ROOT/examples/." "$WORKSPACE/examples/"
}

fake_apple_tools() {
  mkdir -p "$WORKSPACE/bin"
  cat > "$WORKSPACE/bin/xcrun" <<'EOF'
#!/bin/sh
if [ "$1" = "--sdk" ] && [ "$3" = "--show-sdk-path" ]; then
  echo "/tmp/fake-$2.sdk"
  exit 0
fi
if [ "$1" = "--sdk" ] && [ "$3" = "swiftc" ]; then
  shift 3
  exec swiftc "$@"
fi
if [ "$1" = "ar" ] && [ "$2" = "crs" ]; then
  out="$3"
  mkdir -p "$(dirname "$out")"
  printf 'fake swift archive' > "$out"
  exit 0
fi
if [ "$1" = "simctl" ]; then
  echo "$*" >> simctl.log
  exit 0
fi
echo "unexpected xcrun invocation: $*" >&2
exit 2
EOF
  cat > "$WORKSPACE/bin/swiftc" <<'EOF'
#!/bin/sh
echo "swiftc $*" >> swiftc.log
out=""
module=""
emit_object=0
srcs=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    out="$1"
  elif [ "$1" = "-emit-module-path" ]; then
    shift
    module="$1"
  elif [ "$1" = "-emit-object" ]; then
    emit_object=1
  fi
  case "$1" in
    *.swift) srcs="$srcs $1" ;;
  esac
  shift
done
if [ -n "$out" ]; then
  mkdir -p "$(dirname "$out")"
  printf 'fake swift binary' > "$out"
fi
if [ "$emit_object" = 1 ]; then
  for src in $srcs; do
    object="$(basename "$src" .swift).o"
    printf 'fake swift object' > "$object"
  done
fi
if [ -n "$module" ]; then
  mkdir -p "$(dirname "$module")"
  printf 'fake swift module' > "$module"
fi
EOF
  cat > "$WORKSPACE/bin/codesign" <<'EOF'
#!/bin/sh
for last do :; done
app="$last"
mkdir -p "$app/_CodeSignature"
printf 'signed' > "$app/_CodeSignature/CodeResources"
EOF
  chmod +x "$WORKSPACE/bin/xcrun" "$WORKSPACE/bin/swiftc" "$WORKSPACE/bin/codesign"
}
