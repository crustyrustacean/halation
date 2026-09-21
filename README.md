# Halation

An early-Instagram-style photo sharing server — chronological, calm, and ad-free.

Actix Web 4 · SQLx · Postgres · Tera · [Datastar](https://data-star.dev)

## Status

Phase 0 (scaffold) — infrastructure only: configuration layering, telemetry,
Postgres pool, storage backend abstraction (in-memory fake + OpenDAL S3/R2),
Tera rendering, health endpoints. No features yet.

## Deployment

The compose stack is the deployment unit: app + postgres + Caddy
(automatic HTTPS). On a fresh droplet:

```sh
scripts/init_dev_db.sh is NOT needed in production — the app creates
and migrates its database on boot.
```

See the deployment runbook (repo wiki / planning doc) for the full
step-by-step: droplet creation, deploy key, `.env`, image transfer
(`docker save | docker load`), Cloudflare DNS, and backups. Configuration
is environment-driven (`APP_` prefix, `__` separator) — see
`configuration/production.yaml`, `.env.example`, and
`docker-compose.yml`.

## Development

The app bootstraps its own database — it creates the database if missing
and applies migrations on every boot. All you need is the dev Postgres
container:

```sh
scripts/init_dev_db.sh            # first time: creates the container
cargo run                         # visit http://127.0.0.1:8000
```

`scripts/init_dev_db.sh --clean` additionally sweeps throwaway test
databases. Tests (`cargo test`) spin up their own throwaway databases
per test and need the same container running.

`DATABASE_URL` in `.env` points at `localhost:5433` for sqlx-cli.
