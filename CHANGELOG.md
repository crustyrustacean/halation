# Changelog

All notable changes to the Halation project will be documented in this file.

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
