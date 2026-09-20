// src/storage/in_memory.rs

use crate::storage::{StorageBackend, StorageError};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

pub struct InMemoryStorageBackend {
    store: Mutex<HashMap<Uuid, Vec<u8>>>,
}

impl Default for InMemoryStorageBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryStorageBackend {
    pub fn new() -> Self {
        let store = Mutex::new(HashMap::new());

        Self { store }
    }
}

#[async_trait]
impl StorageBackend for InMemoryStorageBackend {
    async fn delete(&self, id: Uuid) -> Result<(), StorageError> {
        let mut locked_store = self
            .store
            .lock()
            .map_err(|e| StorageError::Operation(anyhow::anyhow!("Mutex poisoned: {e}")))?;
        let _ = locked_store.remove(&id);

        Ok(())
    }

    async fn find(&self, id: Uuid) -> Result<Bytes, StorageError> {
        let locked_store = self
            .store
            .lock()
            .map_err(|e| StorageError::Operation(anyhow::anyhow!("Mutex poisoned: {e}")))?;
        let result = locked_store.get(&id);

        match result.ok_or_else(|| StorageError::NotFound("No value with that key.".to_string())) {
            Ok(b) => Ok(Bytes::copy_from_slice(b.as_slice())),
            Err(e) => Err(e),
        }
    }

    async fn save(&self, id: Uuid, bytes: Bytes) -> Result<(), StorageError> {
        let mut locked_store = self
            .store
            .lock()
            .map_err(|e| StorageError::Operation(anyhow::anyhow!("Mutex poisoned: {e}")))?;
        let _ = locked_store.insert(id, bytes.to_vec());

        Ok(())
    }

    // health_check: the trait default (Ok) is correct here — there is no
    // remote dependency to probe.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::contract;

    // Each test constructs the in-memory fake and delegates the assertions
    // to the shared contract functions in `storage::contract`. The same
    // contract functions exercise the OpenDAL impl, so every backend is
    // held to the same behavioral guarantees.

    #[tokio::test]
    async fn in_memory_save_then_find_round_trips() {
        contract::save_then_find_round_trips(&InMemoryStorageBackend::new()).await;
    }

    #[tokio::test]
    async fn in_memory_find_on_missing_id_returns_not_found() {
        contract::find_on_missing_id_returns_not_found(&InMemoryStorageBackend::new()).await;
    }

    #[tokio::test]
    async fn in_memory_delete_removes_stored_bytes() {
        contract::delete_removes_stored_bytes(&InMemoryStorageBackend::new()).await;
    }

    #[tokio::test]
    async fn in_memory_delete_on_missing_id_is_idempotent() {
        contract::delete_on_missing_id_is_idempotent(&InMemoryStorageBackend::new()).await;
    }

    // NOTE on `save`: there is no meaningful unhappy path for the in-memory
    // backend. `HashMap::insert` cannot fail (barring allocation), and the
    // only `Err` case — a poisoned mutex — requires a panic mid-mutation,
    // which can't be triggered without deliberately corrupting state. The
    // OpenDAL backend exercises real failure modes (network, auth); this
    // backend's value is determinism, not failure coverage.
}
