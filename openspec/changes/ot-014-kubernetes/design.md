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

**Result (2026-11-25): holds, with two changes to the plan.**

- Values match Git byte for byte (compared by hash), awkward ones included. A
  changed secret was served 56 s after the push. With only its own key, the
  cluster can't read the Talos secrets or any `prod` file.
- A dump of ArgoCD's Redis contains the `SopsSecret` and its ciphertext (the
  controls) and no plaintext or base64 of it. The repo-server has no key; a
  search of its files found no plaintext, but also no ciphertext, so that search
  alone proves nothing.
- Undecryptable secret plus new image, five runs: the old pod served throughout,
  the new image never started.
- **Changed: secret names carry a version (D3).** With the operator unreachable,
  its stale "Healthy" let the new image start with the old secret. Repeated with
  a versioned name, the workload waited, stayed blocked when the operator came
  back and couldn't decrypt, and recovered 29 s after the revert.
- **Changed: `retry.refresh` (D10).** With unlimited retries alone, a failing sync
  retried its own revision and ignored the revert (7 minutes, until terminated by
  hand). With it, reverts recovered in 41–46 s.
- About 10,000 requests across all runs, none failed.

### Slice 1 drills (2026-11-25)

With the canary workload on the local cluster and a request loop running: about
14,000 requests in these drills, none failed. Times are from the push.

| Drill | Outcome | Time | Limit |
| --- | --- | --- | --- |
| Deploy a new image | running, one pod | 115 s | 10 min |
| Roll back (revert) | previous image running | 89 s | 10 min |
| Drift: workload replicas | reverted | 2 s | 15 min |
| Drift: ArgoCD's own setting | reverted | 18 s | 15 min |
| Drift: the root Application | reverted | 70 s | 15 min |
| Image that doesn't exist | old pod serves; `k8s-status` names it; Degraded after the 300 s deadline | revert 41–74 s | |
| Missing secret | old pod serves; status names the secret | revert 112 s | |
| Undecryptable secret | see Result | revert 29–46 s | |
| Manifest the API rejects | old state serves; status names the resource | seen at 32 s, revert 92 s | |
| Overlay that doesn't render | old state serves; status names the file | 192 s (was 331 s before the shorter grace period) | |
| Repository unreachable | workload serves, and restarts; status says so | clean 6 s after it returned | |

Not yet tested: a machine restart (slice 2).

Rebuild (1.10, 2026-11-25): `k8s-down`, then `k8s-up` by the operator; seven
Applications Synced and Healthy, the secret decrypted and `k8s-status` clean within
5 minutes of the node registering. CoreDNS and the ArgoCD pods waited for the
network and the Redis password, as expected, and nothing needed a hand.

### Slice 2 so far (2026-11-26)

- The edge: Traefik's LoadBalancer Service gets `10.5.0.100` from Cilium's LB-IPAM,
  announced over L2 on the VM bridge; `k8s-up` maps `todo.k8s.test` to it in
  `/etc/hosts`. Cilium only applies a changed setting on restart, so its pods now
  roll on config changes (`rollOutCiliumPods`); before that, L2 announcements were
  on in the ConfigMap and off in the agent.
- TLS: a local CA whose key is a `local` SopsSecret, so it survives rebuilds and is
  trusted once; it is name-constrained to `k8s.test`. cert-manager issues the
  edge's certificate. TLS 1.3 negotiates `X25519MLKEM768`; `just smoke` passes
  (28 checks), Chromium trusting the CA through the NSS store, without
  `ignoreHTTPSErrors`.
- First boot with development mode off found, in order: five settings the overlays
  lacked (Argon2 cost, token lifetimes; the example overlay too), the app owner
  unable to create `pg_stat_statements` (now a CNPG `Database` with that extension,
  in the base), and a sender name with parentheses, which the backend can't parse.
  Each was named by `k8s-status` and stopped by the 300 s progress deadline, with
  web serving throughout.
- Email: a Mailtrap sandbox was replaced by Ethereal (Mailtrap allows 50 a month);
  `backend-v1` to `backend-v2`, the old one pruned.



- An overlay can't be emptied: Kustomize refuses to render one, and ArgoCD refuses
  to prune an Application to nothing. The canary's removal left the app's namespace.
- Secrets get versioned names (D3), and ArgoCD needs `retry.refresh` (D10).
- Wait for `.status.sync.revisions` to name the commit before judging a result.
- Traefik reads a client's trace context before any middleware, so D6's test comes
  first in slice 2, and the edge isn't chosen until it has run.
- The VM takes 12 GiB, and the host had about 15 GiB available with it up.
  Measure before slice 5.

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
- `secrets-check` fails if values change and the name doesn't (agreed
  2026-11-25).
- The `SopsSecret` and its Secret share a versioned name (`canary-v2`), which
  workloads refer to. Any change to the values bumps it: a new name has no status
  until the operator decrypts it, and a pod can't start without its Secret.
  `PruneLast` keeps the previous version until the new one is in use.
- Reloader restarts pods if a Secret is changed in place all the same.
- The age key is mounted only in the operator.

Accepted limits: the operator doesn't verify SOPS's file-level MAC, so removing
or replacing a field isn't detected by decryption (Git review is the control);
anyone who can create a `SopsSecret` in a namespace and read its Secrets can
decrypt with it, here only the operator of the cluster; and its ClusterRole can
manage all Secrets. Its status has no observed generation, so an old "Healthy"
is read for a new revision while the operator is down (tested).

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

**Result (2026-11-26): fails.** Traefik v3.8.0 (chart 43.0.0) in Docker, OTLP to
a collector, a header echo behind it, and a request carrying a forged
`traceparent` and `tracestate`:

| Setup | Forged ID reaches the backend | Edge's span and access-log trace ID |
| --- | --- | --- |
| Default | yes, `tracestate` too | the forged one |
| Headers middleware deleting both | yes: re-injected after the middleware | the forged one |
| Entry-point tracing off, plus that middleware | no | none: the edge neither traces nor logs an ID |

Without a client header each request got a new trace. Traefik 3.8 has no option
to ignore incoming context (`--help`, 2026-11-26).

**Decided (2026-11-26): Traefik as it is, risk accepted by the operator.** On the
cluster a client can choose a request's trace ID, which the backend adopts and
returns in `X-Trace-Id`, so the observability scenario "Client-supplied trace
context is ignored" fails there, and only there: compose's Caddy still strips it.
A client could join its requests to a trace it knows the ID of, or make traces
collide; it gains no access. Slice 5 checks the rest of that spec, and 8.2 adds
this to the threat model's edge assumptions.

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
and `ServerSideApply`, and a `retry` with backoff, no limit and
`refresh: true`, so a failure that clears doesn't need a hand and a revert is
picked up. `selfHeal` reverts manual changes.
ArgoCD hides repository errors, a commit that doesn't
render included, for a grace period; it is cut from 180 s to 30.
Deployments set `progressDeadlineSeconds: 300`, so a stuck rollout turns the
Application Degraded.

**D11. What ArgoCD doesn't manage.** Only the Talos machine config, the
cluster's age key, and the first application of ArgoCD and Cilium. ArgoCD
is its Helm chart at a pinned version with committed values, applied once by `k8s-up`, and then managed by an
Application of their own; a root Application holds the rest, itself included. There is no Git
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
them. CNPG, cert-manager and Traefik turned out to run under `restricted`
unchanged (2026-11-26), so they aren't exceptions. _Alternative:_ Kyverno, more capable but unneeded for this.

**D13. Manifest checks.** `kubeconform` v0.8.0 in strict mode, with custom
resource schemas generated from the pinned definitions and committed, so CNPG,
Traefik, cert-manager and ArgoCD resources are checked, not skipped. A
`just k8s-check` recipe joins `ci-checks`; a `k8s` CI job runs it and joins the
`images` job's `needs`.

**D14. Pins.** CLIs in `.mise.toml`; everything else in `deploy/k8s/versions.env`
or its Application. Starting points, the latest releases on 2026-11-25: Talos
1.14.7, Cilium 1.20.5, ArgoCD 3.6.3 (chart 10.20.0), sops-secrets-operator 0.22.5, Reloader chart 2.2.21, cert-manager 1.21.3, Traefik chart 43.0.0,
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
