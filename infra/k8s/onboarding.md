# Struo Cluster Onboarding

This runbook covers one-time production cluster setup for Struo. Day-to-day web
deploys should only build the image, apply the Struo website Helm release, and run smoke
tests. Cluster-level controllers such as external-dns are installed here.

## DNS Controller

external-dns watches the Struo website Ingress and keeps the `struo.run` and `buildonce.dev` Cloudflare
record in sync with the ingress-nginx LoadBalancer address. Install it once per
workload cluster after ingress-nginx is available.

Prerequisites:

- The current `KUBECONFIG` points at the Struo production workload cluster (still named `once-production`).
- The Cloudflare API token is stored at
  `op://struo-k8s-production/cloudflare-struo-dns/credential`.
- The token has `Zone:Read` and `DNS:Edit` permissions for the `struo.run` and `buildonce.dev` zones.

```bash
tmpdir="$(mktemp -d)"
chmod 700 "$tmpdir"
trap 'rm -rf "$tmpdir"' EXIT
token_file="$tmpdir/cloudflare-api-token"
op read 'op://struo-k8s-production/cloudflare-struo-dns/credential' | tr -d '\n' >"$token_file"
chmod 600 "$token_file"

kubectl create namespace external-dns --dry-run=client -o yaml | kubectl apply -f -
kubectl -n external-dns create secret generic cloudflare-api-token \
  --from-file=token="$token_file" \
  --dry-run=client -o yaml | kubectl apply -f -

helm repo add external-dns https://kubernetes-sigs.github.io/external-dns 2>/dev/null \
  || helm repo update external-dns
helm upgrade --install external-dns external-dns/external-dns \
  --version 1.21.1 \
  -n external-dns --create-namespace \
  -f infra/k8s/external-dns-values.yaml \
  --timeout 5m --wait
```

Validate:

```bash
kubectl -n external-dns get deploy external-dns
dig @1.1.1.1 struo.run A +short
```
