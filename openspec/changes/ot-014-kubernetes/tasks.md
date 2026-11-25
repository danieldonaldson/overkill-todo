# Tasks

"Deployed" here means running on the local cluster; there is no production.
Work stays on the branch until the operator asks for the merge (D10).
Slices 2–7 are outlined in design.md and get tasks after slice 1 ships. They go
before "Feature done", which is renumbered to stay last.

## 1. Slice 1 (first slice): commits drive a fresh cluster, secrets included

Delivers these scenarios: Creation is interrupted; Not enough resources; Local
development is unaffected; Manual change; Invalid manifest; Repository
unreachable; Cluster cannot write to the repository; Cluster reads only its own
environment; Missing or undecryptable secret; Decrypted secrets are not exposed by the deploy tooling; Secret changes.
With a throwaway workload it also exercises Create from nothing, Deploy, Roll
back and Broken image, which slice 2 completes for the app.

- [ ] 1.1 Pin `talosctl`, `kubectl`, `helm`, `argocd`, `sops` (at least 3.13, no longer `latest`) and `kubeconform` in `.mise.toml`, and the rest in `deploy/k8s/versions.env` (D14); check the versions support each other. Verify: each CLI prints its pin, and the pinned operator's `go.mod` has SOPS ≥ 3.13 and age ≥ 1.3.
- [ ] 1.2 Add `just k8s-up` / `k8s-down`: resource preflight, Talos VM, per-cluster kubeconfig, Cilium (D2). Verify: the node is Ready; interrupting, then down and up, works; too little memory or disk stops before any change; `just dev` runs alongside, and its data survives `k8s-down`.
- [ ] 1.3 Extend `scripts/secrets.sh` with `machines: false` and the `<name>.sops.yaml` form (D3), and document both. Verify: `just secrets-check` accepts one, and fails on a plaintext value, on a classical recipient and on a kind other than `SopsSecret`; `secrets-new`, `secrets-sync` and `secrets-decrypt` behave as in D3.
- [ ] 1.4 Add the cluster's key to the policy; encrypt the Talos secrets and the throwaway `local` secrets. Verify: `just secrets-check` passes; the cluster's key fails on a `prod` file and on the Talos secrets.
- [ ] 1.5 Commit ArgoCD's manifests and NetworkPolicies, the root, self and two-source workloads Applications (D3, D10, D11), the SOPS operator, Reloader, local-path storage and the Cilium Application, applied once by `k8s-up`. Verify: every Application is Synced and Healthy, ArgoCD manages itself, Cilium is adopted without restarting, a test volume binds, no Secret holds a Git credential, the age key is mounted only in the SOPS operator, a second render of Cilium shows no diff, and a pod outside ArgoCD can't reach its Redis or repo-server.
- [ ] 1.6 Retire the Hypothesis with the throwaway secrets and a workload using them. Verify: every condition in design.md holds, recorded there; if the stale-status case fails, use versioned names, and otherwise apply the D3 fallback, before going on.
- [ ] 1.7 Add `just k8s-check` with committed schemas for the custom resources (D13), in `ci-checks` and a `k8s` CI job the `images` job needs. Verify: an invalid field in a Deployment and in an ArgoCD Application each fail, naming the resource, and no keys are needed.
- [ ] 1.8 Add `just k8s-status`: failed syncs, plus rollout and pod state. Verify: it names the resource for a broken commit, and the stuck workload for an image that never becomes ready.
- [ ] 1.9 Exercise deploy, rollback, drift on a workload and on ArgoCD itself, broken image, missing and undecryptable secret, and repository outage, with the throwaway workload. Verify: outcomes and timings are recorded in design.md and within the spec's limits.
- [ ] 1.10 Rewrite `deploy/k8s/README.md` (bring-up, deploy, rollback, bootstrap boundary, key removal) and remove Sealed Secrets guidance from it, `.mise.toml`, `backend/.env.example` and `overlays/example/` (both files). Verify: following the README on a destroyed cluster converges with no other manual steps, and nothing recommends Sealed Secrets.
- [ ] 1.11 `just ci` green. Revise design.md with what slice 1 showed, remove the throwaway workload and secrets, and add tasks for slices 2–7. Slice 2 starts with the D6 edge test and moves the base's namespace into it (D12), and removes the base's barman-cloud plugin and ScheduledBackup from the local overlay, not just its ObjectStore, until slice 3. Verify: `openspec validate ot-014-kubernetes --strict` passes.

## 2. Feature done

- [ ] 2.1 When the operator asks, merge the branch to `main` and switch ArgoCD to it (D10). Then check every scenario on a cluster rebuilt from nothing, with ArgoCD tracking `main`. Verify: results recorded in design.md.
- [ ] 2.2 Update README, `deploy/k8s/README.md`, `deploy/compose/README.md` and the threat model's edge assumptions. Verify: docs match the final state and `just ci` is green.
- [ ] 2.3 Archive the change. Verify: `openspec/specs/deployment/spec.md` exists and strict validation passes.
