#!/usr/bin/env bash
# Create the dev database (if missing) and apply migrations, using the
# dockerized Postgres that base.yaml points at (localhost:5433).
#
# Usage: scripts/init_dev_db.sh [--clean]
#   --clean  also drops throwaway test databases left by `cargo test`
set -euo pipefail

CONTAINER="${CONTAINER:-halation-db-1}"
DB_USER="${DB_USER:-postgres}"
DB_NAME="${DB_NAME:-halation}"
MIGRATIONS_DIR="$(cd "$(dirname "$0")/../migrations" && pwd)"

if ! docker ps --format '{{.Names}}' | grep -qx "$CONTAINER"; then
  echo "Container '$CONTAINER' is not running. Start it with:"
  echo "  docker run -d --name $CONTAINER -e POSTGRES_USER=$DB_USER \\"
  echo "    -e POSTGRES_PASSWORD=password -p 5433:5432 postgres:17"
  exit 1
fi

psql() { docker exec -i "$CONTAINER" psql -U "$DB_USER" "$@"; }

if [ "${1:-}" = "--clean" ]; then
  echo "Dropping throwaway test databases..."
  psql -tAc "SELECT datname FROM pg_database
             WHERE datname ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'" \
    | while read -r db; do
        [ -n "$db" ] && psql -c "DROP DATABASE IF EXISTS \"$db\" WITH (FORCE)" > /dev/null
      done
  echo "Test databases swept."
fi

EXISTS=$(psql -tAc "SELECT 1 FROM pg_database WHERE datname = '$DB_NAME'" | tr -d '[:space:]')
if [ "$EXISTS" != "1" ]; then
  psql -c "CREATE DATABASE $DB_NAME"
  echo "Created database '$DB_NAME'."
fi

# Apply all migrations if the schema isn't there yet (table-presence check;
# migrations applied here are not tracked for sqlx-cli — that's fine, the
# app and the test harness don't use it against this database).
USERS_TABLE=$(psql -d "$DB_NAME" -tAc \
  "SELECT 1 FROM information_schema.tables WHERE table_name = 'users'" | tr -d '[:space:]')
if [ "$USERS_TABLE" = "1" ]; then
  echo "Migrations already applied to '$DB_NAME'."
else
  for file in "$MIGRATIONS_DIR"/*.up.sql; do
    echo "apply  $(basename "$file")"
    psql -v ON_ERROR_STOP=1 -d "$DB_NAME" < "$file" > /dev/null
  done
  echo "Migrations applied."
fi

echo "Dev database '$DB_NAME' is ready on localhost:5433."
