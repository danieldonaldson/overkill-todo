#!/usr/bin/env bash
# Local Kubernetes cluster: one Talos VM under QEMU (OT-014, deploy/k8s/README.md).
#
#   k8s.sh up      create the cluster and hand it to ArgoCD
#   k8s.sh down    destroy it, whatever state it is in
#   k8s.sh status  what is out of sync, failing or stuck; exits 1 if anything is
#   k8s.sh env     print the exports for kubectl and talosctl
#
# Runs as you; only the VM, its network and one firewall rule need sudo, and each
# such command is printed first. State is in $XDG_STATE_HOME/overkill-todo/k8s.
set -euo pipefail
cd "$(dirname "$0")/.."

name=overkill-todo
cidr=10.5.0.0/24
gateway=10.5.0.1
node=10.5.0.2
cpus=4
mem_gib=12
disk_gib=40
# Left over for everything else once the VM has its memory.
spare_mem_gib=2

state=${XDG_STATE_HOME:-$HOME/.local/state}/overkill-todo/k8s
age_key=${OVERKILL_CLUSTER_AGE_KEY:-$HOME/.config/overkill-todo/cluster.agekey}
talos_secrets=deploy/secrets/local/talos.sops
export KUBECONFIG=$state/kubeconfig TALOSCONFIG=$state/talosconfig

# shellcheck source=deploy/k8s/versions.env
. deploy/k8s/versions.env

die() { echo "k8s: $*" >&2; exit 1; }
say() { echo "k8s: $*" >&2; }
as_root() { say "sudo $*"; sudo "$@"; }
# sudo resets PATH, so name the binary. Resolved here, in the repo, because mise
# picks the pinned version by directory.
talosctl_bin=$(mise which talosctl 2>/dev/null || command -v talosctl || true)
root_talosctl() { as_root env HOME="$HOME" "$talosctl_bin" "$@" --name "$name" --state "$state/clusters"; }
bridge() { ip -o -4 addr show | awk -v a="$gateway/" 'index($4, a) == 1 {print $2; exit}'; }
ufw_active() { systemctl is-active --quiet ufw; }

wait_for() { # seconds, what, command...
  local deadline=$((SECONDS + $1)) what=$2
  shift 2
  say "waiting for $what"
  until "$@" >/dev/null 2>&1; do
    ((SECONDS < deadline)) || die "timed out waiting for $what"
    sleep 3
  done
}

# Checks only; changes nothing. Reports every problem at once.
preflight() {
  local problems=() tool have dir
  for tool in talosctl kubectl helm sops qemu-system-x86_64 ip sudo; do
    command -v "$tool" >/dev/null || problems+=("$tool is not installed")
  done
  [[ -r /dev/kvm && -w /dev/kvm ]] || problems+=("/dev/kvm is not usable")
  have=$(awk '/^MemAvailable:/ {print int($2 / 1048576)}' /proc/meminfo)
  ((have >= mem_gib + spare_mem_gib)) ||
    problems+=("memory: $have GiB available, $((mem_gib + spare_mem_gib)) GiB needed ($mem_gib for the VM)")
  dir=$state
  while [[ ! -d $dir ]]; do dir=$(dirname "$dir"); done
  have=$(df --output=avail -BG "$dir" | tail -1 | tr -dc 0-9)
  ((have >= disk_gib + 5)) || problems+=("disk: $have GiB free at $dir, $((disk_gib + 5)) GiB needed")
  [[ -f $age_key ]] || problems+=("the cluster's age key is missing: $age_key")
  [[ -f $talos_secrets ]] || problems+=("$talos_secrets is missing")
  [[ ! -e $state/clusters/$name ]] || problems+=("a cluster (or part of one) exists: run \`just k8s-down\` first")
  [[ -z $(bridge) ]] || problems+=("$gateway is already in use on $(bridge)")
  if ((${#problems[@]})); then
    printf 'k8s: %s\n' "${problems[@]}" >&2
    die "nothing was changed"
  fi
}

# `kubectl get nodes` succeeds with no nodes, and `kubectl wait --all` then fails.
node_registered() { [[ -n $(kubectl get nodes -o name) ]]; }

install_cilium() {
  helm template cilium cilium --repo https://helm.cilium.io --version "$CILIUM_VERSION" \
    --namespace kube-system --kube-version "$KUBERNETES_VERSION" --include-crds \
    --values deploy/k8s/cluster/cilium/values.yaml |
    kubectl apply --server-side --field-manager=argocd-controller --force-conflicts -f - >/dev/null
}

# The last thing done by hand: from here ArgoCD manages the cluster, itself included.
bootstrap_argocd() {
  local apply=(kubectl apply --server-side --field-manager=argocd-controller --force-conflicts)
  "${apply[@]}" -f deploy/k8s/cluster/apps/namespaces.yaml >/dev/null
  # The cluster's only hand-placed secret; it decrypts the rest.
  kubectl -n sops-operator create secret generic age-key --from-file=key.txt="$age_key" \
    --dry-run=client -o yaml | kubectl apply --server-side -f - >/dev/null
  say "installing ArgoCD $ARGOCD_VERSION"
  helm template argocd argo-cd --repo https://argoproj.github.io/argo-helm --version "$ARGOCD_CHART_VERSION" \
    --namespace argocd --kube-version "$KUBERNETES_VERSION" --include-crds \
    --values deploy/k8s/cluster/argocd/values.yaml | "${apply[@]}" -n argocd -f - >/dev/null
  kubectl wait --for=condition=Established crd/applications.argoproj.io --timeout=60s >/dev/null
  kubectl -n argocd rollout status deployment/argocd-repo-server --timeout=300s
  kubectl -n argocd rollout status statefulset/argocd-application-controller --timeout=300s
  "${apply[@]}" -f deploy/k8s/cluster/apps/root.yaml >/dev/null
}

cmd_up() {
  preflight
  # Plaintext machine secrets live in memory only, and go when this exits.
  tmp=$(mktemp -d -p /dev/shm k8s.XXXXXX)
  trap 'rm -rf "$tmp" 2>/dev/null || sudo rm -rf "$tmp"' EXIT
  # Before sudo: fails here if you can't decrypt them.
  (umask 077 && sops decrypt --input-type binary --output-type binary "$talos_secrets" >"$tmp/secrets.yaml")
  talosctl gen config "$name" "https://$node:6443" --with-secrets "$tmp/secrets.yaml" \
    --talos-version "$TALOS_VERSION" --kubernetes-version "$KUBERNETES_VERSION" \
    --install-image "factory.talos.dev/metal-installer/376567988ad370138ad8b2698212367b8edcb69b5fd68c80be1f2ec7d603b4ba:$TALOS_VERSION" \
    --install-disk /dev/vda --config-patch @deploy/k8s/cluster/talos/patch.yaml \
    --with-docs=false --with-examples=false \
    --output-types controlplane,talosconfig --output "$tmp/config" >/dev/null
  talosctl validate --config "$tmp/config/controlplane.yaml" --mode metal >/dev/null

  mkdir -p "$state/clusters"
  chmod 700 "$state"
  # `maintenance` leaves the VM waiting for our config instead of making its own
  # keys; the throwaway config it writes goes to $tmp.
  repo=$PWD
  cd "$tmp"
  root_talosctl cluster create qemu --presets iso,maintenance --talos-version "$TALOS_VERSION" \
    --controlplanes 1 --workers 0 --cpus-controlplanes "$cpus" --memory-controlplanes "${mem_gib}GiB" \
    --disks "virtio:${disk_gib}GiB" --cidr "$cidr" --talosconfig-destination "$tmp/unused"
  cd "$repo"
  if ufw_active; then
    local br
    br=$(bridge)
    [[ -n $br ]] || die "no bridge has $gateway"
    # Without these the VM gets no DHCP answer and can't reach the internet.
    as_root ufw allow in on "$br" comment "$name k8s"
    as_root ufw route allow in on "$br" comment "$name k8s"
    as_root ufw route allow out on "$br" comment "$name k8s"
  fi

  install -m 600 "$tmp/config/talosconfig" "$TALOSCONFIG"
  talosctl config endpoint "$node"
  talosctl config node "$node"
  wait_for 300 "the VM" talosctl get disks --insecure --nodes "$node"
  talosctl apply-config --insecure --nodes "$node" --file "$tmp/config/controlplane.yaml"
  wait_for 600 "Talos to install and restart" talosctl version
  wait_for 300 "etcd to be bootstrapped" talosctl bootstrap
  wait_for 300 "the Kubernetes API's credentials" talosctl kubeconfig "$KUBECONFIG" --force
  chmod 600 "$KUBECONFIG"
  wait_for 600 "the Kubernetes API" kubectl get --raw /readyz
  wait_for 300 "the node to register" node_registered
  say "installing Cilium $CILIUM_VERSION"
  wait_for 120 "Cilium to be applied" install_cilium
  kubectl wait --for=condition=Ready node --all --timeout=600s
  # The node is Ready before cluster DNS answers.
  kubectl -n kube-system rollout status deployment/coredns --timeout=300s
  bootstrap_argocd
  say "ready. For kubectl and talosctl: eval \"\$(./scripts/k8s.sh env)\""
}

cmd_down() {
  local br
  br=$(bridge)
  if [[ -e $state/clusters/$name ]]; then
    root_talosctl cluster destroy --force || say "destroy reported errors; cleaning up what is left"
  fi
  # A creation that stopped early can leave these behind.
  local pids
  mapfile -t pids < <(pgrep -f "qemu-system.*$state/clusters/$name/")
  if ((${#pids[@]})); then as_root kill "${pids[@]}" || true; fi
  if [[ -n $br ]]; then
    if ufw_active; then
      as_root ufw delete allow in on "$br" || true
      as_root ufw route delete allow in on "$br" || true
      as_root ufw route delete allow out on "$br" || true
    fi
    if ip link show "$br" >/dev/null 2>&1; then as_root ip link delete "$br" || true; fi
  fi
  [[ ! -e $state/clusters ]] || as_root rm -rf "$state/clusters"
  rm -f "$state/kubeconfig" "$state/talosconfig"
  say "the cluster is gone"
}

cmd_env() {
  # Without it kubectl falls back to localhost:8080, the compose edge's port.
  [[ -f $KUBECONFIG ]] || die "no cluster: run \`just k8s-up\` first"
  printf 'export KUBECONFIG=%q\nexport TALOSCONFIG=%q\n' "$KUBECONFIG" "$TALOSCONFIG"
}

# A failed sync, a secret that won't decrypt and a rollout that never finishes each
# show up in a different place; this looks in all of them. Names only, no values.
cmd_status() {
  [[ -f $KUBECONFIG ]] || die "no cluster: run \`just k8s-up\` first"
  local problems
  problems=$(
    kubectl -n argocd get applications -o json | jq -r '.items[]
      | .metadata.name as $n
      | (select(.status.sync.status != "Synced" or .status.health.status != "Healthy")
          | "application \($n): \(.status.sync.status // "Unknown"), \(.status.health.status // "Unknown")"),
        (.status.conditions[]? | "application \($n): \(.type): \(.message)"),
        (select(.status.operationState.phase as $p | ["Running", "Failed", "Error"] | index($p))
          | .status.operationState
          | "application \($n): sync \(.phase): \(.message)",
            (.syncResult.resources[]? | select((.status // "") == "SyncFailed" or (.hookPhase // "") == "Failed")
              | "application \($n): \(.kind) \(.namespace)/\(.name): \(.message)"))'
    kubectl get sopssecrets -A -o json 2>/dev/null | jq -r '.items[]
      | select(.status.message != "Healthy")
      | "secret \(.metadata.namespace)/\(.metadata.name): \(.status.message // "not decrypted yet")"'
    kubectl get deployments,statefulsets,daemonsets -A -o json | jq -r '.items[]
      | (.spec.replicas // .status.desiredNumberScheduled // 0) as $want
      | (.status.updatedReplicas // .status.updatedNumberScheduled // 0) as $new
      | (.status.availableReplicas // .status.numberAvailable // 0) as $up
      | (.status.replicas // $want) as $all
      | select($new < $want or $up < $want or $all > $want)
      | "\(.kind | ascii_downcase) \(.metadata.namespace)/\(.metadata.name): \($new)/\($want) updated, \($up) available"
        + ([.status.conditions[]? | select(.type == "Progressing" and .status == "False") | ": \(.message)"] | join(""))'
    kubectl get pods -A -o json | jq -r '.items[]
      | select(.status.phase != "Succeeded")
      | . as $p | (.status.containerStatuses // [])[], (.status.initContainerStatuses // [])[]
      | select(.state.waiting.reason // "" | . != "" and . != "ContainerCreating" and . != "PodInitializing")
      | "pod \($p.metadata.namespace)/\($p.metadata.name): \(.state.waiting.reason): \(.state.waiting.message // "")"'
  )
  if [[ -n $problems ]]; then
    echo "$problems"
    exit 1
  fi
  echo "every Application is synced and healthy, every secret decrypted, every rollout finished"
}

case ${1:-} in
  up | down | status | env) "cmd_$1" ;;
  *) sed -n '2,10s/^# \{0,1\}//p' "$0" >&2; exit 2 ;;
esac
