# Halation

An early-Instagram-style photo sharing server — chronological, calm, and ad-free.

Actix Web 4 · SQLx · Postgres · Tera · [Datastar](https://data-star.dev)

## Status

Phase 0 (scaffold) — infrastructure only: configuration layering, telemetry,
Postgres pool, storage backend abstraction (in-memory fake + OpenDAL S3/R2),
Tera rendering, health endpoints. No features yet.

## Development

Start the dev database (once), then run migrations:

```sh
docker run -d --name halation-db-1 -e POSTGRES_USER=postgres \
  -e POSTGRES_PASSWORD=password -p 5433:5432 postgres:17
scripts/init_dev_db.sh            # creates `halation` + applies migrations
scripts/init_dev_db.sh --clean    # also sweeps throwaway test databases
```

Then `cargo run` and visit http://127.0.0.1:8000. Tests
(`cargo test`) spin up their own throwaway databases per test and
need the same container running.

`DATABASE_URL` in `.env` points at `localhost:5433` for sqlx-cli.
