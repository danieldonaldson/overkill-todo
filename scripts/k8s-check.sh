#!/usr/bin/env bash
# Render every Kubernetes manifest and check it against its schema (OT-014 D13).
#
#   k8s-check.sh           check (needs network for charts and core schemas, no keys)
#   k8s-check.sh schemas   regenerate deploy/k8s/schemas/ from the pinned definitions
#
# Custom resources use the committed schemas, so a missing one fails rather than
# being skipped.
set -euo pipefail
cd "$(dirname "$0")/.."

# shellcheck source=deploy/k8s/versions.env
. deploy/k8s/versions.env
schemas=deploy/k8s/schemas
apps=deploy/k8s/cluster/apps
failed=0

fail() { echo "k8s-check: $*" >&2; failed=1; }

conform() { # label; manifests on stdin
  kubeconform -strict -kubernetes-version "$KUBERNETES_VERSION" -skip CustomResourceDefinition \
    -schema-location default \
    -schema-location "$schemas/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json" - ||
    fail "$1 does not match its schema"
}

check_rendered() { # label, command...
  local label=$1 out
  shift
  out=$("$@") || { fail "$label does not render"; return; }
  conform "$label" <<<"$out"
}

# Every chart an Application installs, with the version and values it names.
check_chart() {
  local app=$1 repo chart version values release ns
  IFS=$'\t' read -r repo chart version release values < <(yq -r \
    '.spec.sources[0] | [.repoURL, .chart, .targetRevision, .helm.releaseName, .helm.valueFiles[0]] | @tsv' "$app")
  ns=$(yq '.spec.destination.namespace' "$app")
  check_rendered "$app ($chart chart)" helm template "$release" "$chart" --repo "$repo" --version "$version" \
    --namespace "$ns" --kube-version "$KUBERNETES_VERSION" --values "${values/#\$repo\//}"
}

pinned() { # Application, versions.env value
  local have
  have=$(yq '.spec.sources[0].targetRevision' "$apps/$1.yaml")
  [[ $have == "$2" ]] || fail "$apps/$1.yaml installs $have, but versions.env pins $2"
}

cmd_check() {
  local dir f
  for dir in deploy/k8s/overlays/*/ deploy/k8s/cluster/storage deploy/k8s/cluster/edge; do
    check_rendered "$dir" kubectl kustomize "$dir"
  done
  for f in "$apps"/*.yaml deploy/secrets/*/*.sops.yaml; do
    [[ -f $f ]] || continue
    check_rendered "$f" cat "$f"
    if [[ $(yq 'select(document_index == 0) | .spec.sources[0].chart // ""' "$f") ]]; then check_chart "$f"; fi
  done
  pinned cilium "$CILIUM_VERSION"
  pinned argocd "$ARGOCD_CHART_VERSION"
  pinned sops-operator "$SOPS_OPERATOR_CHART_VERSION"
  pinned reloader "$RELOADER_CHART_VERSION"
  pinned cnpg "$CNPG_CHART_VERSION"
  pinned cert-manager "$CERT_MANAGER_VERSION"
  pinned traefik "$TRAEFIK_CHART_VERSION"
  pinned barman-cloud "$BARMAN_PLUGIN_CHART_VERSION"
  ((failed == 0)) || exit 1
  echo "k8s-check: every manifest renders and matches its schema"
}

# A CRD's schema, closed to unknown fields the way the API server's strict mode is.
from_crd() {
  local raw=https://raw.githubusercontent.com/$1
  curl -sfL "$raw" | yq -o=json | jq -c '
    def close: if type == "object" then
        (if has("properties") and (has("additionalProperties") | not)
            and .["x-kubernetes-preserve-unknown-fields"] != true
         then .additionalProperties = false else . end)
        | with_entries(.value |= close)
      elif type == "array" then map(close) else . end;
    . as $crd | .spec.versions[] | select(.served) |
    { path: "\($crd.spec.group)/\($crd.spec.names.kind | ascii_downcase)_\(.name).json",
      schema: (.schema.openAPIV3Schema
        | .properties |= with_entries(if .key == "metadata" then . else .value |= close end)
        | .additionalProperties = false
        | .properties.metadata = { type: "object" }) }' |
    while read -r doc; do
      mkdir -p "$schemas/$(dirname "$(jq -r .path <<<"$doc")")"
      jq -cS '.schema' <<<"$doc" >"$schemas/$(jq -r .path <<<"$doc")"
    done
}

cmd_schemas() {
  rm -rf "$schemas"
  from_crd "argoproj/argo-cd/$ARGOCD_VERSION/manifests/crds/application-crd.yaml"
  from_crd "isindir/sops-secrets-operator/$SOPS_OPERATOR_VERSION/config/crd/bases/isindir.github.com_sopssecrets.yaml"
  from_crd "cloudnative-pg/cloudnative-pg/$CNPG_VERSION/config/crd/bases/postgresql.cnpg.io_clusters.yaml"
  from_crd "cloudnative-pg/cloudnative-pg/$CNPG_VERSION/config/crd/bases/postgresql.cnpg.io_scheduledbackups.yaml"
  from_crd "cloudnative-pg/cloudnative-pg/$CNPG_VERSION/config/crd/bases/postgresql.cnpg.io_databases.yaml"
  from_crd "cilium/cilium/v$CILIUM_VERSION/pkg/k8s/apis/cilium.io/client/crds/v2/ciliumloadbalancerippools.yaml"
  from_crd "cilium/cilium/v$CILIUM_VERSION/pkg/k8s/apis/cilium.io/client/crds/v2alpha1/ciliuml2announcementpolicies.yaml"
  from_crd "cert-manager/cert-manager/$CERT_MANAGER_VERSION/deploy/crds/cert-manager.io_clusterissuers.yaml"
  from_crd "cert-manager/cert-manager/$CERT_MANAGER_VERSION/deploy/crds/cert-manager.io_issuers.yaml"
  from_crd "cert-manager/cert-manager/$CERT_MANAGER_VERSION/deploy/crds/cert-manager.io_certificates.yaml"
  from_crd "cloudnative-pg/plugin-barman-cloud/$BARMAN_PLUGIN_VERSION/config/crd/bases/barmancloud.cnpg.io_objectstores.yaml"
}

case ${1:-check} in
  check | schemas) "cmd_${1:-check}" ;;
  *) sed -n '2,8s/^# \{0,1\}//p' "$0" >&2; exit 2 ;;
esac
