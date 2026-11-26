# Tasks

"Deployed" here means running on the local cluster; there is no production.
Work stays on the branch until the operator asks for the merge (D10).
Slices 3–7 are coarse: revise each one's tasks at the boundary before it.

## 1. Slice 1 (first slice): commits drive a fresh cluster, secrets included

Delivers these scenarios: Creation is interrupted; Not enough resources; Local
development is unaffected; Manual change; Invalid manifest; Repository
unreachable; Cluster cannot write to the repository; Cluster reads only its own
environment; Missing or undecryptable secret; Decrypted secrets are not exposed by the deploy tooling; Secret changes.
With a throwaway workload it also exercises Create from nothing, Deploy, Roll
back and Broken image, which slice 2 completes for the app.

- [x] 1.1 Pin `talosctl`, `kubectl`, `helm`, `argocd`, `sops` (at least 3.13, no longer `latest`) and `kubeconform` in `.mise.toml`, and the rest in `deploy/k8s/versions.env` (D14); check the versions support each other. Verify: each CLI prints its pin, and the pinned operator's `go.mod` has SOPS ≥ 3.13 and age ≥ 1.3.
- [x] 1.2 Add `just k8s-up` / `k8s-down`: resource preflight, Talos VM, per-cluster kubeconfig, Cilium (D2). Verify: the node is Ready; interrupting, then down and up, works; too little memory or disk stops before any change; `just dev` runs alongside, and its data survives `k8s-down`.
- [x] 1.3 Extend `scripts/secrets.sh` with `machines: false` and the `<name>.sops.yaml` form (D3), and document both. Verify: `just secrets-check` accepts one, and fails on a plaintext value, on a classical recipient and on a kind other than `SopsSecret`; `secrets-new`, `secrets-sync` and `secrets-decrypt` behave as in D3.
- [x] 1.4 Add the cluster's key to the policy; encrypt the Talos secrets and the throwaway `local` secrets. Verify: `just secrets-check` passes; the cluster's key fails on a `prod` file and on the Talos secrets.
- [x] 1.5 Commit ArgoCD's manifests and NetworkPolicies, the root, self and two-source workloads Applications (D3, D10, D11), the SOPS operator, Reloader, local-path storage and the Cilium Application, applied once by `k8s-up`. Verify: every Application is Synced and Healthy, ArgoCD manages itself, Cilium is adopted without restarting, a test volume binds, no Secret holds a Git credential, the age key is mounted only in the SOPS operator, a second render of Cilium shows no diff, and a pod outside ArgoCD can't reach its Redis or repo-server.
- [x] 1.6 Retire the Hypothesis with the throwaway secrets and a workload using them. Verify: every condition in design.md holds, recorded there; if the stale-status case fails, use versioned names, and otherwise apply the D3 fallback, before going on.
- [x] 1.7 Add `just k8s-check` with committed schemas for the custom resources (D13), in `ci-checks` and a `k8s` CI job the `images` job needs. Verify: an invalid field in a Deployment and in an ArgoCD Application each fail, naming the resource, and no keys are needed.
- [x] 1.8 Add `just k8s-status`: failed syncs, plus rollout and pod state. Verify: it names the resource for a broken commit, and the stuck workload for an image that never becomes ready.
- [x] 1.9 Exercise deploy, rollback, drift on a workload and on ArgoCD itself, broken image, missing and undecryptable secret, and repository outage, with the throwaway workload. Verify: outcomes and timings are recorded in design.md and within the spec's limits.
- [x] 1.10 Rewrite `deploy/k8s/README.md` (bring-up, deploy, rollback, bootstrap boundary, key removal) and remove Sealed Secrets guidance from it, `.mise.toml`, `backend/.env.example` and `overlays/example/` (both files). Verify: following the README on a destroyed cluster converges with no other manual steps, and nothing recommends Sealed Secrets.
- [x] 1.11 `just ci` green. Revise design.md with what slice 1 showed, remove the throwaway workload and secrets, and add tasks for slices 2–7. Slice 2 starts with the D6 edge test and moves the base's namespace into it (D12), and removes the base's barman-cloud plugin and ScheduledBackup from the local overlay, not just its ObjectStore, until slice 3. Verify: `openspec validate ot-014-kubernetes --strict` passes.

## 2. Slice 2: the app over TLS

Delivers: Create from nothing; Deploy; Roll back; Broken image; Member signs in;
Smoke checks pass; Database unavailable; Machine restarts.

- [x] 2.1 Test Traefik's trace context at the edge before building on it (D6): a client's `traceparent` must not reach the backend, and the edge's trace must. Verify: the result is recorded in design.md. If it fails, the operator chooses between Caddy and a header-stripping hop, recorded in D6, before 2.4.
- [x] 2.2 Commit the CNPG operator, cert-manager and the edge's chart as Applications, pinned and with schemas (D13), and Postgres running from `overlays/local` with the base's barman-cloud plugin and ScheduledBackup removed (until slice 3). Move the base's namespace into it and drop the overlay's `namespace.yaml` copy (D12). Verify: `just k8s-check` passes with no skipped resource, the database is ready with its data on local-path, and `overlays/example` still renders.
- [x] 2.3 Add the `local` backend secrets: keys, Turnstile test keys, sandbox SMTP (D5); the operator supplies the SMTP credentials. Verify: `just secrets-check` passes, and the backend starts with `DEV_MODE=false`.
- [x] 2.4 Run backend and web from pinned `main` digests behind Traefik (D6), TLS from a local CA, HTTP redirected. Verify: `just smoke` passes trusting the CA, post-quantum check included.
- [x] 2.5 Sign in as a Member over TLS with an emailed code. Verify: it works end to end in a real browser.
- [x] 2.6 Drills with the app: deploy, rollback, broken image, database stopped and returned, VM restart (measured, data intact). Verify: outcomes and timings are in design.md and within the spec's limits.
- [ ] 2.7 Merge `main` (D10), `just ci` green, README status updated, design.md revised, slice 3's tasks checked. Verify: strict validation passes.

## 3. Slice 3: backups and the restore drill

Delivers: Destroying the cluster touches nothing else; Restore into a rebuilt
cluster; Rebuild never silently starts empty; Backup storage unavailable.

- [ ] 3.1 Prove Garage with barman-cloud, path-style addressing included, and pin it (D7). Verify: a base backup and WAL arrive, recorded in design.md; otherwise revise D7.
- [ ] 3.2 Run Garage on the host with its own volume, reached over the VM bridge; its credentials are `local` secrets. Verify: `k8s-down` leaves its data.
- [ ] 3.3 Add the barman-cloud plugin and the base's backup to the local overlay, with a separate explicit first-run `initdb` overlay and `bootstrap.recovery` for a rebuild. Verify: with no usable backup the database doesn't start.
- [ ] 3.4 Drill: write a row, confirm its backup, destroy and rebuild, restore. Verify: the row is there; duration and newest data recovered are recorded.
- [ ] 3.5 Drill: storage stopped while the app runs. Verify: the app serves, the failure is visible, and backups resume. Boundary work as in 2.7.

## 4. Slice 4: isolation, admission and the edge contract

Delivers: Unneeded connection is refused; Only the edge reaches the backend;
Operator tools stay private; Privileged workload is rejected; Workload exceeds
its memory limit.

- [ ] 4.1 Test the client address through the edge. Verify: recorded in design.md.
- [ ] 4.2 NetworkPolicies, default deny, for every app, database and tool. Verify: each allowed path works, and the rest is refused, tested both ways.
- [ ] 4.3 Pod Security on every namespace, memory limits, exceptions listed with reasons (D12). Verify: a privileged pod is rejected, and a memory hog restarts alone.
- [ ] 4.4 The compose README's edge contract on the cluster's edge. Verify: every item is checked. Boundary work as in 2.7.

## 5. Slice 5: observability

Delivers: Backups stop succeeding, and the observability spec on the cluster.

- [ ] 5.1 Measure memory with slices 1–4 running and decide what fits (D12, Risks). Test pod-log parsing and redaction first. Verify: recorded in design.md.
- [ ] 5.2 Run the stack as a component from `deploy/observability/`, with the pod-log receiver and `alertmanager.cluster.yml` (D9). Verify: traces, logs and metrics arrive, no secret appears in a log, and the observability spec holds except the client trace context accepted in D6.
- [ ] 5.3 Alert when no backup has succeeded for 26 hours. Verify: it fires and emails. Boundary work as in 2.7. Supersedes OT-001 4.1–4.4 with slice 4.

## 6. Slice 6: analytics (cut first)

- [ ] 6.1 Umami with its own CNPG cluster, no plugin or ScheduledBackup, and a retention CronJob (D9). Verify: the real client address arrives through the edge, retention runs, and account backups hold no analytics data. Boundary work as in 2.7.

## 7. Slice 7: rebuild from nothing

- [ ] 7.1 Destroy everything but the backups and the keys, rebuild, restore. Verify: every scenario holds, and total recovery time is recorded in design.md.

## 8. Feature done

- [ ] 8.1 When the operator asks, merge the branch to `main` and switch ArgoCD to it (D10). Then check every scenario on a cluster rebuilt from nothing, with ArgoCD tracking `main`. Verify: results recorded in design.md.
- [ ] 8.2 Update README, `deploy/k8s/README.md`, `deploy/compose/README.md` and the threat model's edge assumptions. Verify: docs match the final state and `just ci` is green.
- [ ] 8.3 Archive the change. Verify: `openspec/specs/deployment/spec.md` exists and strict validation passes.
