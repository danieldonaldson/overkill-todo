# Migrations

## Policy: forward-only

Migrations in this directory are **forward-only**. We do not ship `.down.sql`
reversal scripts. Each migration is applied once, in order, and recorded in the
`_sqlx_migrations` table by `sqlx migrate run`.

Rationale: down-migrations are rarely exercised, drift from the real schema, and
give false confidence in production. We prefer roll-*forward* fixes (a new
migration that corrects the previous one) over automated rollbacks.

## Policy: backward-compatible (expand/contract) — mandatory

Every migration **must be backward-compatible with the currently-running app
version.** This is not a style preference; it is required by how the backend
deploys.

The backend Deployment uses a surge rolling update (`maxSurge=1`,
`maxUnavailable=0` — see `k8s/apps/base/backend/deployment.yaml`) so a restart
never drops the sole replica. During every roll the **new pod and the old pod
run against the same database at the same time** for a few seconds. The new pod
runs `sqlx migrate run` at boot (serialized safely by the default Postgres
advisory lock), so the schema can change *while the old pod is still serving
requests against it*. If a migration removes or narrows something the old code
still uses, the old pod starts erroring mid-roll — a self-inflicted outage
during what is supposed to be a zero-downtime deploy.

Safe in a single migration (old code keeps working):

- `ADD COLUMN` that is nullable or has a `DEFAULT`
- `CREATE TABLE`, `CREATE INDEX CONCURRENTLY` (see below)
- widening a type, adding a permissive constraint

**Not** safe in a single migration — split across two separate deploys:

- `DROP COLUMN` / `DROP TABLE`, rename
- adding `NOT NULL` without a default, narrowing a type
- tightening a constraint the old code can still violate

Breaking changes follow the **expand → migrate → contract** sequence, one step
per deploy:

1. **Expand** — add the new shape (new column/table/constraint) alongside the
   old. Deploy. Both old and new code work.
2. **Migrate** — ship app code that reads/writes the new shape and stops using
   the old. Backfill as needed. Deploy.
3. **Contract** — once no running pod references the old shape, drop it in a
   later, separate migration + deploy.

Never collapse a breaking change into one migration just because it is fewer
files. The overlap window is short but real, and the whole point of the surge
strategy is that nobody sees an error during a deploy.

## Naming: UTC timestamp prefixes

The old chain used sequential numeric prefixes (`001`..`021`). These **collide
across parallel branches** — two branches both adding `022_*.sql` produce a
conflict (or worse, a silent reordering) at merge time.

Use a UTC timestamp prefix, matching the baseline:

```
20260619142530_add_widget_table.sql
```

Timestamps sort chronologically, never collide between branches, and make merge
order unambiguous. This is also the format `sqlx migrate add` generates.

**The prefix must be digits only.** sqlx parses everything before the first `_`
with `parse::<i64>()` and returns a hard error — not a warning, not a skip — if
it does not parse:

```
error parsing migration filename "...": expected integer version prefix
```

So `20260619T142530_add_widget_table.sql` (with a `T`) fails to resolve the
entire migration set. Write `20260619142530_…`, no separator.

## Indexing a populated table: use `CREATE INDEX CONCURRENTLY`

The baseline creates indexes with a bare `CREATE INDEX`. That is **only safe
because it runs against empty bootstrap tables** — a plain `CREATE INDEX` takes
an `ACCESS EXCLUSIVE` lock for the duration of the build, blocking all reads and
writes on that table.

For any index added to a table that already holds rows in a deployed environment
(`users`, `refresh_tokens`, …), use `CREATE INDEX CONCURRENTLY` so the build does
not block live auth traffic:

```sql
CREATE INDEX CONCURRENTLY idx_users_last_active_at ON users (last_active_at);
```

Caveats for `CONCURRENTLY`:

- It **cannot run inside a transaction.** sqlx wraps each migration file in one
  by default, so a `CONCURRENTLY` migration must opt out with the first line
  (sqlx matches this literal prefix exactly — `-- sqlx:no-transaction` and other
  variants are silently ignored, leaving the transaction in place):
  ```sql
  -- no-transaction
  ```
  Even with the opt-out, sqlx sends the whole file to Postgres as a single
  `execute()`, which Postgres runs as one implicit transaction — so a
  `CONCURRENTLY` statement must be the **only** statement in its migration file.
  Put any accompanying DDL in a separate migration.
- It can leave an `INVALID` index behind if the build fails (e.g. a unique
  violation). Drop and recreate rather than trusting a half-built index.
- Prefer **expand/contract** for column/constraint changes on populated tables:
  add the new shape, backfill, switch reads/writes, then drop the old — each step
  a separate, individually-revertible migration.

## Adding a value to an enum: two migrations, not one

`user_role`, `oauth_provider`, and friends are Postgres `ENUM` types.
Adding a value and *using* it in the same migration fails:

```
ERROR:  unsafe use of new value "moderator" of enum type user_role
HINT:   New enum values must be committed before they can be used.
```

Postgres allows `ALTER TYPE … ADD VALUE` inside a transaction, but the new label
cannot be referenced until that transaction commits — and sqlx runs each
migration file in one transaction. So this is a single-file trap:

```sql
ALTER TYPE user_role ADD VALUE 'moderator';
UPDATE users SET role = 'moderator' WHERE …;  -- fails
```

Split it:

1. One migration containing only the `ALTER TYPE … ADD VALUE`.
2. A **separate** migration that backfills or otherwise references the new label.

(A single file with `-- no-transaction` also works, but it forfeits atomicity for
every statement in that file — prefer the split.)

This composes with the expand/contract policy above: adding a label is an
*expand* step and is safe for the old pod, which simply never emits the new
value. Postgres cannot remove an enum label at all short of rebuilding the type.

## Rolling back manually

There is no `sqlx migrate revert` here because we author no down scripts. To
undo a migration in a non-production / recoverable environment:

1. **Prefer roll-forward.** Write a new migration that reverses the unwanted
   change (drop the column/table, restore the prior constraint, etc.) and run
   `cargo sqlx migrate run`. This keeps history linear and auditable.

2. **If you must hand-revert** (e.g. a bad migration on a dev DB):
   - Manually run the inverse SQL against the database
     (`DROP TABLE ...`, `ALTER TABLE ... DROP COLUMN ...`, etc.).
   - Delete the corresponding row from `_sqlx_migrations` so sqlx no longer
     considers it applied:
     ```sql
     DELETE FROM _sqlx_migrations WHERE version = <numeric_prefix>;
     ```
   - For destructive production rollbacks, restore from a database backup rather
     than hand-editing schema and migration bookkeeping.

After changing any query or schema, run `just db-prepare` and commit the updated
`.sqlx/` offline cache.
