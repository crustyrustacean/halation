# Changelog

All notable changes to the Halation project will be documented in this file.

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
