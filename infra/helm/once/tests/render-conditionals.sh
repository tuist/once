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
# The image is public, so the default values render no imagePullSecrets.
if grep -q '^[[:space:]]*imagePullSecrets:' "$tmpdir/default.yaml"; then
  echo "imagePullSecrets rendered with the default (empty) image.pullSecretName" >&2
  exit 1
fi

render --set-string image.pullSecretName=ghcr-pull >"$tmpdir/image-pull-secret.yaml"
if ! grep -q '^[[:space:]]*imagePullSecrets:' "$tmpdir/image-pull-secret.yaml"; then
  echo "imagePullSecrets not rendered when image.pullSecretName was set" >&2
  exit 1
fi

# The DNS-01 issuer contributes its own ExternalSecret (for the Cloudflare
# token); disable it here so the counts isolate the app/pull-secret matrix.
render --set dnsIssuer.enabled=false --set externalSecrets.enabled=true --set externalSecrets.pullSecret.enabled=true --set-string image.pullSecretName=ghcr-pull >"$tmpdir/external-secrets-enabled-pull-secret-enabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-enabled-pull-secret-enabled.yaml" 2

render --set dnsIssuer.enabled=false --set externalSecrets.enabled=true --set externalSecrets.pullSecret.enabled=false >"$tmpdir/external-secrets-enabled-pull-secret-disabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-enabled-pull-secret-disabled.yaml" 1

render --set dnsIssuer.enabled=false --set externalSecrets.enabled=false --set externalSecrets.pullSecret.enabled=true >"$tmpdir/external-secrets-disabled-pull-secret-enabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-disabled-pull-secret-enabled.yaml" 0

render --set dnsIssuer.enabled=false --set externalSecrets.enabled=false --set externalSecrets.pullSecret.enabled=false >"$tmpdir/external-secrets-disabled-pull-secret-disabled.yaml"
expect_external_secret_count "$tmpdir/external-secrets-disabled-pull-secret-disabled.yaml" 0

render \
  --set dnsIssuer.enabled=false \
  --set externalSecrets.enabled=true \
  --set externalSecrets.pullSecret.enabled=true \
  --set-string image.pullSecretName= \
  >"$tmpdir/external-secrets-enabled-pull-secret-enabled-no-name.yaml"
expect_external_secret_count "$tmpdir/external-secrets-enabled-pull-secret-enabled-no-name.yaml" 1

# The namespaced Issuer renders whenever dnsIssuer.enabled, but its Cloudflare
# token ExternalSecret is gated on externalSecrets.enabled like the rest of the
# chart's secret syncing, so it stays installable on clusters without the
# External Secrets CRD.
render --set dnsIssuer.enabled=true --set externalSecrets.enabled=false >"$tmpdir/dns-issuer-no-es.yaml"
expect_external_secret_count "$tmpdir/dns-issuer-no-es.yaml" 0
if ! grep -q '^kind: Issuer$' "$tmpdir/dns-issuer-no-es.yaml"; then
  echo "namespaced Issuer not rendered when dnsIssuer.enabled and externalSecrets disabled" >&2
  exit 1
fi

render --set dnsIssuer.enabled=true --set externalSecrets.enabled=true >"$tmpdir/dns-issuer-with-es.yaml"
if ! grep -q 'name: cloudflare-buildonce-token' "$tmpdir/dns-issuer-with-es.yaml"; then
  echo "token ExternalSecret not rendered when dnsIssuer and externalSecrets enabled" >&2
  exit 1
fi
if ! grep -q '^kind: Issuer$' "$tmpdir/dns-issuer-with-es.yaml"; then
  echo "namespaced Issuer not rendered when dnsIssuer.enabled=true" >&2
  exit 1
fi

render --set dnsIssuer.enabled=false --set externalSecrets.enabled=false >"$tmpdir/dns-issuer-disabled.yaml"
if grep -q '^kind: Issuer$' "$tmpdir/dns-issuer-disabled.yaml"; then
  echo "namespaced Issuer rendered when dnsIssuer.enabled=false" >&2
  exit 1
fi
