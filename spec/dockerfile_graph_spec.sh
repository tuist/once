#shellcheck shell=bash

Describe 'Dockerfile native project'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  discover_image_without_manifest() {
    cp "$REPO_ROOT/crates/once-frontend/prelude/examples/dockerfile-image-instructions/Dockerfile" "$WORKSPACE/Dockerfile"
    cp "$REPO_ROOT/crates/once-frontend/prelude/examples/dockerfile-image-instructions/message.txt" "$WORKSPACE/message.txt"
    test ! -e "$WORKSPACE/once.toml" || return
    once --format json query targets | jq -e 'any(.[]; .kind == "dockerfile_image" and .id == "image")' || return
    once --format json query target image | jq -e '.kind == "dockerfile_image"' || return
    once --format json lint image | jq -e '.status == "completed"' || return
  }

  It 'discovers and lints an image with no once.toml'
    When call discover_image_without_manifest
    The status should be success
    The stdout should include 'true'
  End
End

Describe 'Dockerfile default builder'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  docker_unavailable() {
    ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1
  }

  build_native_image_with_default_builder() {
    cp "$REPO_ROOT/crates/once-frontend/prelude/examples/dockerfile-image-instructions/Dockerfile" "$WORKSPACE/Dockerfile"
    cp "$REPO_ROOT/crates/once-frontend/prelude/examples/dockerfile-image-instructions/message.txt" "$WORKSPACE/message.txt"
    export DOCKER_CONFIG="${DOCKER_CONFIG:-$SPEC_ORIGINAL_HOME/.docker}"
    unset BUILDX_BUILDER
    test ! -e "$WORKSPACE/once.toml" || return
    once --format json build image | jq -e '.status == "completed"' || return
    test -s "$WORKSPACE/.once/out/image/image.docker.tar"
  }

  It 'builds a discovered image without configuring a builder'
    Skip if 'Docker is unavailable' docker_unavailable
    When call build_native_image_with_default_builder
    The status should be success
    The stdout should include 'true'
  End
End

Describe 'Dockerfile image layer caches'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  docker_builder_unconfigured() {
    [ -z "${ONCE_DOCKER_TEST_BUILDER:-}" ]
  }

  build_image_expect_cache() {
    result="$(once --format json build image)" || return
    printf '%s\n' "$result"
    [ "$(printf '%s' "$result" | jq -r '.cache')" = "$1" ]
  }

  build_and_restore_images() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/dockerfile-image-layer-cache/." "$WORKSPACE/"
    export DOCKER_CONFIG="${DOCKER_CONFIG:-$SPEC_ORIGINAL_HOME/.docker}"
    cat > "$WORKSPACE/once.toml" <<EOF_MANIFEST
[[target]]
name = "base_image"
kind = "dockerfile_image"
srcs = ["Dockerfile", "message.txt"]
[target.attrs]
execution_mode = "buildkit"
builder = "$ONCE_DOCKER_TEST_BUILDER"
cacheable = true
platform = "linux/arm64"
export_cache = true
pull = false
network = "none"

[[target]]
name = "image"
kind = "dockerfile_image"
srcs = ["context.Dockerfile", "message.txt"]
[target.dependencies]
caches = ["base_image"]
[target.attrs]
execution_mode = "buildkit"
dockerfile = "context.Dockerfile"
builder = "$ONCE_DOCKER_TEST_BUILDER"
cacheable = true
platform = "linux/arm64"
pull = false
network = "none"
EOF_MANIFEST
    build_image_expect_cache miss || return
    test -s "$WORKSPACE/.once/out/base_image/layer-cache/index.json" || return
    test -s "$WORKSPACE/.once/out/image/image.docker.tar" || return
    rm -rf "$WORKSPACE/.once/out"
    build_image_expect_cache hit || return
    test -s "$WORKSPACE/.once/out/base_image/layer-cache/index.json" || return
    test -s "$WORKSPACE/.once/out/image/image.docker.tar" || return
    printf 'changed source\n' > "$WORKSPACE/message.txt"
    build_image_expect_cache miss || return
    layers="$(tar -xOf "$WORKSPACE/.once/out/image/image.docker.tar" manifest.json | jq -r '.[0].Layers[]')" || return
    for layer in $layers; do
      members="$(tar -xOf "$WORKSPACE/.once/out/image/image.docker.tar" "$layer" | tar -tf -)" || return
      if printf '%s\n' "$members" | grep -E '(^|/)\.once(/|$)'; then
        return 1
      fi
    done
  }

  It 'imports declared layers and restores the archive and cache after output deletion'
    Skip if 'set ONCE_DOCKER_TEST_BUILDER to a container or remote builder' docker_builder_unconfigured
    When call build_and_restore_images
    The status should be success
    The stdout should include '"cache":"miss"'
    The stdout should include '"cache":"hit"'
  End
End

Describe 'Dockerfile instruction actions'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  instruction_builder_unconfigured() {
    [ -z "${ONCE_DOCKER_TEST_BUILDER:-}" ]
  }

  build_instruction_image() {
    result="$(once --format json build image)" || return
    printf '%s\n' "$result"
    [ "$(printf '%s' "$result" | jq -r '.cache')" = "$1" ]
  }

  restore_and_invalidate_instructions() {
    export PATH="/usr/bin:$PATH"
    export DOCKER_CONFIG="${DOCKER_CONFIG:-$SPEC_ORIGINAL_HOME/.docker}"
    cat > "$WORKSPACE/Dockerfile" <<'EOF_RECIPE'
FROM alpine:3.24.2@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6 AS base
WORKDIR /app
COPY early.txt /app/early.txt
RUN cat early.txt > built.txt
FROM base AS fork
COPY late.txt /app/late.txt
RUN test "$(stat -c %Y built.txt)" -gt 0 && cat built.txt late.txt > result.txt
FROM scratch
COPY --from=fork /app/result.txt /result.txt
CMD ["/result.txt"]
EOF_RECIPE
    printf 'early\n' > "$WORKSPACE/early.txt"
    printf 'late\n' > "$WORKSPACE/late.txt"
    cat > "$WORKSPACE/once.toml" <<EOF_MANIFEST
[[target]]
name = "image"
kind = "dockerfile_image"
srcs = ["Dockerfile", "early.txt", "late.txt"]
[target.attrs]
builder = "$ONCE_DOCKER_TEST_BUILDER"
platform = "linux/arm64"
cacheable = true
pull = false
network = "none"
EOF_MANIFEST
    build_instruction_image miss || return
    test -s "$WORKSPACE/.once/out/image/instructions/4/image/index.json" || return
    rm -rf "$WORKSPACE/.once/out"
    build_instruction_image hit || return
    test -s "$WORKSPACE/.once/out/image/instructions/4/image/index.json" || return
    printf 'changed late source\n' > "$WORKSPACE/late.txt"
    build_instruction_image miss || return
    latest_log="$(ls -t "$(once_log_dir)"/*.log | head -1)"
    jq -s -e 'any(.[]; .fields.identifier == "image:4:run" and .fields.cache == "hit")' "$latest_log" >/dev/null || return
    jq -s -e 'any(.[]; .fields.identifier == "image:6:copy" and .fields.cache == "miss")' "$latest_log" >/dev/null || return
    layer="$(tar -xOf "$WORKSPACE/.once/out/image/image.docker.tar" manifest.json | jq -r '.[0].Layers[-1]')" || return
    content="$(tar -xOf "$WORKSPACE/.once/out/image/image.docker.tar" "$layer" | tar -xOf - result.txt)" || return
    [ "$content" = "$(printf 'early\nchanged late source')" ] || return
    once --format json lint image || return
  }

  It 'restores instruction snapshots and reuses the prefix when a later source changes'
    Skip if 'set ONCE_DOCKER_TEST_BUILDER to a container or remote builder' instruction_builder_unconfigured
    When call restore_and_invalidate_instructions
    The status should be success
    The stdout should include '"cache":"hit"'
    The stdout should include '"status":"completed"'
  End
End
