#shellcheck shell=bash

Describe 'Native container images'
  BeforeEach 'setup_workspace'
  AfterEach 'cleanup_workspace'

  engine_unavailable() {
    ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1 || ! crane version >/dev/null 2>&1 || ! command -v go >/dev/null 2>&1
  }

  registry_unconfigured() {
    engine_unavailable || [ -z "${ONCE_TEST_REGISTRY:-}" ]
  }

  stage_example() {
    cp -R "$REPO_ROOT/crates/once-frontend/prelude/examples/oci-pull-and-extend/." "$WORKSPACE/"
    export DOCKER_CONFIG="${DOCKER_CONFIG:-$SPEC_ORIGINAL_HOME/.docker}"
  }

  pull_extend_and_run() {
    stage_example
    once --format json build hello_image | jq -e '.status == "completed"' >/dev/null || return
    once run hello_load >/dev/null || return
    output="$(docker run --rm once-hello:latest /usr/local/bin/hello)" || return
    [ "$output" = "hello from a Once container" ] || return
    release="$(docker run --rm once-hello:latest cat /etc/alpine-release)" || return
    [ -n "$release" ]
  }

  It 'extends a digest-pinned base, loads the image, and runs it'
    Skip if 'Docker, crane, and Go are required' engine_unavailable
    When call pull_extend_and_run
    The status should be success
  End

  push_matches_the_local_digest() {
    stage_example
    sed -i.bak "s#registry.example.com/team/hello#${ONCE_TEST_REGISTRY}/team/hello#; s#^remote_tags = .*#remote_tags = [\"v1\", \"latest\"]\ninsecure = true#" "$WORKSPACE/once.toml"
    once --format json build hello_image | jq -e '.status == "completed"' >/dev/null || return
    local_digest="$(jq -r '.manifests[0].digest' "$WORKSPACE/.once/out/hello_image/hello_image.oci/index.json")" || return
    once run hello_push >/dev/null || return
    for tag in v1 latest; do
      remote="$(curl -fsSI -H 'Accept: application/vnd.oci.image.manifest.v1+json' "http://${ONCE_TEST_REGISTRY}/v2/team/hello/manifests/${tag}" | tr -d '\r' | awk -F': ' 'tolower($1)=="docker-content-digest"{print $2}')" || return
      [ "$remote" = "$local_digest" ] || return
    done
  }

  It 'publishes an image under the digest of the local layout'
    Skip if 'set ONCE_TEST_REGISTRY to a plain-HTTP registry such as localhost:5000' registry_unconfigured
    When call push_matches_the_local_digest
    The status should be success
  End
End
