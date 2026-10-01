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
cargo xtask dev-db is NOT needed in production — the app creates
and migrates its database on boot.
```

Full runbook: `docs/DEPLOYMENT.md`.

## Development

The app bootstraps its own database — it creates the database if missing
and applies migrations on every boot. All you need is the dev Postgres
container:

```sh
cargo xtask dev-db            # first time: creates the container
cargo run                     # visit http://127.0.0.1:8000
```

`cargo xtask dev-db --clean` additionally sweeps throwaway test
databases. Tests (`cargo test`) spin up their own throwaway databases
per test and need the same container running.

Interactive behaviour needs a real browser, so it gets its own check:

```sh
cargo xtask e2e                # start the app, click load-more, assert
cargo xtask e2e --url <url>    # drive an already-running instance
```

It launches Chrome, clicks the feed's load-more button, and fails if the
post count does not move — the one failure `cargo test` cannot see, since
the fragments and routes can all be correct while the browser never
executes the JavaScript. Set `HALATION_E2E_DEBUG=1` to see the browser's
console output and requests.

`DATABASE_URL` in `.env` points at `localhost:5433` for sqlx-cli.

Much of this code was written with AI assistance. To find the places worth
a second human look, there is a recorded technical-debt baseline:

```sh
debtmap analyze . --context-providers git_history
```

Current figures, and what they actually mean, are in
`docs/DEBT-BASELINE.md`. Read the caveats there before acting on the
headline number — the majority of it is vendored third-party JavaScript
that should not be touched.
