# Slurm SSH credentials

Profiles store only an `ssh_credential` slot name. That name is a directory,
not a hostname. Key material stays in this tree (or another directory pointed
at by `BEAMPIPE_SSH_CREDENTIALS_HOST`).

```text
credentials/
  known_hosts                 # optional shared host keys
  hpc/
    private_key               # required; mode 0600 or 0400
    private_key.pub           # install this on the login node
    passphrase                # required when the key is encrypted; mode 0600
    known_hosts               # optional per-slot host keys
```

Generate a new Beampipe-owned key (`init` does not log you in until the public
key is in remote `authorized_keys`):

```bash
beampipe slurm credentials init --slot hpc --host login.example.org --acl
beampipe slurm credentials copy-id --slot hpc --user USER --host login.example.org
beampipe slurm credentials check --slot hpc
```

Import an existing key instead, and skip the upload if the cluster already has
that public key:

```bash
beampipe slurm credentials import --slot hpc \
  --private-key ~/.ssh/id_ed25519 --known-hosts ~/.ssh/known_hosts --acl
```

Do not run `ssh-keygen` and `init` for the same slot. Beampipe does not use
`ssh-agent`; workers unlock `private_key` plus an optional passphrase file.

Compose bind-mounts `BEAMPIPE_SSH_CREDENTIALS_HOST` (default `./deploy/ssh/credentials`)
to `/run/beampipe/ssh` and sets `BEAMPIPE_SSH_CREDENTIALS_DIR=/run/beampipe/ssh`.

The container user is uid `10001`. `--acl` grants that uid read access on the
`0600` files. You can also run:

```bash
setfacl -m u:10001:r credentials/hpc/private_key credentials/hpc/passphrase
```

Do not commit `private_key`, `passphrase`, or live `known_hosts` files.
