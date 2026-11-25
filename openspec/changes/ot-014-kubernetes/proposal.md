# OT-014: Kubernetes

## Why

As the operator, I want to run the whole app on a Kubernetes cluster I can
break and rebuild, so that I learn to operate one on a system I know well.

The motivation is learning, not scale: one host is still enough. The existing
Kubernetes manifests render, but have never been applied as part of this repo,
so nothing they claim about deploys, backups or isolation has been tested.
There is no production and no data to move, which makes this a cheap time to
find out.

## What Changes

- The operator can create, destroy and recreate a single-node cluster in a
  virtual machine on their own computer, with documented commands.
- What runs in the cluster is what is committed, apart from a short documented
  bootstrap. Deploying or rolling back is
  committing an image digest; CI still holds no cluster credentials.
- The app, with its database, secrets and TLS edge, runs there with
  development mode off.
- Account data is backed up outside the virtual machine, and a timed restore
  into a rebuilt cluster is rehearsed.
- Workloads can reach only what they need.
- The existing observability and analytics behaviour works there too.
- Compose stays: it remains the way to run the app locally, and its production
  override is untouched. Kubernetes sits beside it.

## Capabilities

### New Capabilities

- `deployment`: running the app on a cluster: rebuilding it, deploying,
  rolling back, secrets, backup and restore, and isolation.

### Modified Capabilities

None. The observability, analytics and security requirements don't depend on
the platform, so the cluster must meet them as written.

## Impact

Deploy manifests, operator docs and recipes, the secrets policy (a cluster
key), and the gate (manifest checks). No API, schema or page changes.

## Out of Scope

- Choosing a production target, cloud hosting, public DNS or public
  certificates.
- Several nodes, high availability and autoscaling.
- Replacing or changing the compose workflow.
- Surviving loss of the operator's computer: backups leave the virtual machine,
  not the host.
- Sign-in rate limiting at the edge (threat model P-03 stays an accepted gap).
- Cluster-only product features.

## Dependencies

- The images CI publishes from `main`. Both pull anonymously (checked
  2026-11-25) and must stay public.
- A sandbox SMTP account with a publicly trusted certificate.
- Outgoing internet access for Turnstile's published test keys.
- Host capacity: about 12 GiB of memory and 40 GiB of disk for the machine.
- OT-013 is in flight but doesn't block this.

## Appetite

TBD.

## Shape of the work

Each part can be released on its own:

1. Cluster, GitOps and secrets.
2. The app and its database over TLS.
3. Backup outside the machine, and a timed restore.
4. Isolation and the full edge contract.
5. Observability.
6. Analytics.
7. Rebuild everything from nothing.

## Ready

- **Riskiest unknown:** Whether the cluster can decrypt the repo's committed
  post-quantum secrets from the single encrypted copy, and restart workloads
  when one changes. If it can't, the deploy model changes.
- **First slice:** A commit deploys a small workload to a fresh local cluster,
  reading a secret decrypted there; changing or reverting the commit takes
  effect.
