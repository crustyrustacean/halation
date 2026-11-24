# Deploying Halation to Fly.io

Target shape: Halation running on a Fly.io Machine with embedded SQLite (WAL
mode) stored on a persistent Fly volume (`/data/halation.db`), media in
Cloudflare R2 (or Fly Tigris — any S3-compatible bucket), and automatic TLS
terminated at Fly's edge proxy.

```
Internet ──> Fly Edge (TLS + Fly-Client-IP) ──> Halation Machine (:8000) ──> /data/halation.db (Fly Volume)
                                                                         └──> Cloudflare R2 / Tigris (media)
```

## 0. Prerequisites

- [`flyctl`](https://fly.io/docs/flyctl/install/) installed and authenticated (`fly auth login`)
- An S3-compatible bucket for media (Cloudflare R2 or Fly Tigris): bucket name,
  endpoint URL, access key ID, and secret access key
- Optional: a custom domain (e.g. `halation.photos`) whose DNS you control

## 1. Create the Fly app

Review [`fly.toml`](../fly.toml) first and adjust `app` (must be globally unique
on Fly.io) and `primary_region` (e.g. `ord`, `sea`, `yyz`, `iad`, `lhr`) to the
region closest to you.

```bash
fly apps create halation
```

## 2. Create the persistent volume for SQLite

`fly.toml` mounts a Fly volume named `halation_data` at `/data`, where Halation
stores `/data/halation.db` (and its `-wal`/`-shm` sidecars) and applies SQLx
migrations automatically on boot.

```bash
fly volumes create halation_data --region ord --size 1
```

> Match `--region` to `primary_region` in `fly.toml`. Keep the app at 1 machine
> while running embedded SQLite (the `DatabaseBackend` trait makes swapping to
> Postgres later a drop-in change).

## 3. Set production secrets

Generate a session signing key (at least 64 bytes — Halation refuses to boot in
production with the checked-in dev key or a short key):

```bash
openssl rand -hex 48
```

Set all secrets in one call (staging them so they take effect on the first
deploy):

```bash
fly secrets set --stage \
  APP_SECRETS__SESSION_SIGNING_KEY="<96-char hex from openssl>" \
  APP_APPLICATION__BASE_URL="https://halation.photos" \
  APP_STORAGE__R2__BUCKET="<your bucket>" \
  APP_STORAGE__R2__ENDPOINT="https://<account-id>.r2.cloudflarestorage.com" \
  APP_STORAGE__R2__ACCESS_KEY="<access key id>" \
  APP_STORAGE__R2__SECRET_KEY="<secret access key>"
```

Notes on optional env vars (set in `fly.toml` `[env]` or via `fly secrets set`):

- **Registration**: `configuration/production.toml` closes self-service
  registration (`registration_open = false`) while under active development. To
  open registration, set `APP_APPLICATION__REGISTRATION_OPEN=true`.
- **Trusted proxy**: `configuration/production.toml` sets `trusted_proxy = true`.
  Halation reads `Fly-Client-IP` (set and overwritten by Fly's edge proxy) for
  rate-limiting and session metadata.
- **Reverse geocoding**: enabled by default (`APP_GEOCODE__ENABLED=true` in
  `fly.toml`).
- **Storage prefix**: set `APP_STORAGE__R2__FS_ROOT` only if you want all
  objects nested under a subfolder inside the bucket.

## 4. Deploy

```bash
fly deploy
# or via xtask (also checks `fly status`):
cargo xtask deploy --yes
```

Fly builds the multi-stage `Dockerfile` on its remote x86-64 builders (no local
cross-compilation needed), mounts `halation_data` at `/data`, starts the
machine, and waits for `GET /health_check` to return `200 OK`.

## 5. Custom domain and TLS (optional)

```bash
fly certs add halation.photos
fly certs show halation.photos
```

Create the `A` / `AAAA` (or `CNAME`) records Fly prints in your DNS provider.
If you use Cloudflare DNS, keep the record **DNS only (grey cloud)** while Fly
issues the Let's Encrypt certificate (or configure Cloudflare SSL/TLS to
**Full (strict)** once issued).

## 6. Verify

```bash
fly status
curl -fsSL https://halation.photos/api/v1/health
```

Expected response from `/api/v1/health`:

```json
{
  "status": "ok",
  "version": "0.15.0",
  "database": "ok",
  "storage": "ok"
}
```

Then verify in the browser:

- Visit `https://halation.photos` (or `https://<app>.fly.dev`)
- Log in and upload a photo (JPEG or iPhone HEIC)
- Confirm the original + derivatives (`thumb`, `medium`, `large`) appear in your
  R2 / Tigris bucket under `{owner_id}/{media_id}/`

## 7. Operations & Maintenance

- **Logs**: `fly logs`
- **Shell into a running machine**: `fly ssh console`
- **Deploy an update**: `fly deploy` (or `cargo xtask deploy --yes`)
- **Scale memory / VM**: edit `[[vm]]` in `fly.toml` (default `1gb` gives
  headroom for decoding large HEIC/JPEG uploads in memory) and redeploy.
- **Keep a machine warm (no cold starts)**: set `min_machines_running = 1` in
  `fly.toml`.
- **SQLite backups**: Fly automatically takes daily volume snapshots (retained
  for 5 days by default; inspect with `fly volumes snapshots list <vol-id>`).
  For an ad-hoc local copy, run `fly ssh sftp get /data/halation.db`.

### Media key backfill (legacy pre-0.13 objects only)

The deploy image ships `/app/backfill_media_keys` alongside `/app/halation` for
migrating legacy `{media_id}/…` objects to owner-scoped `{owner_id}/{media_id}/…`
keys:

```bash
fly ssh console -C "/app/backfill_media_keys --dry-run"
fly ssh console -C "/app/backfill_media_keys"
```
