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

Ensures the dev Postgres container (`halation-db-1`, postgres:17, port 5433)
and the `halation` database exist. `--clean` also drops the throwaway
UUID-named databases `cargo test` leaves behind.

Migrations are deliberately **not** applied here — the app self-migrates on
boot, and applying them twice breaks `cargo run` with
`relation users already exists`.

This replaces both `scripts/init_dev_db.sh` and `scripts/init_db.ps1`, which
had drifted into a genuine footgun: a `.ps1` with no `param()` block silently
discarded `-Clean`, exiting 0 having done nothing at all.

### `deploy [--dry-run] [--yes] [--host=…]`

Prints the droplet plan, and **refuses to run without `--yes`**. The plan
deliberately includes the verification steps that get skipped when you're
tired:

1. build `--platform linux/amd64` (arm64 crash-loops on the x86-64 droplet);
2. transfer the image, then **compare image IDs** across machines;
3. `docker compose up -d`;
4. read the *serving* version from `/api/v1/health` — `compose up -d` with an
   unchanged image is a silent no-op, and the schema can run ahead of the
   container, so "the schema is right" proves nothing;
5. re-verify the image ID on the droplet.

Host defaults to `root@167.99.176.197`, overridable via `--host=` or
`HALATION_DEPLOY_HOST`.

> **Note:** steps are currently *printed*; the live execution path is
> deliberately not wired up yet. Treat `deploy --yes` as a checklist that
> always shows you the plan, and run the commands yourself until it's finished.

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
