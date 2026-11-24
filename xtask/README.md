# xtask

Dev automation for Halation. One binary, one dialect, every platform.

The point is not code-sharing. It's that the release and deploy mechanics are
*commands you can run* instead of a ritual you have to remember — which, for a
solo dev, is the difference between the work happening and not happening.

```
cargo xtask                # usage
cargo xtask release-check  # is the bookkeeping coherent?
cargo xtask bump 0.14.0    # version bump + open a CHANGELOG section
cargo xtask dev-db          # ensure the dev database exists
cargo xtask deploy --dry-run
```

## Commands

### `release-check`

Verifies `Cargo.toml`, `CHANGELOG.md`, and the git history tell the same story:

- the newest CHANGELOG entry matches the `Cargo.toml` version;
- CHANGELOG dates descend — the file is newest-first, so a date that *increases*
  going down means a release is dated before the one above it;
- no version appears twice (edits an old release instead of appending);
- the latest git tag agrees with `Cargo.toml`.

Exits non-zero on any problem, so it drops into a pre-commit hook or CI unchanged.

This exists because the bookkeeping has already slipped: the storage
partitioning merge shipped without its `Cargo.toml` version bump and had to be
folded into a later release. Run it before committing and after a merge.

### `bump <version>`

Rewrites the `Cargo.toml` version and opens a matching `## [x.y.z] - date`
section at the top of the CHANGELOG. It does **not** commit, tag, or push —
those remain your decision.

`next_patch` is mechanical and suggests `0.13.1` for `0.13.0`; whether a change
warrants a patch or minor bump is a judgement call, so pass the version
explicitly.

### `dev-db [--clean]`

Ensures the local SQLite `data/` directory exists (no Docker container needed;
the app creates and migrates `data/halation.db` on boot). `--clean` deletes
`data/halation.db` (plus `-wal`/`-shm` sidecars) and sweeps any leftover temp
test database files.

### `deploy [--dry-run] [--yes] [--url=…]`

Prints the Fly.io deploy plan, and **refuses to run without `--yes`**. With
`--yes`, it checks that `fly` is installed, runs `fly deploy` and `fly status`,
and reminds you to verify `/api/v1/health`:

1. `fly deploy` (builds the `Dockerfile` remotely on Fly's x86-64 builders);
2. `fly status` (confirm machines are started and passing `/health_check`);
3. read the *serving* version from `/api/v1/health` to confirm it matches
   `Cargo.toml` and both `database` and `storage` report `"ok"`.

Health URL defaults to `https://halation.photos/api/v1/health`, overridable via
`--url=` or `HALATION_HEALTH_URL`.

## Scope

xtask owns **dev-side** automation only. Runtime tools that must run against
the live environment inside the deploy image — `backfill_media_keys`,
`reset_password` — stay in `src/bin/`. An xtask runs on your machine and has
neither the R2 credentials nor the live database.

## Design notes

- **No dependencies.** Not even `clap`. This is a build-time tool; every
  dependency is another way `cargo check` at the repo root can fail for
  reasons unrelated to the app.
- `std::env::args()` and a `match`. Easy to read, trivial to extend.
- The `.cargo/config.toml` alias makes `cargo xtask` work from the root with no
  wrapper script.
- Unit tests live in the modules; run them with `cargo test -p xtask`.
