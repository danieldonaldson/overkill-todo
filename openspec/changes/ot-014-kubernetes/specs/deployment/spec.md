# Spec Delta

## Purpose

Lets the operator run the whole app on a cluster whose contents come from the
repository, and rebuild, deploy, roll back and restore it with known results.

## ADDED Requirements

### Requirement: The cluster can be rebuilt from the repository
The operator SHALL be able to create and destroy a single-node cluster on their
own computer with documented commands. Rebuilding SHALL need no
unpublished configuration and no hand-made credentials beyond the operator's
decryption key and the cluster's key.

#### Scenario: Create from nothing
- **WHEN** the operator runs the documented commands on a computer with no cluster
- **THEN** a cluster is created and every committed workload becomes healthy without further manual changes

#### Scenario: Creation is interrupted
- **WHEN** creation stops part-way and the operator destroys the cluster and creates it again
- **THEN** it succeeds with no leftovers from the first attempt

#### Scenario: Not enough resources
- **WHEN** the computer lacks the required memory or disk
- **THEN** creation stops before changing anything, naming what is missing

#### Scenario: Local development is unaffected
- **WHEN** the cluster is running
- **THEN** the local development stack starts and behaves as before, with no port conflicts

#### Scenario: Destroying the cluster touches nothing else
- **WHEN** the operator destroys the cluster
- **THEN** the local development data and the stored backups are unchanged

### Requirement: What runs is what is committed
The cluster SHALL apply the state committed to its tracked branch, with the
app's images pinned by digest. The documented bootstrap (the machine's own
configuration and the cluster's key) is the only exception. CI SHALL NOT hold cluster credentials, and the cluster SHALL
NOT be able to write to the repository.

#### Scenario: Deploy
- **WHEN** a commit changes an image digest
- **THEN** that image is running within 10 minutes, and requests made during the rollout do not fail

#### Scenario: Roll back
- **WHEN** that commit is reverted
- **THEN** the previous image is running within 10 minutes against the newer schema

#### Scenario: Manual change
- **WHEN** someone changes a committed setting directly in the cluster, outside the documented bootstrap
- **THEN** it is returned to the committed value within 15 minutes

#### Scenario: Broken image
- **WHEN** a commit names an image that does not start or does not become ready
- **THEN** the previous version keeps serving, and the operator's status command names the workload that is stuck

#### Scenario: Invalid manifest
- **WHEN** a manifest that fails to render or does not match its schema is committed
- **THEN** the full gate and CI fail, naming the file or resource

#### Scenario: Cluster cannot write to the repository
- **WHEN** the credentials held in the cluster are listed
- **THEN** none can write to the repository

#### Scenario: Repository unreachable
- **WHEN** the repository can't be reached
- **THEN** running workloads keep serving

### Requirement: Secrets are committed only encrypted
Cluster secrets SHALL follow the repository's secrets policy, including
post-quantum keys only, and SHALL be readable only by the people and the
cluster the policy names.

#### Scenario: Cluster reads only its own environment
- **WHEN** the cluster's key is used on another environment's secrets
- **THEN** decryption fails

#### Scenario: Missing or undecryptable secret
- **WHEN** a secret a workload needs is missing or can't be decrypted
- **THEN** the new version doesn't start, the previous one keeps serving, and the failure names the secret but not its value

#### Scenario: Decrypted secrets are not exposed by the deploy tooling
- **WHEN** a workload outside the deploy tooling tries to reach its stores of rendered configuration
- **THEN** the connection is refused or times out

#### Scenario: Secret changes
- **WHEN** an encrypted secret is changed and committed
- **THEN** the workloads using it restart with the new value

### Requirement: The app behaves as in production
The cluster SHALL run the app with development mode off, served from one TLS
address, with every service the app requires configured. The app SHALL meet
its other specifications there.

#### Scenario: Member signs in
- **WHEN** a Visitor registers with an emailed code and signs in over TLS
- **THEN** they reach their dashboard as a Member

#### Scenario: Smoke checks pass
- **WHEN** the operator runs the read-only smoke checks against the cluster's address
- **THEN** they all pass, including the post-quantum key-exchange check

#### Scenario: Database unavailable
- **WHEN** the database is stopped and then returns
- **THEN** the backend reports not ready meanwhile, and serves again without a manual restart

#### Scenario: Machine restarts
- **WHEN** the virtual machine restarts
- **THEN** every workload returns to healthy with no data lost

### Requirement: The database survives losing its primary
The cluster SHALL run the database with replicas, at least one synchronous, so
that losing the primary instance promotes a replica without losing a committed
write.

#### Scenario: Primary fails
- **WHEN** the primary database instance is killed while requests are being served
- **THEN** a replica is promoted within 60 seconds, every write the app confirmed before the failure is present, and the app serves again without a manual restart

#### Scenario: Planned switchover
- **WHEN** the operator switches the primary to another instance
- **THEN** reads keep succeeding throughout, and writes resume within 30 seconds

### Requirement: Account data can be restored
Account data SHALL be backed up continuously to storage outside the virtual
machine. Every restore rehearsal SHALL record how long it took and the newest
data recovered.

#### Scenario: Restore into a rebuilt cluster
- **WHEN** a row is written after the last full backup, its backup is confirmed, and the cluster is destroyed and rebuilt
- **THEN** that row and all earlier account data are present

#### Scenario: Rebuild never silently starts empty
- **WHEN** a cluster expected to restore finds no usable backup
- **THEN** the database doesn't start, rather than starting empty

#### Scenario: Backup storage unavailable
- **WHEN** backup storage can't be reached
- **THEN** the app keeps serving, the operator can see backups are failing, and backups resume when it returns

#### Scenario: Backups stop succeeding
- **WHEN** no backup has succeeded for 26 hours
- **THEN** the operator is alerted as for other operational problems

### Requirement: Workloads are isolated
Each app, database, observability and analytics workload SHALL be able to reach
only what it needs, SHALL run without root or added privileges, and SHALL have
a memory limit. Exceptions, and the cluster's own components, SHALL be listed
with a reason.

#### Scenario: Unneeded connection is refused
- **WHEN** a workload with no need for the database tries to connect to it
- **THEN** the connection is refused or times out

#### Scenario: Only the edge reaches the backend
- **WHEN** a workload other than the edge, or the readiness probe, connects to the backend
- **THEN** the connection is refused or times out

#### Scenario: Operator tools stay private
- **WHEN** a Visitor requests database, backup or cluster management through the app's address
- **THEN** none is reachable

#### Scenario: Privileged workload is rejected
- **WHEN** a workload asking for privileges with no documented exception is applied
- **THEN** it is rejected

#### Scenario: Workload exceeds its memory limit
- **WHEN** one workload uses more memory than its limit
- **THEN** only that workload restarts
