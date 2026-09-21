# Halation

An early-Instagram-style photo sharing server — chronological, calm, and ad-free.

Actix Web 4 · SQLx · Postgres · Tera · [Datastar](https://data-star.dev)

## Status

Phase 0 (scaffold) — infrastructure only: configuration layering, telemetry,
Postgres pool, storage backend abstraction (in-memory fake + OpenDAL S3/R2),
Tera rendering, health endpoints. No features yet.

## Deployment

Multi-stage Dockerfile (non-root, healthcheck on `/health_check`):

```sh
docker build -t halation .
docker compose up --build   # app + postgres stack
```

Configuration is environment-driven (`APP_` prefix, `__` separator) —
see `configuration/production.yaml` and `docker-compose.yml`. For
production: set `APP_SECRETS__SESSION_SIGNING_KEY` (64+ random bytes),
point `APP_DATABASE__*` at your managed Postgres (DO/Railway — create
the `halation` database first, or grant the user CREATEDB and the app
creates it), and switch media storage to R2 with
`APP_STORAGE__BACKEND=s3` + the `APP_STORAGE__R2__*` variables.

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
