# Dashboard deployment and security

Dash needs Node.js and network access to Core. It does not need PostgreSQL,
CASDA credentials, SSH keys, or access to worker filesystems.

```text
browser --HTTPS--> dashboard --private HTTP/HTTPS--> Core /api/v2
```

`BEAMPIPE_API_URL` is always interpreted from the **Dash server's** network
viewpoint, not the browser's:

| Dash runtime | Typical Core URL |
|---|---|
| Same Compose network | `http://api:8080` |
| Native process on the Core host | `http://127.0.0.1:18080` |
| Separate host | private TLS URL reachable from that server |

Do not use `127.0.0.1:18080` inside a Dash container unless Core runs in that
same container.

## Compose installation

Start Core first, then let its setup command install Dash:

```bash
beampipe start
beampipe doctor
beampipe setup --dashboard
```

For native Core, `beampipe start` remains in the foreground; run the doctor
and Dashboard setup in a second terminal.

Or run the Dashboard installer directly:

```bash
curl -fsSL https://raw.githubusercontent.com/jbwod/beampipe-dash/main/scripts/install.sh | sh

# From an existing Dashboard checkout:
./scripts/install.sh --core-home "$HOME/beampipe"
docker compose ps
```

The installer writes `compose.beampipe-local.yml`, joins Core's Compose
network, sets `BEAMPIPE_API_URL=http://api:8080`, and publishes Dash on
`127.0.0.1:3000`. Without that overlay, the Dashboard repository's base
Compose file uses `host.docker.internal` and Core's published host port.

The image runs as UID/GID `1001`, uses Next.js standalone output, and exposes a
container health check at `/api/health`. Dash has no database migration or
persistent volume.

## Native process

Node.js 24 is the supported development and production runtime:

```bash
git clone https://github.com/jbwod/beampipe-dash.git
cd beampipe-dash
npm ci
npm run build

NODE_ENV=production \
PORT=3000 \
BEAMPIPE_API_URL=http://127.0.0.1:18080 \
BEAMPIPE_DASH_SECURE_COOKIES=false \
npm start
```

Run the process as a dedicated unprivileged user under a service manager.
`NEXT_PUBLIC_EAGLE_URL` is a build-time public editor URL; the other settings
below are runtime values.

## Configuration

| Variable | Timing | Purpose |
|---|---|---|
| `BEAMPIPE_API_URL` | runtime | Core base URL reachable by the Dash server |
| `BEAMPIPE_DASH_SECURE_COOKIES` | runtime | Must be `true` when the browser uses HTTPS |
| `PORT` | runtime | Native Dashboard listen port |
| `NEXT_PUBLIC_EAGLE_URL` | build | Public EAGLE editor base URL |

`BEAMPIPE_API_URL` is server-only and is never exposed to browser JavaScript.
Do not put credentials in it.

## Reverse proxy

Terminate TLS in a trusted reverse proxy and forward all paths to Dash,
including `/api/*`. Overwrite forwarded authority headers; do not append
untrusted client values.

```nginx
location / {
    proxy_pass http://beampipe-dash:3000;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-Host $host;
    proxy_set_header X-Forwarded-Proto $scheme;
    proxy_set_header X-Real-IP $remote_addr;
}
```

For an HTTPS browser origin:

```text
BEAMPIPE_DASH_SECURE_COOKIES=true
```

Publish Dash, not Core's API port, to the operator LAN. Core itself consumes
`X-Forwarded-For` only from peers listed in `BEAMPIPE_TRUSTED_PROXY_CIDRS`; see
[API rate limiting and proxy trust](../operations/index.md#api-rate-limiting-and-proxy-trust).

## Session and secret posture

- Access and refresh tokens are always `HttpOnly`, `SameSite=Lax`, and scoped
  to `/`. They are `Secure` only when
  `BEAMPIPE_DASH_SECURE_COOKIES=true`.
- Refresh rotates through Core and the original request is retried once.
- Client JavaScript cannot read or write tokens.
- Cross-site mutation requests are rejected using fetch metadata and
  origin/authority checks.
- The BFF strips inbound authorization headers and adds its own bearer token.
- Upstream errors pass through Core's redaction policy.

Run one Dashboard replica or use sticky affinity per browser session. Refresh
coalescing is process-local, so unconstrained multi-replica routing can race
rotating refresh tokens.

Deployment profiles contain targets and resource policy, not private keys or
passphrases. Configure SSH slots, CASDA passwords, JWT secrets, notification
credentials, and routing keys in Core. Dash shows redacted readiness metadata
and omits an unchanged redacted field when saving.

```bash
beampipe security check
beampipe doctor --profile PROFILE_NAME
```

## Health and troubleshooting

| Endpoint or symptom | Interpretation |
|---|---|
| Dash `/api/health` | Next.js process is serving requests |
| Dash `/api/connection` | Dash server can reach Core health |
| Core `/api/v2/ready` | Authenticated database, Redis, TAP, runnable-queue, and running-job checks |
| `beampipe worker list` / **Workers** | Worker heartbeat, capability, and pool health |
| `beampipe doctor --profile NAME` | TM/DIM or SSH/Slurm profile connectivity |
| Login reports unavailable | Check `BEAMPIPE_API_URL` from inside the Dash process/container |
| Login returns `401` | Account/password is rejected; bootstrap or reset the Core account |
| Mutation returns `403` behind proxy | Check overwritten `Host`, `X-Forwarded-Host`, and `X-Forwarded-Proto` |
| Session expires immediately on HTTPS | Set `BEAMPIPE_DASH_SECURE_COOKIES=true` |
| Save/test action disabled | The current Core user must be a superuser |

After a native upgrade, repeat `npm ci`, the quality checks, `npm run build`,
and the supervised restart. Compose deployments replace the image and need no
Dash data migration.
