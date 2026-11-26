# Kubernetes

A local single-node cluster that ArgoCD fills from Git (OT-014). It sits beside
`../compose/`, which stays the way to run the app locally and has its own
production override. Which one production uses is not decided.

> **Status:** slice 2 done. The app runs at https://todo.k8s.test with
> development mode off. Backups arrive in slice 3; until then the database lives
> only on the VM's disk.

```
cluster/apps/      ArgoCD Applications; root.yaml owns them all, itself included
cluster/<name>/    chart values and manifests each Application uses
overlays/local/    what the cluster runs (the workloads Application)
overlays/example/  template for another environment
base/              backend, web, database and Ingress
schemas/           generated, for `just k8s-check`
versions.env       what runs in the cluster; CLIs are pinned in /.mise.toml
```

## Bring-up

Needs KVM, QEMU, 14 GiB of available memory, 45 GiB of disk and `mise install`.
Two keys: your own age key, cleared for `local` in
[`../secrets/access.yaml`](../secrets/access.yaml), and the cluster's, at
`~/.config/overkill-todo/cluster.agekey` (or `$OVERKILL_CLUSTER_AGE_KEY`). Keep a
copy of the cluster's key; if it is lost, see [Replacing the key](#replacing-the-clusters-key).

```bash
just k8s-up       # prints each sudo command it runs; about 5 minutes
just k8s-status   # what is out of sync, failing or stuck
just k8s-down     # destroys the VM, its network and firewall rules
```

`k8s-up` refuses, changing nothing, if something is missing or a cluster (or part
of one) exists. `k8s-down` cleans up after an interrupted `k8s-up`. Neither
touches the compose stacks. A host reboot stops the VM: run both again.

For `kubectl` and `talosctl` (without this, `kubectl` tries `localhost:8080`,
which is the compose edge):

```bash
eval "$(./scripts/k8s.sh env)"
```

The ArgoCD UI, user `admin`:

```bash
kubectl -n argocd port-forward svc/argocd-server 8443:443   # https://localhost:8443
kubectl -n argocd get secret argocd-initial-admin-secret -o jsonpath='{.data.password}' | base64 -d
```

## The app

`k8s-up` adds `todo.k8s.test` to `/etc/hosts` (`./scripts/k8s.sh hosts` does it
alone). Trust the local CA once, in the NSS store that Chromium-based browsers and
Playwright use (Firefox has its own):

```bash
certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n "overkill-todo local CA" -i deploy/k8s/local-ca.crt
NODE_EXTRA_CA_CERTS=$PWD/deploy/k8s/local-ca.crt just smoke https://todo.k8s.test
```

The CA can only sign names under `k8s.test`. Email goes to an Ethereal sandbox:
codes appear in its Messages tab, never in a real inbox.

## What is not in Git

`k8s-up` does by hand only what ArgoCD can't: the Talos machine config (from
`../secrets/local/talos.sops`), the cluster's age key, and the first application
of Cilium and ArgoCD, which ArgoCD then manages. Everything else is a commit. The
cluster holds no Git credential, and CI holds no cluster credential.

## Deploying and rolling back

Deploy by committing a digest in `overlays/local/kustomization.yaml` and pushing
to the tracked branch (`targetRevision` in `cluster/apps/`). Roll back with
`git revert`. ArgoCD polls every minute; expect one to two minutes. A version that
doesn't start leaves the previous one serving, and `just k8s-status` names it.
Manual changes are reverted.

`just k8s-check` (in `just ci` and CI) renders everything and checks it against
its schema. After changing a version in `versions.env`, change the matching
Application too, and run `./scripts/k8s-check.sh schemas` if it has CRDs.

## Secrets

Each is a `SopsSecret` in `../secrets/local/<name>.sops.yaml`, decrypted in the
cluster by the SOPS operator. ArgoCD only ever sees ciphertext. To change one:

1. `sops edit` it: change the value and give both names the next version
   (`backend-v1` → `backend-v2`).
2. Point the workloads at the new name, and commit both together.

The version matters: while the operator is down its last status still says
"Healthy", so a reused name could let a new version start with old values.
`just secrets-check` enforces it.

### Replacing the cluster's key

1. `age-keygen -pq -o ~/.config/overkill-todo/cluster.agekey`
2. Put its public key under `machines:` in `access.yaml`, `just secrets-sync`.
3. Commit the policy, `.sops.yaml` and the re-keyed secrets together. The files'
   data keys are rotated, so every `SopsSecret` needs its next version.
4. If the old key may have leaked, rotate the values: it still reads git history.
5. `just k8s-down && just k8s-up`.

Reverting only the policy commit leaves files encrypted to a removed key, and
`just secrets-check` fails.

## More

- Design and measurements: `openspec/changes/ot-014-kubernetes/design.md`
- Web analytics (Umami, OT-005) is compose-only until slice 6; its edge must meet
  the [edge contract](../compose/README.md#edge-contract).
- The same stack in the cloud: `~/Work/dev-workflows/runbooks/`
