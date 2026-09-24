# Changelog

All notable changes to the Halation project will be documented in this file.

## [0.13.0] - 2026-09-24

### Added — registration close (shareable portfolio mode)

Registration can be shut while the platform is under active development —
existing members log in unaffected, anonymous visitors can browse.

- `application.registration_open` config flag (default `true`; env
  `APP_APPLICATION__REGISTRATION_OPEN`), `production.yaml` ships `false`
- `GET/POST /register` → 303 `/register/closed` when shut — the POST
  refuses before rate-limit budget or any database work, so curl can't
  create accounts either
- New `/register/closed` page; the nav Register link and the login page's
  sign-up nudge hide while closed (site-wide flags now ride the template
  renderer via `TeraRenderer::with_site` — handlers stay out of it)
- New test module `registration_closed` (open renders, closed redirects +
  hides links, refused POST creates no account); `spawn_app_with` tweaks
  config per test

### Changed — per-user storage partitioning

Media objects are now owner-scoped in object storage. Keys move from the
global `{media_id}/…` namespace to `{owner_id}/{media_id}/…`, making each
user's media a prefix operation (bulk list/delete, audits) and keeping a
literal per-user-bucket backend open as a future option.

- `media_key(owner, media, name)` is the single key construction point
  (`services/media.rs`); `is_legacy_key` detects pre-partitioning keys
- `process_and_store` / `store_derivatives` take the owner and write
  owner-scoped keys — the upload route passes the uploading user, rotate
  passes the media's owner
- **`backfill_media_keys` bin** migrates existing objects: copy-then-update
  (rows repointed in one transaction only after every object is copied),
  legacy objects left in place, idempotent, `--dry-run` supported. Ships in
  the deploy image; on the droplet:
  `docker compose exec app /app/backfill_media_keys --dry-run`
- Dockerfile builds and ships both binaries
- Serving is unchanged: `/media/{media_id}/{variant}` resolves keys through
  the database, so URLs and browser caches are unaffected

83 tests green (37 lib + 40 API + 3 backfill + 3 registration), zero
warnings on `--all-targets`.

## [0.12.0] - 2026-10-01

### Changed — relational storage behind a `DatabaseBackend` trait

Routes talked to `PgPool` directly, with SQL scattered across `posts.rs`,
`follows.rs`, `users.rs`, and inline handler queries. 

- **`database.rs`** — `DatabaseError` (`NotFound`/`Operation`,
  `ResponseError`, mirroring `StorageError`) and the async
  `DatabaseBackend` trait: every relational operation the app performs,
  named in domain terms.
- **`database/postgres.rs`** — `PostgresDatabase { pool }`; every SQL
  statement now lives in this one file.
- **Routes** inject `web::Data<Box<dyn DatabaseBackend>>`; the raw pool
  is no longer app data. The actix session store keeps its pool —
  sessions are infrastructure, not domain storage.
- **Store modules keep the types**: `posts.rs` (cards, views, hashtag
  parsing, `MediaRotation`), `follows.rs` (`AccountSummary`),
  `users.rs` (`User`/`NewUser`/`UserStoreError`). Their free SQL
  functions moved into the trait implementation.
- **Upload pipeline restructured**: every file is decoded and stored
  first, then `create_post_with_media` persists post + hashtags + media
  rows + derivative rows + links + location in one store-side
  transaction — no DB connection is held across image decode or object
  storage I/O anymore.
- **Rotation** persists regenerated derivatives' storage keys alongside
  dimensions and sizes, keeping the database the single source of truth
  for where bytes live (also the hook for the planned per-user storage
  partitioning).
- Removed the superseded `insert_media`/`MediaRow` path from
  `services/media.rs`; tidied five pre-existing warnings in test
  targets (`cargo clippy` without `--all-targets` skips tests).

74 tests green (34 lib + 40 API).

## [0.11.0] - 2026-09-22

### Phase 4 — the social graph

- **Migration**: `follows` (composite PK = idempotent follows, cascade
  delete, both lookup indexes)
- **Follow store** (`follows.rs`): follow/unfollow (idempotent),
  is_following, follower/following counts, follower/following account
  lists (with server-computed avatar initials), user lookup by username
- **Follow/unfollow fragments**: `POST/DELETE
  /fragments/users/{username}/follow` inside the CSRF-guarded scope —
  returns the follow-button partial (Datastar swaps it in place);
  401 anon, 403 without `Datastar-Request`, 404 unknown account,
  400 self-follow; idempotent duplicates
- **Profile page**: follower/following counts (linked to list pages) and
  the follow button (owner and anonymous visitors see none)
- **Follower/following list pages** at
  `/u/{username}/followers` and `/following` — unknown accounts 404
- 74 tests green (34 lib + 40 API), zero warnings. New social suite
  covers auth, CSRF, ownership, idempotency, graph persistence,
  list pages, and the round trip

## [0.10.0] - 2026-09-21

### Added — deployment stack (droplet-ready)

- `docker-compose.yml` is now the deployment unit: app + postgres +
  **Caddy** (automatic HTTPS for `$DOMAIN`). Postgres is reachable only
  inside the compose network; the app publishes to loopback; Caddy owns
  ports 80/443 on the host.
- `Caddyfile`: `{$DOMAIN}` → reverse_proxy app:8000; certificates are
  obtained and renewed automatically once DNS points at the server.
- Required compose variables fail fast with instructions when missing
  (DOMAIN, POSTGRES_PASSWORD, HALATION_SESSION_SIGNING_KEY) — set them
  in `.env` next to the R2 values, and `docker compose up -d`.
- `.env.example` gains the compose section; README deployment section
  reworked. Full runbook lives in the planning doc.

Bump to 0.10.0.

## [0.9.0] - 2026-09-21

### Fixed — R2 configuration never reached OpenDAL

`APP_STORAGE__R2__BUCKET` (and friends) never actually mapped into the
config: the `__` separator produces the nested path `storage.r2.bucket`,
while `StorageSettings` declared flat `r2_bucket` fields — so the bucket
name stayed empty and OpenDAL failed at boot with "The bucket is
misconfigured" (ConfigInvalid). Reproduced, then fixed.

- `StorageSettings` now nests R2 credentials under `r2: R2Settings`,
  matching the `__`-separator env convention exactly:
  `APP_STORAGE__R2__BUCKET / __ENDPOINT / __ACCESS_KEY / __SECRET_KEY`
  (so `.env` files written per the v0.8.1 docs work unchanged)
- `configuration/base.yaml` storage section restructured to the nested
  shape; `fs_root` moved into the R2 group (bucket path prefix)
- serde mapping locked by unit tests: nested env shape → StorageSettings,
  plus defaults when the r2 section is absent entirely
- Deep health honestly reports storage unavailability against fake
  credentials (verified); manual rotate + upload flows unchanged

Bump to 0.9.0 — the storage config surface changed shape.

## [0.8.1] - 2026-09-21

### Fixed — environment loading + secret hygiene

- **`.env` is now loaded**: dotenvy runs first in `main`, so local runs
  pick up `.env` (dev convenience; production containers configure via
  real env vars instead).
- **`.env` untracked from git** (it had been committed with dev-only
  credentials) and added to `.gitignore`; `.env.example` added as the
  full variable map, including the commented Cloudflare R2 block.

## [0.8.0] - 2026-09-21

### Added — production container (DigitalOcean-ready)

- **Multi-stage `Dockerfile`** (cargo-chef pattern, ported from
  metallian-photos): dependency-cached release builds, `libheif-dev` in
  the builder, slim runtime with `libheif1` + ca-certificates + curl,
  non-root `appuser`, `HEALTHCHECK` on `/health_check`
- **`docker-compose.yml`**: local production-like stack (app + postgres:17
  with persistent volume); the app self-creates + self-migrates its
  database on boot
- **`.dockerignore`**: lean build context (no target/, .git, tests, .env)
- Verified: image builds, runs as `appuser`, healthcheck healthy,
  register-through-container persists to Postgres
- Deployment targets: DO App Platform (builds from the Dockerfile) or a
  droplet with docker compose; managed Postgres note: create the
  `halation` database in the control panel (or grant CREATEDB — the app
  self-creates when the user has rights); `APP_DATABASE__REQUIRE_SSL`
  defaults true in production and DO managed Postgres supports it

## [0.7.1] - 2026-09-21

### Fixed — card layout breathing room

- **Permalink page**: the location chip, caption, and tags rendered bare
  inside the card with no padding at all (the `.post-body` wrapper only
  existed on feed cards) — they now share the same padded body block.
- **Feed cards**: bumped the post body's bottom padding (4px → 16px) so
  captions and tags no longer hug the footer divider.
- **EXIF meta rows** now carry horizontal padding on permalinks instead
  of running edge-to-edge.
- **Location chip** now overlays the photo (top-left, translucent navy,
  gold dot) exactly like the mockup, instead of floating orphaned in the
  card flow. Rotate response keeps the buttons alive across replacements
  (context carried `can_rotate` through).

Cosmetic only — all 65 tests pass unchanged.

## [0.7.0] - 2026-09-20

### Added — beauty pass: the design contract lands

- **Full design system** in `screen.css`, ported from the mockup:
  Tumblr-dashboard navy (#001935), white cards with 8px radius and soft
  shadows, gold (#ffb02e) accents, blue links, 604px column
- **Topbar chrome**: sticky translucent navy with blur, gold-accented
  brand, nav (Feed / Recent / Upload / Log out)
- **Post cards** styled like the mockup: white card, 2-up photo sets,
  caption body, tag links, owner + date footer
- **Profile page**: identity card (gradient avatar with initial, display
  name, handle, bio, post count, joined date) over a 3-column thumb grid
- **Forms**: card-wrapped stacked forms (login, register, upload) with
  gold-focus inputs and gold submit buttons; error pills; success notices
- **Location chips** styled (gold dot + navy-tint pill); **load-more**
  pill on navy; styled 404/error cards; footer
- Zero behavior changes — all 65 tests green untouched; the restyle is
  verified by the same page-render tests added in v0.3.1

## [0.6.0] - 2026-09-20

### Phase 3.5 — orientation fix, reverse geocoding, manual rotate

- **EXIF orientation is now applied by the pipeline.** Portrait phone
  photos carry a rotation in their EXIF Orientation tag; the image crate
  does not apply it on decode, and our derivatives are EXIF-stripped —
  so sideways photos were baked in forever. The pipeline now reads the
  tag (kamadak) and bakes the rotation into every derivative.
- **Reverse geocoding**: uploads with GPS EXIF get a location chip
  ("Discovery Park"-style) via Nominatim (OpenStreetMap), best-effort
  with a 3s timeout — offline or disabled simply means no chip.
  Privacy note: this sends coordinates to OSM at upload time; disable
  with `geocode.enabled: false` in config.
- **Manual rotate**: owners get a per-photo rotate button on permalinks
  (owner-only, 403 otherwise; 401 unauthenticated). Rotation re-derives
  all variants from the stored original (EXIF orientation + cumulative
  manual rotation), bumps the media version as a cache buster, and
  patches the page in place via Datastar. Originals are never modified.
- 65 tests green (32 lib incl. hand-built EXIF orientation fixture and
  Nominatim parser; 33 API incl. rotate ownership/version/dimensions,
  unauthenticated 401, location chip rendering).

## [0.5.0] - 2026-09-20

### Phase 3 — the app becomes browsable

- **Migrations**: `hashtags` + `post_hashtags` (CITEXT tags, cascade delete)
- **Hashtag parsing**: `#tag` tokens extracted from captions on post
  creation (lowercase, deduped, checked-constrained at the DB), rendered
  as links on cards and permalinks
- **Feed pages**: `/` (global timeline until follows land in Phase 5) and
  `/recent` — post cards (photo / photo-set / caption / tags / owner /
  date) with **keyset cursor pagination**, never OFFSET
- **Load-more fragment**: `GET /fragments/feed?before={post_id}` — first
  customer of the CSRF-guarded `/fragments` scope; returns a Datastar SSE
  stream of two patches: cards inserted before the button, button
  replaced with the next cursor (or removed when exhausted)
- **Profile pages**: `/u/{username}` — identity card (avatar initial,
  display name, bio, post count, joined date) over a 3-column thumbnail
  grid; backed by the read-only `ProfileView` (no email, no hash);
  unknown accounts get a styled 404
- **Hashtag pages**: `/hashtags/{tag}`
- **EXIF meta row** on permalinks: camera, lens, date, ƒ/, exposure, ISO,
  focal length, GPS decimal degrees — rendered from `media.exif`
- 58 tests green (28 lib incl. hashtag parser + pipeline contracts;
  30 API incl. feed chronology, fragment pagination + exhaustion, hashtag
  flow, profile grid isolation, EXIF meta row via committed fixture)

## [0.4.0] - 2026-09-20

### Added — self-bootstrapping database

- **`cargo run` on a fresh machine now just works**: on boot the app
  creates its database if missing (SQLSTATE 3D000 → CREATE DATABASE via
  the maintenance connection), applies all migrations (sqlx-tracked,
  idempotent), and fails fast with an actionable message when Postgres
  itself is unreachable — no more silent lazy-pool 500s.
- **`scripts/init_dev_db.sh` hardened**: creates or starts the dev
  container, waits for readiness, and — deliberately — no longer applies
  migrations itself. Applying them twice (script + app) broke boot with
  "relation users already exists"; migrations belong to the app now.
- New integration test: building against a nonexistent database
  bootstraps it fully (create + migrate + serve).
- README documents the two-command flow.

## [0.3.1] - 2026-09-20

### Fixed

- **Template rendering on bare GETs**: Tera 2 errors when rendering an
  undefined variable, and `GET /register`, `GET /login` and `GET /upload`
  never rendered in tests (every auth test asserted redirects or error
  paths) — so `{{ username }}`, `{{ email }}` and the unpassed `errors`
  list 500'd every fresh page visit. Templates now carry `default` filters
  and handlers pass complete contexts.
- **Dev database missing**: the `halation` database didn't exist in the
  docker container (tests create throwaway DBs; nothing created the dev
  one). `scripts/init_dev_db.sh` now creates + migrates it (idempotent,
  psql-only — no sqlx-cli), with `--clean` to sweep throwaway test
  databases. README documents the flow; `.env` aligned to `halation`.
- 48 tests green (5 new page-render tests covering every route and state:
  fresh login/register pages, registered notice, upload page logged in,
  permalink without caption). Verified end-to-end via curl: register →
  login → upload → permalink → derivative serving.

## [0.3.0] - 2026-09-20

### Phase 2 — Media pipeline + MVP composer (login → upload → see your photo)

- **Migrations**: `media` (owner, kind, storage_key, mime, dimensions,
  size, sha256, `exif` JSONB) + `media_derivatives` (thumb/medium/large) +
  `posts` + `post_media` (photo sets, positions preserved)
- **Media pipeline** (`services::media`): sniff (JPEG/PNG/WebP native, HEIC
  via `heic` crate conversion), decode, EXIF extraction (Make/Model/Lens,
  DateTimeOriginal, ƒ/, exposure, ISO, focal length, GPS → decimal) into
  `media.exif` JSONB, sha256, derivatives at JPEG q85 — thumb 300×300 cover
  crop, medium 640 / large 1080 long-edge caps with no upscaling; every
  derivative is EXIF-free by construction (privacy by pipeline)
- **Storage keys**: `StorageBackend` evolved from Uuid keys to path keys
  (`{media_id}/original.{ext}`, `{media_id}/{variant}.jpg`); contract tests
  follow
- **Composer** (`GET/POST /upload`, auth-gated): multipart, up to 4 images
  + caption, 50 MB request cap, per-transaction media + post persistence
- **Permalink** (`GET /p/{post_id}`): photo set (2-up grid, mockup-style)
  + caption + owner + date
- **Serving** (`GET /media/{id}/{variant}`): public thumb/medium/large with
  immutable cache headers; `original` is never publicly served (it carries
  EXIF/GPS) — 404 by design
- 43 tests green (25 lib incl. HEIC fixture + hand-built EXIF segment +
  derivative contracts; 18 API incl. upload round-trip, auth gates, 422
  rejection, original-never-served, photo sets)

## [0.2.0] - 2026-09-20

### Phase 1 — Authentication

- **Registration** (`GET/POST /register`): open registration, classic form POST →
  redirect (no-JS friendly); username `[a-z0-9_]{3,30}` (CHECK-constrained),
  unique email, argon2id password hashing; duplicate handle/email → 409,
  validation failures → 422 with inline errors
- **Login/Logout** (`GET/POST /login`, `POST /logout`): username-or-email
  identifier; constant-time argon2id verification with a single generic
  failure message; logout purges the server-side session record
- **Server-side sessions**: custom Postgres `SessionStore` impl for
  `actix-session` (save/load/update/update_ttl/delete over the `sessions`
  table); actix state stored as JSONB alongside Halation metadata (user_id,
  user_agent, ip, sliding 7-day TTL); expired rows swept on load
- **Middleware**: `IdentityMiddleware` + `SessionMiddleware` wired in
  `startup.rs`, `/health_check` untouched by session work
- **CSRF**: `require_datastar_request_header` guard middleware for the future
  `/fragments` scope — mutations without the header get 403
- **Rate limiting**: flux-limiter (GCRA) on login (per IP + identifier) and
  registration (per IP); denials carry `Retry-After`
- **Migrations**: `users` (CITEXT handles, partial unique indexes so deleted
  accounts recycle their names) and `sessions` (nullable user_id for
  anonymous sessions)
- 30 tests green: 13 lib (password, guard, rate limiter, mailer, storage
  contract, template) + 13 API integration (full register → login →
  authenticated page → logout → revocation lifecycle, rate limiting, session
  store round-trip incl. expiry sweep) + 4 carried from Phase 0

## [0.1.0] - 2026-09-20

### Phase 0 — Scaffold

- **Seeded from `actix-web-sqlx-starter` v0.2.1**
  - bin/lib split, configuration layering, telemetry, test harness
  - Postgres via sqlx (per-test throwaway databases)
- **Ported from `metallian-photos`**
  - `StorageBackend` trait: in-memory fake with contract tests, OpenDAL S3/R2 impl
  - `TemplateRenderer` trait + Tera implementation
  - vendored Datastar bundle (`static/datastar.js`) — server-rendered hypermedia footing
- **New**
  - `Mailer` trait with `NoopMailer` stub (Mailtrap integration lands later)
  - `/health_check` (liveness, dependency-free) and `/api/v1/health` (deep: database + storage)
  - placeholder home page and error page
