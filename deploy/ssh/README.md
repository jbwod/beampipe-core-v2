# Slurm SSH credentials

Profiles store only an `ssh_credential` slot name. Key material stays in this
tree (or another directory pointed at by `BEAMPIPE_SSH_CREDENTIALS_HOST`).

```text
credentials/
  known_hosts                 # optional shared host keys
  setonix/
    private_key               # required; mode 0600 or 0400
    passphrase                # required when the key is encrypted; mode 0600
    known_hosts               # optional per-slot host keys
```

Create that layout with:

```bash
beampipe slurm credentials init --slot setonix --acl
beampipe slurm credentials check --slot setonix
```

Compose bind-mounts `BEAMPIPE_SSH_CREDENTIALS_HOST` (default `./deploy/ssh/credentials`)
to `/run/beampipe/ssh` and sets `BEAMPIPE_SSH_CREDENTIALS_DIR=/run/beampipe/ssh`.

The container user is uid `10001`. `--acl` grants that uid read access on the
`0600` files. You can also run:

```bash
setfacl -m u:10001:r credentials/setonix/private_key credentials/setonix/passphrase
```

Do not commit `private_key`, `passphrase`, or live `known_hosts` files.
