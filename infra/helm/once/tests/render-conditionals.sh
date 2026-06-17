#!/usr/bin/env bash
set -euo pipefail

chart_path="${CHART_PATH:-infra/helm/once}"
rendered_manifest="${RENDERED_MANIFEST:-}"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

render() {
  helm template once "$chart_path" --namespace once-production "$@"
}

external_secret_count() {
  grep -c '^kind: ExternalSecret$' "$1" || true
}

expect_external_secret_count() {
  local file="$1"
  local expected="$2"
  local actual
  actual="$(external_secret_count "$file")"
  if [[ "$actual" != "$expected" ]]; then
    echo "expected $expected ExternalSecret resources in $file, found $actual" >&2
    exit 1
  fi
}

if [[ -n "$rendered_manifest" ]]; then
  if [[ ! -f "$rendered_manifest" ]]; then
    echo "rendered manifest not found: $rendered_manifest" >&2
    exit 1
  fi
  cp "$rendered_manifest" "$tmpdir/default.yaml"
else
  render >"$tmpdir/default.yaml"
fi
grep -q '^[[:space:]]*imagePullSecrets:' "$tmpdir/default.yaml"

render --set-string image.pullSecretName= >"$tmpdir/no-image-pull-secret.yaml"
if grep -q '^[[:space:]]*imagePullSecrets:' "$tmpdir/no-image-pull-secret.yaml"; then
  echo "imagePullSecrets rendered when image.pullSecretName was empty" >&2
  exit 1
fi

render --set externalSecrets.enabled=true --set externalSecrets.pullSecret.enabled=true >"$tmpdir/external-secrets-enabled-pull-secret-enabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-enabled-pull-secret-enabled.yaml" 2

render --set externalSecrets.enabled=true --set externalSecrets.pullSecret.enabled=false >"$tmpdir/external-secrets-enabled-pull-secret-disabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-enabled-pull-secret-disabled.yaml" 1

render --set externalSecrets.enabled=false --set externalSecrets.pullSecret.enabled=true >"$tmpdir/external-secrets-disabled-pull-secret-enabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-disabled-pull-secret-enabled.yaml" 0

render --set externalSecrets.enabled=false --set externalSecrets.pullSecret.enabled=false >"$tmpdir/external-secrets-disabled-pull-secret-disabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-disabled-pull-secret-disabled.yaml" 0

render \
  --set externalSecrets.enabled=true \
  --set externalSecrets.pullSecret.enabled=true \
  --set-string image.pullSecretName= \
  >"$tmpdir/external-secrets-enabled-pull-secret-enabled-no-name.yaml"
expect_external_secret_count "$tmpdir/external-secrets-enabled-pull-secret-enabled-no-name.yaml" 1
