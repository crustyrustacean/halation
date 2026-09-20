// src/storage.rs

use crate::utils::error_chain_fmt;
use actix_web::{ResponseError, http::StatusCode};
use async_trait::async_trait;
use bytes::Bytes;
use thiserror::Error;
use uuid::Uuid;

mod in_memory;
mod opendal;

pub use in_memory::*;
pub use opendal::*;

#[derive(Error)]
pub enum StorageError {
    #[error("File not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Operation(#[from] anyhow::Error),
}

impl std::fmt::Debug for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

impl ResponseError for StorageError {
    fn status_code(&self) -> StatusCode {
        match self {
            StorageError::NotFound(_) => StatusCode::NOT_FOUND,
            StorageError::Operation(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn delete(&self, id: Uuid) -> Result<(), StorageError>;
    async fn find(&self, id: Uuid) -> Result<Bytes, StorageError>;
    async fn save(&self, id: Uuid, bytes: Bytes) -> Result<(), StorageError>;

    /// Cheap connectivity probe for the deep health endpoint. The default
    /// suits backends with no remote failure mode (the in-memory fake);
    /// remote backends override it with a real round-trip. A `NotFound`
    /// from a probe counts as healthy — it proves the backend answered.
    async fn health_check(&self) -> Result<(), StorageError> {
        Ok(())
    }
}

#[cfg(test)]
pub mod contract {
    //! Shared behavioral contract for any `StorageBackend` implementation.
    //!
    //! Each function exercises a guarantee that *every* backend must honor —
    //! the in-memory fake today, the OpenDAL S3/R2 impl in production. Writing
    //! the assertions once here avoids duplicate logic drifting across impls.

    use super::*;

    pub async fn save_then_find_round_trips<S: StorageBackend>(storage: &S) {
        // Arrange — a fresh id and some arbitrary bytes
        let id = Uuid::new_v4();
        let bytes = Bytes::from_static(&[1, 2, 3, 4]);

        // Act — save then read back
        storage.save(id, bytes.clone()).await.unwrap();
        let found = storage.find(id).await.unwrap();

        // Assert — the bytes round-trip intact
        assert_eq!(found, bytes);
    }

    pub async fn find_on_missing_id_returns_not_found<S: StorageBackend>(storage: &S) {
        // Arrange — a fresh id that was never saved
        let missing_id = Uuid::new_v4();

        // Act — attempt to find it
        let result = storage.find(missing_id).await;

        // Assert — the backend reports NotFound, not a generic error
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    pub async fn delete_removes_stored_bytes<S: StorageBackend>(storage: &S) {
        // Arrange — save some bytes so there is something to delete
        let id = Uuid::new_v4();
        storage.save(id, Bytes::from_static(&[1])).await.unwrap();

        // Act — delete, then attempt to find
        storage.delete(id).await.unwrap();
        let result = storage.find(id).await;

        // Assert — the bytes are gone
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    pub async fn delete_on_missing_id_is_idempotent<S: StorageBackend>(storage: &S) {
        // Arrange — a fresh id that was never saved
        let missing_id = Uuid::new_v4();

        // Act — delete it anyway
        let result = storage.delete(missing_id).await;

        // Assert — the backend does not treat a missing key as an error
        assert!(result.is_ok());
    }
}
