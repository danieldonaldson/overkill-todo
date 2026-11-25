# Design

## Context

See proposal.md for motivation. Observed on 2026-11-25:

- `deploy/k8s/base` has backend, web, an Ingress and a CNPG cluster with
  barman-cloud backups. `overlays/example` holds placeholders. Nothing in the
  gate renders them.
- Secrets are `deploy/secrets/<level>/<name>.sops[.env]`, encrypted to
  post-quantum age keys only; `secrets.sh` rejects others. A `local` level
  exists and machines can be assigned to it.
- With `DEV_MODE=false` the backend requires an OTLP endpoint, Turnstile, MX
  validation and SMTP configuration. A collector that is down doesn't block startup
  (observability spec).
- SMTP always uses TLS: implicit on 465, STARTTLS otherwise.
- The collector reads Docker's json-file logs, which a cluster doesn't have.
- The repository is public, and CI publishes images to GHCR from `main` only.
  Both packages pull anonymously (checked 2026-11-25).
- Kustomize refuses a `secretGenerator` file outside its own directory, but can
  use a directory elsewhere as a base.
- CI doesn't run `just ci`; it has its own jobs.
- The host has 12 cores, about 22 GiB of available memory, 164 GiB of free
  disk, KVM and QEMU. `kubectl` and `helm` are installed; `talosctl` has no
  version set, and there is no `argocd` CLI or virsh.
- Patterns to reuse: `~/Work/dev-workflows/k8s/` and its
  `talos-cluster-bringup.md` and `dr-rehearsal.md` runbooks.

There are no domain, application or schema changes: everything is in `deploy/`,
`scripts/`, the justfile, CI and docs.

## Goals / Non-Goals

**Goals:** learn by running the pieces a real cluster has; one source of config
shared with compose where practical.

**Non-Goals:** high availability; hiding Kubernetes behind an abstraction;
parity of every compose convenience.

## Hypothesis

We believe the SOPS operator can decrypt, in the cluster, secrets encrypted to a
post-quantum age key, and that ArgoCD can hold back a workload until it has.

Slice 1 tests it with throwaway secrets. It holds if, from commits alone:

- the Secret appears with exactly the right values, including `$`, `#`, quotes,
  `=` and multiple lines;
- changing one restarts the pod that uses it, with the new value;
- the cluster's key fails on a `prod` file and on the Talos secrets;
- an undecryptable secret committed together with a new image leaves the old pod
  serving and the new one not started, and the status names the secret;
- ArgoCD's Redis and repo-server hold no plaintext;
- `just secrets-check` passes.

Checked in source on 2026-11-25: sops-secrets-operator 0.22.5 embeds SOPS 3.13.3
and age 1.3.2, and SOPS parses hybrid (`age1pq1`, `AGE-SECRET-KEY-PQ-1`) keys from
3.12.0. Nothing has been run.

**Result:** pending slice 1.

## Decisions

**D1. One Talos VM under QEMU.** One control-plane node that also runs
workloads; about 4 vCPU, 12 GiB of memory and 40 GiB of disk. Immutable and
API-only, and it matches the runbook. _Alternatives:_ kind or k3d (containers,
not a VM); k3s on Debian (brings its own CNI, ingress and storage).

**D2. Cilium.** CNI and network policy, replacing kube-proxy, from the existing
install script's values without its gVisor setting, kept in one file used by both
the bootstrap and the Application. `k8s-up` applies `helm template` output
server-side, under the release name and namespace ArgoCD uses, so no unmanaged
Helm release is left behind. Hubble's certificates use `method: cronJob`: the
default, `helm`, generates a CA while rendering and reuses it through a cluster
lookup ArgoCD's rendering doesn't have, so every render would differ and roll
pods. _Alternative:_ Flannel, which doesn't enforce NetworkPolicy.

**D3. ArgoCD, with secrets decrypted by an operator.** ArgoCD v3.6.3, full
install without Dex and notifications, chosen by the operator of this project,
who wants to practise it. sops-secrets-operator 0.22.5 decrypts; ArgoCD only
handles ciphertext, which is the style its docs recommend.

- Each cluster secret is a `SopsSecret` at
  `deploy/secrets/local/<name>.sops.yaml`, with an explicit namespace. Only
  `data` and `stringData` are encrypted, so names stay reviewable. No `local`
  secrets exist yet, so this is their only copy; compose's local values are
  unencrypted defaults and stay as they are.
- `secrets.sh` learns that one extra form in its path checks, `store`,
  `file_recipients`, `check`, `sync` and `new`. `.sops.yaml` gets a rule per level
  for it, before the level's general rule (first match wins), that sets
  `encrypted_regex`. `decrypt` writes compose's files and skips it. The check also
  rejects one that isn't a `SopsSecret` or has an unencrypted value.
- The Talos secrets are an opaque `talos.sops`, so the source's include never
  hands ArgoCD or the operator a file the cluster's key can't read (D4).
- The workloads Application has two sources: that directory, including only
  `*.sops.yaml`, and the kustomize overlay. Kustomize can't read files outside
  its directory, and one Application lets waves order both.
- `SopsSecret`s sync in an earlier wave, with a custom health check that is
  healthy only when the operator says so. Waves across separate Applications
  wouldn't hold back an ordinary commit, since each syncs on its own.
- Reloader restarts pods when a Secret they use changes.
- The age key is mounted only in the operator.

Accepted limits: the operator doesn't verify SOPS's file-level MAC, so removing
or replacing a field isn't detected by decryption (Git review is the control);
anyone who can create a `SopsSecret` in a namespace and read its Secrets can
decrypt with it, here only the operator of the cluster; and its ClusterRole can
manage all Secrets. Its status has no observed generation, so an old "Healthy"
could briefly be read for a new revision: slice 1 tests that. If it happens, use
versioned Secret names, so a pod can't start until its version exists.

_Alternatives:_ decrypting while ArgoCD renders, by a plugin or KSOPS (puts
plaintext in ArgoCD's Redis and the key in its repo-server; KSOPS 4.5.1 does
support these keys); Flux (decrypts SOPS itself, but not the tool to be learned);
projectcapsule's sops-operator 0.11.0 (bundles its own age code, whose recipient
parsing looks X25519-only). _Fallback:_ a plugin sidecar running our SOPS.
_Rejected:_ Sealed Secrets: RSA wrapping breaks the security spec's
post-quantum-only requirement for committed secrets.

**D4. The cluster is a `local` machine.** Its age key is generated on the host,
added under `machines:` with `env: local`, and placed in the cluster at
bring-up. It can't read `prod`, and we don't grant it any. `secrets.sh` always
adds an environment's machines as readers (its `only:` narrows people alone),
so slice 1 adds a per-secret `machines: false` exception, used for the Talos
secrets: the cluster must not read the keys to its own control plane.
Observability and analytics get their own `local` secrets (Grafana, Umami, its
database). _Alternative:_ `also:` on the `prod` files, which would let a local
machine read production secrets.

**D5. Development mode off.** Turnstile uses Cloudflare's published test keys;
SMTP uses a sandbox account (agreed 2026-11-25), since a mail catcher would need a publicly trusted
certificate. Both are `local` secrets. The OTLP endpoint names the collector
that arrives in slice 5. _Alternative:_ `DEV_MODE=true`, which would skip the
startup checks this is meant to exercise.

**D6. Traefik edge, private CA.** Traefik matches the intended production edge;
cert-manager issues from a local CA. Plain HTTP redirects to HTTPS, which the
smoke suite requires. Smoke trusts the CA through `NODE_EXTRA_CA_CERTS` and the
browser's trust store. If Chromium ignores that store, add an env-gated
`ignoreHTTPSErrors`, off by default. Slice 4 implements the compose
README's edge contract.

The edge must start each trace, ignore a client's `traceparent` and log the trace
ID; the backend trusts what the edge sends. Traefik's entry point extracts
incoming trace context before any middleware (read in its source, 2026-11-25),
so stripping the header in a middleware looks too late, and turning propagation
off would also stop the backend joining the trace. Slice 2 tests this before
building on Traefik. If it fails, as we expect, the operator chooses between
Caddy with the existing rules as the edge, and a small header-stripping hop in
front of Traefik. Either way it is settled before slices 3–5.

**D7. Backups go to S3-compatible storage on the host.** Garage in a container
with its own volume, reached over the VM bridge, with the barman-cloud plugin
installed in the cluster. Slice 3 first proves Garage with barman-cloud,
path-style addressing included; its compatibility is unverified. A normal
rebuild uses `bootstrap.recovery`; `initdb` is a separate, explicit first-run
overlay, so nothing silently starts empty. _Alternatives:_ in-cluster storage
(destroyed by the wipe); MinIO (repository archived).

**D8. Local-path storage.** Enough for one node, and managed by ArgoCD.
Talos's root is read-only, so its path moves from `/opt` to under `/var`, and its
namespace is `privileged`, since its helper pods mount host paths. Data is as
durable as the VM disk, hence D7.

**D9. `overlays/local`; `example` stays a template.** Observability and analytics
are components reading the existing files in `deploy/observability/` and
`deploy/analytics/`, with two exceptions: a pod-log receiver for the collector
(redaction shared), and `alertmanager.cluster.yml`, which emails the operator
through the sandbox account. Compose's local stack still sends no email.
Umami gets its own CNPG cluster with no plugin and no ScheduledBackup, plus a
retention CronJob.

**D10. Branch, tracking and timing.** Work stays on `ot-014-kubernetes`; other
work continues on `main`. Nothing goes back to `main` until the operator asks.
Meanwhile `main` is merged into the branch at least at each slice boundary
(merged, not rebased, once the branch is pushed, so ArgoCD's tracked branch is
never force-pushed). `targetRevision` is the branch, switched to `main` when the
work is merged. Secrets policy changes are made on the branch only, since
`.sops.yaml` is generated. CI doesn't run on branch pushes, so it is started
with `workflow_dispatch`; the `images` job runs only on `main`, so its new
dependency is checked after the merge. ArgoCD reads GitHub, so a commit has to
be pushed to deploy. App images come from `main`, and a newer digest brings its
migrations with it: merge `main` first so manifests and image match.
Polling is every 60 s with no jitter (`timeout.reconciliation`; the
default is 2 minutes plus up to 1). Automated sync with `prune`, `selfHeal`,
and `ServerSideApply`, and a `retry` with backoff and no limit, so a
failure that clears doesn't need a hand. `selfHeal` reverts manual changes.
Deployments set `progressDeadlineSeconds: 300`, so a stuck rollout turns the
Application Degraded.

**D11. What ArgoCD doesn't manage.** Only the Talos machine config, the
cluster's age key, and the first application of ArgoCD and Cilium. ArgoCD's
manifests are committed, applied once by `k8s-up`, and then managed by an
Application of their own; a root Application holds the rest. There is no Git
credential: the repository is public. Order is by sync wave: controllers, their
config, then workloads. ArgoCD no longer assesses Application health by default,
so we restore that check for waves to wait, and custom resources use
`SkipDryRunOnMissingResource`. The UI is reached by port-forward only.

**D12. Admission and limits.** Pod Security Admission, built in, set to
`restricted` on the app
namespace, which also holds its CNPG cluster (the backend reads the Secret CNPG
generates, and a pod can't read another namespace's), and on the analytics
namespace, with its own. Namespaces are committed manifests carrying these
labels, in the first wave, rather than created by ArgoCD. Each base sets its own
namespace (`deploy/k8s/base` sets none today); the overlays stop transforming it.
Observability is
`restricted` except the log collector, which reads host paths and gets its own
`privileged` namespace. Cilium, ArgoCD, the SOPS operator, Reloader, storage, Traefik, cert-manager and CNPG
are listed exceptions with reasons, and have limits where their charts set
them. _Alternative:_ Kyverno, more capable but unneeded for this.

**D13. Manifest checks.** `kubeconform` v0.8.0 in strict mode, with custom
resource schemas generated from the pinned definitions and committed, so CNPG,
Traefik, cert-manager and ArgoCD resources are checked, not skipped. A
`just k8s-check` recipe joins `ci-checks`; a `k8s` CI job runs it and joins the
`images` job's `needs`.

**D14. Pins.** CLIs in `.mise.toml`; everything else in `deploy/k8s/versions.env`
or its Application. Starting points, the latest releases on 2026-11-25: Talos
1.14.7, Cilium 1.20.5, ArgoCD 3.6.3, sops-secrets-operator 0.22.5, Reloader chart 2.2.21, cert-manager 1.21.3, Traefik chart 43.0.0,
CNPG 1.30.2, barman-cloud plugin 0.16.0. Kubernetes
is 1.36.6: Talos 1.14 supports up to 1.37, but Cilium 1.20, cert-manager 1.21 and
CNPG 1.30 list 1.36 as their newest tested. Garage is pinned in slice 3.

### Slice plan

Only slice 1 has tasks until it ships.

| Slice | Delivers | Tests first |
| --- | --- | --- |
| 1 | Cluster, GitOps, secrets | Hypothesis above |
| 2 | App, database and TLS; sign-in, smoke, rollout, database and VM failures | Traefik's trace context (D6), then the smoke suite |
| 3 | Backups, destructive restore drill; destroying the cluster leaves backups alone | Garage with barman-cloud |
| 4 | Isolation, admission, full edge contract | Client address through the edge |
| 5 | Observability and the backup alert | Pod log parsing and redaction |
| 6 | Analytics; account backups hold no analytics data | Real client address |
| 7 | Rebuild all from nothing | Total recovery time |

Slices 4 and 5 supersede OT-001's tasks 4.1–4.4. The backup alert belongs to
the deployment spec.

## Risks / Trade-offs

- [Post-quantum decryption fails in the cluster] → test it before building on
  it; D3 fallback.
- [Operator abandoned or drops support] → SOPS files stay readable by `sops`; D3
  fallback.
- [Stale operator status lets a workload through] → tested in slice 1; versioned
  names (D3).
- [Losing the computer loses the cluster and its backups] → accepted; the drill
  covers VM loss only, and the docs say so.
- [Two deploy paths drift] → shared config, both overlays checked in the gate.
- [VM memory too tight with observability] → measure before slice 5; components
  are optional.
- [A broken secret blocks the one workloads Application] → the running version keeps
  serving, and status names it.
- [VM networking needs root] → scripts print what needs `sudo`.
- [Single node] → restarts are outages; accepted.

## Migration Plan

No data or schema migration. Workloads roll back by reverting their commit.
Policy changes need more: to remove the cluster's key, delete it from
`access.yaml`, run `just secrets-sync`, commit all three parts together, and
rotate the values it could read. Reverting the policy commit alone leaves files
encrypted to a removed key, and `just secrets-check` fails. Destroying the
cluster removes everything else. Compose is unaffected.

## Open Questions

- Whether the production target becomes Kubernetes: separate, after slice 7.
