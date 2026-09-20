# Halation

An early-Instagram-style photo sharing server — chronological, calm, and ad-free.

Actix Web 4 · SQLx · Postgres · Tera · [Datastar](https://data-star.dev)

## Status

Phase 0 (scaffold) — infrastructure only: configuration layering, telemetry,
Postgres pool, storage backend abstraction (in-memory fake + OpenDAL S3/R2),
Tera rendering, health endpoints. No features yet.

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
