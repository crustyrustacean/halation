# Changelog

All notable changes to the Halation project will be documented in this file.

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
