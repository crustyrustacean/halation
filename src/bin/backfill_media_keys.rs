// src/bin/backfill_media_keys.rs

//! One-off migration for the per-user storage partitioning (pi-brain
//! `81156523`): media objects written before owner-scoped keys live at
//! `{media_id}/…`; this bin copies each object family to
//! `{owner_id}/{media_id}/…` and repoints the database rows.
//!
//! Guarantees:
//! - **copy-then-update** — objects are copied before any row changes, so a
//!   row's `storage_key` always matches where the bytes physically are;
//! - **legacy objects are left in place** — cleanup is a separate, verified
//!   sweep (R2 dashboard at this scale);
//! - **idempotent** — rows already carrying the owner prefix are skipped, so
//!   the tool is safe to re-run mid-flight;
//! - `--dry-run` prints the plan and touches nothing.
//!
//! Runs inside the app container (which already has the R2 env vars):
//! `docker compose exec app /app/backfill_media_keys --dry-run`

use anyhow::Context;
use halation::configuration::get_configuration;
use halation::services::media::{is_legacy_key, media_key};
use halation::startup::get_connection_pool;
use halation::storage::{InMemoryStorageBackend, OpendalStorageBackend, StorageBackend};
use sqlx::PgPool;
use uuid::Uuid;

/// A media row (plus its derivative rows) still living in the legacy
/// global namespace.
struct PendingMedia {
    media_id: Uuid,
    owner_id: Uuid,
    legacy_original_key: String,
    /// File name of the original ("original.jpg", "original.heic", …) —
    /// preserved so the extension survives the move.
    original_name: String,
    derivatives: Vec<PendingDerivative>,
}

struct PendingDerivative {
    variant: String,
    legacy_key: String,
}

/// Media rows (oldest first) whose original key does not yet carry the
/// owner segment, each with its derivative rows.
async fn find_pending(pool: &PgPool) -> anyhow::Result<Vec<PendingMedia>> {
    let rows: Vec<(Uuid, Uuid, String)> =
        sqlx::query_as("SELECT id, owner_id, storage_key FROM media ORDER BY created_at, id")
            .fetch_all(pool)
            .await
            .context("Failed to list media rows")?;

    let mut pending = Vec::new();
    for (media_id, owner_id, storage_key) in rows {
        if !is_legacy_key(owner_id, &storage_key) {
            continue;
        }
        let original_name = storage_key
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("media {media_id} has an unusable storage key {storage_key:?}")
            })?
            .to_string();

        let derivatives: Vec<(String, String)> =
            sqlx::query_as(
                "SELECT variant, storage_key FROM media_derivatives WHERE media_id = $1 ORDER BY variant",
            )
            .bind(media_id)
            .fetch_all(pool)
            .await
            .context("Failed to list derivative rows")?;

        pending.push(PendingMedia {
            media_id,
            owner_id,
            legacy_original_key: storage_key,
            original_name,
            derivatives: derivatives
                .into_iter()
                .map(|(variant, legacy_key)| PendingDerivative {
                    variant,
                    legacy_key,
                })
                .collect(),
        });
    }
    Ok(pending)
}

/// Copy one object to its new key. Reads the legacy bytes fully first —
/// objects are small (derivatives are capped, originals are photos), and a
/// full read-then-write keeps the failure mode simple.
async fn copy_object(
    storage: &dyn StorageBackend,
    legacy_key: &str,
    new_key: &str,
) -> anyhow::Result<()> {
    let bytes = storage
        .find(legacy_key)
        .await
        .with_context(|| format!("Legacy object {legacy_key} is missing"))?;
    storage
        .save(new_key, bytes)
        .await
        .with_context(|| format!("Failed to write {new_key}"))?;
    Ok(())
}

/// Copy the whole object family for one media row, then repoint its rows in
/// a single transaction. If anything fails before the commit, the rows are
/// untouched and only duplicate objects exist — re-running finishes cleanly.
async fn migrate_row(
    storage: &dyn StorageBackend,
    pool: &PgPool,
    row: &PendingMedia,
) -> anyhow::Result<()> {
    let new_original_key = media_key(row.owner_id, row.media_id, &row.original_name);
    copy_object(storage, &row.legacy_original_key, &new_original_key).await?;

    let mut derivative_updates: Vec<(String, String)> = Vec::with_capacity(row.derivatives.len());
    for derivative in &row.derivatives {
        let new_key = media_key(
            row.owner_id,
            row.media_id,
            &format!("{}.jpg", derivative.variant),
        );
        copy_object(storage, &derivative.legacy_key, &new_key).await?;
        derivative_updates.push((derivative.variant.clone(), new_key));
    }

    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin the repoint transaction")?;
    sqlx::query("UPDATE media SET storage_key = $2 WHERE id = $1")
        .bind(row.media_id)
        .bind(&new_original_key)
        .execute(&mut *tx)
        .await
        .context("Failed to repoint the media row")?;
    for (variant, new_key) in &derivative_updates {
        sqlx::query(
            "UPDATE media_derivatives SET storage_key = $3 WHERE media_id = $1 AND variant = $2",
        )
        .bind(row.media_id)
        .bind(variant)
        .bind(new_key)
        .execute(&mut *tx)
        .await
        .context("Failed to repoint the derivative row")?;
    }
    tx.commit()
        .await
        .context("Failed to commit the repoint transaction")?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dry_run = std::env::args().any(|arg| arg == "--dry-run");

    let configuration = get_configuration().context("Failed to read configuration")?;
    let pool = get_connection_pool(&configuration.database);
    let storage: Box<dyn StorageBackend> = match configuration.storage.backend.as_str() {
        "s3" => Box::new(OpendalStorageBackend::new(&configuration.storage)?),
        _ => Box::new(InMemoryStorageBackend::new()),
    };
    println!("storage backend: {}", configuration.storage.backend);

    let pending = find_pending(&pool).await?;
    if pending.is_empty() {
        println!("nothing to do: every media key is already owner-scoped");
        return Ok(());
    }
    println!("{} media row(s) pending migration", pending.len());

    let mut migrated = 0usize;
    let mut copied = 0usize;
    for row in &pending {
        println!("media {} (owner {}):", row.media_id, row.owner_id);
        println!(
            "  {} -> {}",
            row.legacy_original_key,
            media_key(row.owner_id, row.media_id, &row.original_name)
        );
        for derivative in &row.derivatives {
            println!(
                "  {} -> {}",
                derivative.legacy_key,
                media_key(
                    row.owner_id,
                    row.media_id,
                    &format!("{}.jpg", derivative.variant)
                )
            );
        }
        if dry_run {
            continue;
        }
        migrate_row(storage.as_ref(), &pool, row).await?;
        migrated += 1;
        copied += 1 + row.derivatives.len();
    }

    if dry_run {
        println!(
            "dry run: no changes made ({} row(s) would be migrated)",
            pending.len()
        );
    } else {
        println!(
            "done: {migrated} media row(s) migrated, {copied} object(s) copied; legacy objects left in place"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use secrecy::SecretString;
    use sqlx::{Connection, Executor, PgConnection};

    /// A throwaway database per test, migrated to head — the same pattern
    /// as tests/api/helpers.
    async fn test_pool() -> PgPool {
        let mut configuration = get_configuration().expect("Failed to read configuration");
        configuration.database.database_name = Uuid::new_v4().to_string();

        let maintenance = halation::configuration::DatabaseSettings {
            database_name: "postgres".to_string(),
            username: "postgres".to_string(),
            password: SecretString::new("password".into()),
            ..configuration.database.clone()
        };
        let mut connection = PgConnection::connect_with(&maintenance.connect_options())
            .await
            .expect("Failed to connect to Postgres");
        connection
            .execute(
                format!(
                    r#"CREATE DATABASE "{}";"#,
                    configuration.database.database_name
                )
                .as_str(),
            )
            .await
            .expect("Failed to create the test database");

        let pool = PgPool::connect_with(configuration.database.connect_options())
            .await
            .expect("Failed to connect to the test database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to migrate the test database");
        pool
    }

    /// One user + one media row (original + the three standard derivatives)
    /// in the legacy global namespace, with bytes stored at the legacy keys.
    async fn seed_legacy_media(
        pool: &PgPool,
        storage: &InMemoryStorageBackend,
    ) -> (Uuid, Uuid, Vec<String>) {
        let owner_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (username, email, password_hash) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind("legacyowner")
        .bind("legacy@example.com")
        .bind("not-a-real-hash")
        .fetch_one(pool)
        .await
        .expect("Failed to seed user");

        let media_id = Uuid::new_v4();
        let legacy_original = format!("{media_id}/original.jpg");
        sqlx::query(
            "INSERT INTO media (id, owner_id, storage_key, mime_type, width, height, size_bytes, sha256)
             VALUES ($1, $2, $3, 'image/jpeg', 10, 10, 6, 'deadbeef')",
        )
        .bind(media_id)
        .bind(owner_id)
        .bind(&legacy_original)
        .execute(pool)
        .await
        .expect("Failed to seed media");

        let mut legacy_keys = vec![legacy_original.clone()];
        for variant in ["thumb", "medium", "large"] {
            let legacy_key = format!("{media_id}/{variant}.jpg");
            sqlx::query(
                "INSERT INTO media_derivatives (media_id, variant, storage_key, width, height, size_bytes)
                 VALUES ($1, $2, $3, 10, 10, 6)",
            )
            .bind(media_id)
            .bind(variant)
            .bind(&legacy_key)
            .execute(pool)
            .await
            .expect("Failed to seed derivative");
            legacy_keys.push(legacy_key);
        }

        for key in &legacy_keys {
            storage
                .save(key, Bytes::from_static(b"pixels"))
                .await
                .unwrap();
        }
        (owner_id, media_id, legacy_keys)
    }

    #[tokio::test]
    async fn finds_legacy_rows_only() {
        // Arrange — one legacy row and one already-partitioned row
        let pool = test_pool().await;
        let storage = InMemoryStorageBackend::new();
        let (owner_id, legacy_media_id, _) = seed_legacy_media(&pool, &storage).await;

        let partitioned_media_id = Uuid::new_v4();
        let partitioned_key = media_key(owner_id, partitioned_media_id, "original.jpg");
        sqlx::query(
            "INSERT INTO media (id, owner_id, storage_key, mime_type, width, height, size_bytes, sha256)
             VALUES ($1, $2, $3, 'image/jpeg', 10, 10, 6, 'deadbeef')",
        )
        .bind(partitioned_media_id)
        .bind(owner_id)
        .bind(&partitioned_key)
        .execute(&pool)
        .await
        .unwrap();

        // Act
        let pending = find_pending(&pool).await.unwrap();

        // Assert — only the legacy row is picked up
        assert_eq!(1, pending.len());
        assert_eq!(legacy_media_id, pending[0].media_id);
        assert_eq!(owner_id, pending[0].owner_id);
        assert_eq!(3, pending[0].derivatives.len());
    }

    #[tokio::test]
    async fn migrates_copy_first_and_leaves_legacy_objects_in_place() {
        // Arrange
        let pool = test_pool().await;
        let storage = InMemoryStorageBackend::new();
        let (owner_id, media_id, legacy_keys) = seed_legacy_media(&pool, &storage).await;
        let pending = find_pending(&pool).await.unwrap();
        assert_eq!(1, pending.len());

        // Act
        migrate_row(&storage, &pool, &pending[0]).await.unwrap();

        // Assert — rows repointed to owner-scoped keys
        let original_key: String =
            sqlx::query_scalar("SELECT storage_key FROM media WHERE id = $1")
                .bind(media_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(media_key(owner_id, media_id, "original.jpg"), original_key);
        let derivative_keys: Vec<String> =
            sqlx::query_scalar("SELECT storage_key FROM media_derivatives WHERE media_id = $1")
                .bind(media_id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(3, derivative_keys.len());
        for key in &derivative_keys {
            assert!(
                key.starts_with(&format!("{owner_id}/")),
                "{key} must be owner-scoped"
            );
        }

        // Assert — bytes readable at the new keys…
        assert!(storage.find(&original_key).await.is_ok());
        for key in &derivative_keys {
            assert!(storage.find(key).await.is_ok());
        }
        // …and the legacy objects survived: copy, never move.
        for key in &legacy_keys {
            assert!(
                storage.find(key).await.is_ok(),
                "legacy object {key} must still exist after the backfill"
            );
        }
    }

    #[tokio::test]
    async fn second_run_finds_nothing_pending() {
        // Arrange — migrate once
        let pool = test_pool().await;
        let storage = InMemoryStorageBackend::new();
        let (_, _, _) = seed_legacy_media(&pool, &storage).await;
        for row in find_pending(&pool).await.unwrap() {
            migrate_row(&storage, &pool, &row).await.unwrap();
        }

        // Act
        let pending = find_pending(&pool).await.unwrap();

        // Assert — idempotent: a re-run is a no-op
        assert!(pending.is_empty(), "re-run must find nothing to do");
    }
}
