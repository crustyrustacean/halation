# Halation

An early-Instagram-style photo sharing server — chronological, calm, and ad-free.

Actix Web 4 · SQLx · SQLite · Tera · [Datastar](https://data-star.dev)

## Status

Active development (`v0.15.1`). Implemented so far:

- Configuration layering (`confik` + TOML), structured tracing telemetry, liveness (`/health_check`) and deep health (`/api/v1/health`)
- Password auth (Argon2id), server-side SQLite sessions, rate limiting, and registration gate
- Photo upload pipeline (JPEG/PNG/HEIC decoding, EXIF orientation + metadata extraction, reverse geocoding, owner-scoped S3/R2 derivatives)
- Chronological global & hashtag feeds, user profiles, follow/unfollow social graph, Datastar fragment updates

## Deployment

Halation deploys to **Fly.io** via [`fly.toml`](fly.toml) and [`Dockerfile`](Dockerfile),
with embedded SQLite on a Fly persistent volume (`/data/halation.db`) and
S3-compatible object storage (Cloudflare R2 or Fly Tigris). The app creates and
migrates its database on boot.

```sh
fly deploy
# or
cargo xtask deploy --dry-run
```

Full runbook: [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md).

## Development

The app bootstraps its own SQLite database (`data/halation.db`) and applies
migrations on every boot — no Docker container required:

```sh
cargo run                     # visit http://127.0.0.1:8000
```

`cargo xtask dev-db --clean` resets the local dev database and sweeps any
leftover temp test files. Tests (`cargo test`) spin up isolated throwaway
SQLite files per test and clean them up on drop.

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

`DATABASE_URL` in `.env` points at `sqlite://data/halation.db` for sqlx-cli.

Much of this code was written with AI assistance. To find the places worth
a second human look, there is a recorded technical-debt baseline:

```sh
debtmap analyze . --context-providers git_history
```

Current figures, and what they actually mean, are in
`docs/DEBT-BASELINE.md`. Read the caveats there before acting on the
headline number — the majority of it is vendored third-party JavaScript
that should not be touched.
