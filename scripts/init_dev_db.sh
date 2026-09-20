#!/usr/bin/env bash
# Dev database bootstrap: ensures the dockerized Postgres is running,
# creates the dev database (if missing), and applies migrations.
#
# Usage: scripts/init_dev_db.sh [--clean]
#   --clean  also drops throwaway test databases left by `cargo test`

set -euo pipefail

CONTAINER="${CONTAINER:-halation-db-1}"
DB_USER="${DB_USER:-postgres}"
DB_PASSWORD="${DB_PASSWORD:-password}"
DB_PORT="${DB_PORT:-5433}"
DB_NAME="${DB_NAME:-halation}"

# --- container: running -> ok; stopped -> start; missing -> create ---
if docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
  echo "Container '$CONTAINER' is running."
elif docker ps -a --format '{{.Names}}' | grep -qx "$CONTAINER"; then
  echo "Starting existing container '$CONTAINER'..."
  docker start "$CONTAINER" > /dev/null
else
  echo "Creating container '$CONTAINER'..."
  docker run -d --name "$CONTAINER" \
    -e POSTGRES_USER="$DB_USER" -e POSTGRES_PASSWORD="$DB_PASSWORD" \
    -p "$DB_PORT":5432 postgres:17 > /dev/null
fi

until docker exec "$CONTAINER" pg_isready -U "$DB_USER" > /dev/null 2>&1; do
  echo "Waiting for Postgres..."
  sleep 0.5
done

psql() { docker exec -i "$CONTAINER" psql -U "$DB_USER" "$@"; }

# --- optional sweep of throwaway test databases ---
if [ "${1:-}" = "--clean" ]; then
  echo "Dropping throwaway test databases..."
  psql -tAc "SELECT datname FROM pg_database
             WHERE datname ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'" \
    | while read -r db; do
        [ -n "$db" ] && psql -c "DROP DATABASE IF EXISTS \"$db\" WITH (FORCE)" > /dev/null
      done
  echo "Test databases swept."
fi

# --- database ---
# Migrations are deliberately NOT applied here: the app migrates itself on
# boot (sqlx-tracked). Applying them twice would break `cargo run`.
EXISTS=$(psql -tAc "SELECT 1 FROM pg_database WHERE datname = '$DB_NAME'" | tr -d '[:space:]')
if [ "$EXISTS" != "1" ]; then
  psql -c "CREATE DATABASE $DB_NAME"
  echo "Created database '$DB_NAME'."
fi

echo "Database '$DB_NAME' is ready on localhost:$DB_PORT."
echo "The app applies migrations on boot — just cargo run."
