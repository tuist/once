# Once Cluster Onboarding

This runbook covers one-time production cluster setup for Once. Day-to-day web
deploys should only build the image, apply the Once Helm release, and run smoke
tests. Cluster-level controllers such as external-dns are installed here.

## DNS Controller

external-dns watches the Once Ingress and keeps the `buildonce.dev` Cloudflare
record in sync with the ingress-nginx LoadBalancer address. Install it once per
workload cluster after ingress-nginx is available.

Prerequisites:

- The current `KUBECONFIG` points at the Once production workload cluster.
- The Cloudflare API token is stored at
  `op://once-k8s-production/cloudflare-buildonce-dns/credential`.
- The token has `Zone:Read` and `DNS:Edit` permissions for `buildonce.dev`.

```bash
CLOUDFLARE_API_TOKEN="$(op read 'op://once-k8s-production/cloudflare-buildonce-dns/credential')"

kubectl create namespace external-dns --dry-run=client -o yaml | kubectl apply -f -
kubectl -n external-dns create secret generic cloudflare-api-token \
  --from-literal=token="$CLOUDFLARE_API_TOKEN" \
  --dry-run=client -o yaml | kubectl apply -f -
unset CLOUDFLARE_API_TOKEN

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
dig @1.1.1.1 buildonce.dev A +short
```
