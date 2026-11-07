# Kubernetes (optional)

Use this only when a single host stops being enough: workloads that need real
isolation, a fleet that changes at runtime, or several services each wanting their
own database. For a fixed API plus a static site, `../compose/` is less to run.

> **Status:** renders with `kubectl kustomize overlays/example`. Not applied to a
> cluster as part of the template; the patterns come from a production cluster.

```
base/       backend + web Deployments/Services, one Ingress, CNPG Postgres + daily backup
overlays/   one directory per environment: digests, config, host, object store, secrets
```

What the base assumes is installed in the cluster:

- an Ingress controller (or swap `ingress.yaml` for a Gateway API `HTTPRoute`)
- CloudNativePG with the barman-cloud plugin
- Sealed Secrets or SOPS for `backend-secrets` and the backup credentials

## Deploy model

GitOps: ArgoCD (or Flux) watches `overlays/<env>`. A deploy is a commit that
changes an image digest there. CI builds and pushes images but never holds cluster
credentials.

## Where the rest lives

- Cluster bring-up, Cilium, policies, sealing: `~/Work/dev-workflows/k8s/` and
  `runbooks/talos-cluster-bringup.md`
- Why and when: notes hub *Kubernetes platform patterns*
- Restore drill: `runbooks/dr-rehearsal.md` (a CNPG restore is a new `Cluster`
  with `bootstrap.recovery`)
