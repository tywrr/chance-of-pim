#!/usr/bin/env bash
# Applies Chance of PIM to the cluster in $KUBECONFIG and waits for the tunnel and /readyz.
#   deploy/k8s/apply.sh [--rotate] [OVERLAY_DIR]     (default overlay: deploy/k8s/base)
# Secrets come from $COP_SECRETS_DIR (default ~/.config/chance-of-pim), never from the repo:
#   tunnel-token     the tunnel's token (wrangler tunnel create / the dashboard)
#   internal-token   made here if missing; give the same value to the auth Worker
#   accounts.env     NAME=value lines, one per password_env in your config.toml
# --rotate rebuilds both Secrets from those files and restarts the pod.
set -euo pipefail

rotate=0
if [ "${1:-}" = --rotate ]; then rotate=1; shift; fi
dir=${1:-"$(dirname "$0")/base"}
[ -f "$dir/kustomization.yaml" ] || { echo "no kustomization.yaml in $dir" >&2; exit 1; }
secrets=${COP_SECRETS_DIR:-$HOME/.config/chance-of-pim}
ns=chance-of-pim
k() { kubectl -n "$ns" "$@"; }

if [ -n "${COP_EXPECT_NODE:-}" ]; then
  node=$(kubectl get nodes -o jsonpath='{.items[*].metadata.name}')
  [ "$node" = "$COP_EXPECT_NODE" ] || { echo "refusing: cluster node is '$node', not $COP_EXPECT_NODE" >&2; exit 1; }
fi

[ -s "$secrets/tunnel-token" ] || { echo "missing $secrets/tunnel-token" >&2; exit 1; }
[ -s "$secrets/accounts.env" ] || { echo "missing $secrets/accounts.env" >&2; exit 1; }
if [ ! -s "$secrets/internal-token" ]; then
  (umask 077 && openssl rand -hex 32 | tr -d '\n' >"$secrets/internal-token")
  echo "made $secrets/internal-token"
fi

# The namespace first, so the Secrets exist before the pod wants them.
kubectl apply -k "$dir" -l kubernetes.io/metadata.name="$ns"

changed=0
if [ "$rotate" = 1 ]; then
  k delete secret chance-of-pim chance-of-pim-accounts --ignore-not-found
fi
if ! k get secret chance-of-pim >/dev/null 2>&1; then
  k create secret generic chance-of-pim \
    --from-file=internal-token=<(tr -d '\n' <"$secrets/internal-token") \
    --from-file=tunnel-token=<(tr -d '\n' <"$secrets/tunnel-token")
  changed=1
fi
if ! k get secret chance-of-pim-accounts >/dev/null 2>&1; then
  k create secret generic chance-of-pim-accounts --from-env-file="$secrets/accounts.env"
  changed=1
fi

kubectl apply -k "$dir"
if [ "$rotate" = 1 ] && [ "$changed" = 1 ]; then
  k rollout restart deploy/chance-of-pim
fi
k rollout status deploy/chance-of-pim --timeout=300s

n=0
for _ in $(seq 1 30); do
  n=$(k logs deploy/chance-of-pim -c cloudflared 2>/dev/null | grep -c 'Registered tunnel connection' || true)
  [ "$n" -ge 4 ] && break
  sleep 2
done
echo "tunnel connections registered: $n"

port=${COP_LOCAL_PORT:-18090}
k port-forward deploy/chance-of-pim "$port:8080" >/dev/null 2>&1 &
pf=$!
trap 'kill "$pf" 2>/dev/null || true' EXIT
ready=""
for _ in $(seq 1 15); do
  ready=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/readyz" || true)
  [ "$ready" = 200 ] && break
  sleep 2
done
echo "readyz: ${ready:-no answer}"
if [ "$n" -lt 4 ] || [ "$ready" != 200 ]; then k logs deploy/chance-of-pim -c core --tail=20 >&2; exit 1; fi
