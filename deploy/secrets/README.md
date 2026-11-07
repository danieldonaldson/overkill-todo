# Secrets (SOPS + age)

Secrets are committed encrypted. Each file is encrypted to the age keys of the
people and hosts allowed to read it, and nobody else can decrypt it, CI included.

```
deploy/secrets/
  access.yaml            the policy: who can read what (edit this)
  local/  staging/  prod/  one directory per environment
    <name>.sops.env      a dotenv file, each value encrypted (keys stay readable in diffs)
    <name>.sops          one opaque value (a password, a key file)
.sops.yaml               GENERATED from access.yaml; never edit by hand
```

## The model

- **Levels cascade.** Each person is cleared for one level. A level reads
  itself and every level below it: `prod` reads prod, staging and local;
  `staging` reads staging and local; `local` reads local.
- **Per-secret exceptions.** An entry under `secrets:` in `access.yaml` can
  `only:` narrow a secret to named people of that level (for example, the prod
  backend env for the person who runs deploys), or `also:` grant one secret to
  someone cleared lower (for example, a staging person who needs prod Grafana).
- **Hosts read their own environment only.** A machine's age key is generated
  on the machine and decrypts its environment, nothing above or below it.
- **The unit of access is the file.** SOPS uses one data key per file, so two
  values with different readers must live in different files.

`just secrets-check` (in `just ci`, CI and the pre-commit hook) needs no keys.
It checks that the policy is valid, that `.sops.yaml` matches it, and that
every file is encrypted to exactly its readers. It also fails on anything
under `deploy/secrets/<env>/` that isn't a SOPS file, which catches plaintext
that was dropped in by mistake.

## Everyday use

```bash
sops edit deploy/secrets/prod/backend.sops.env          # edit in place
just secrets-new deploy/secrets/prod/db_password.sops    # new secret, value on stdin
openssl rand -base64 32 | tr -d '\n' | just secrets-new deploy/secrets/staging/db_password.sops
sops decrypt deploy/secrets/local/backend.sops.env       # read one
```

`secrets-new` encrypts straight from stdin, so the plaintext never touches the
disk. Create secrets with it rather than writing a plain file and running
`sops -e -i`.

## Changing who has access

All changes have the same shape: edit `access.yaml`, run `just secrets-sync`
(which regenerates `.sops.yaml` and re-keys every affected file), and commit
the three together. Re-keying a file requires you to be able to decrypt it,
so prod changes are synced by someone cleared for prod. `sync` lists any files
it couldn't re-key.

**Adding a person.** They run `age-keygen -o ~/.config/sops/age/keys.txt` and
send you the public key (the `age1...` line it prints). You add it under
`people:` with their level, then sync and commit.

**Adding a host.** Generate the key on the host and keep the private half
there:

```bash
mkdir -p ~/.config/sops/age && age-keygen -o ~/.config/sops/age/keys.txt   # on the host
```

Add the public key under `machines:` with its `env`, then sync, commit, and
pull on the host.

**Removing someone or lowering their level.** Sync re-keys the affected files
and rotates their data keys, so future edits are unreadable to that person.
They can still decrypt every version they could read before, from git history
or a copy. **Rotate the values themselves** (new passwords, new API keys) for
every file that sync lists as "removed".

## Recovery

If the only prod-cleared key is lost, prod secrets are unrecoverable. Keep at
least two prod readers: a second person, or an offline break-glass key
(`age-keygen` output stored in the team password manager, listed under
`people:` as e.g. `break-glass`, level `prod`).

## Deploying

On a host, `just secrets-decrypt prod` (or `./scripts/secrets.sh decrypt prod`
without `just`; the host only needs `sops`) writes the environment's secrets to
`deploy/compose/.secrets/`, which is gitignored:

- `<name>.sops.env` becomes `<name>.env`, used as an `env_file`.
- `<name>.sops` becomes `<name>`, used as a Compose `secret`.

The directory is mode 0700 and the files are 0644, so containers that don't
run as root (Grafana is uid 472) can read their bind-mounted secret. See
`../compose/README.md`.
