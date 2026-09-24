# syntax=docker/dockerfile:1

# ---- Stage 1: Chef — dependency caching ----
FROM rust:bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

# ---- Stage 2: Planner — extract dependency info from Cargo.toml/lock ----
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- Stage 3: Builder — compile dependencies, then the app ----
FROM chef AS builder

# libheif for the heic crate (iPhone photo support)
RUN apt-get update && apt-get install -y libheif-dev && rm -rf /var/lib/apt/lists/*

# Compile dependencies first (cached layer)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Now the actual source
COPY . .
RUN cargo build --release --bin halation --bin backfill_media_keys

# ---- Stage 4: Runtime — slim image with only what's needed to run ----
FROM debian:bookworm-slim AS runtime

# libheif runtime lib + ca-certificates (Nominatim/DB TLS) + curl (healthcheck)
RUN apt-get update && apt-get install -y --no-install-recommends \
    libheif1 ca-certificates curl && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/halation /app/halation
COPY --from=builder /app/target/release/backfill_media_keys /app/backfill_media_keys
COPY --from=builder /app/templates /app/templates
COPY --from=builder /app/static /app/static
COPY --from=builder /app/configuration /app/configuration

# Non-root
RUN useradd -r -u 1001 appuser && chown -R appuser:appuser /app
USER appuser

EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -sf http://127.0.0.1:8000/health_check || exit 1

ENTRYPOINT ["/app/halation"]
