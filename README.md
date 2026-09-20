# Halation

An early-Instagram-style photo sharing server — chronological, calm, and ad-free.

Actix Web 4 · SQLx · Postgres · Tera · [Datastar](https://data-star.dev)

## Status

Phase 0 (scaffold) — infrastructure only: configuration layering, telemetry,
Postgres pool, storage backend abstraction (in-memory fake + OpenDAL S3/R2),
Tera rendering, health endpoints. No features yet.

## Development

Tests need a local Postgres 17 (the harness creates a throwaway database per test):

```sh
docker run -d --name halation-db-1 -e POSTGRES_USER=postgres \
  -e POSTGRES_PASSWORD=password -p 5433:5432 postgres:17
cargo test
```

`DATABASE_URL` in `.env` points at `localhost:5433` for sqlx-cli.
