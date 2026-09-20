// src/storage/opendal.rs

use crate::{
    configuration::StorageSettings,
    storage::{StorageBackend, StorageError},
};
use anyhow::Context;
use async_trait::async_trait;
use bytes::Bytes;
use opendal::{ErrorKind, Operator, Result, services::S3};
use secrecy::ExposeSecret;
use uuid::Uuid;

pub struct OpendalStorageBackend {
    op: Operator,
}

impl OpendalStorageBackend {
    pub fn new(config: &StorageSettings) -> Result<Self> {
        let builder = S3::default()
            .root(&config.fs_root)
            .bucket(&config.r2_bucket)
            .region("auto")
            .endpoint(&config.r2_endpoint)
            .access_key_id(config.r2_access_key.expose_secret())
            .secret_access_key(config.r2_secret_key.expose_secret());

        let op: Operator = Operator::new(builder)?;

        Ok(Self { op })
    }
}

#[async_trait]
impl StorageBackend for OpendalStorageBackend {
    async fn delete(&self, id: Uuid) -> Result<(), StorageError> {
        match self.op.delete(&id.to_string()).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::Operation(e.into())),
        }
    }

    async fn find(&self, id: Uuid) -> Result<Bytes, StorageError> {
        let buffer = self.op.read(&id.to_string()).await.map_err(|e| {
            if e.kind() == opendal::ErrorKind::NotFound {
                StorageError::NotFound(id.to_string())
            } else {
                StorageError::Operation(e.into())
            }
        })?;

        Ok(buffer.to_bytes())
    }

    async fn save(&self, id: Uuid, bytes: Bytes) -> Result<(), StorageError> {
        self.op
            .write(&id.to_string(), bytes)
            .await
            .context("Unable to save the media.")?;

        Ok(())
    }

    /// A stat against a key we never write proves connectivity and auth:
    /// `NotFound` means the backend answered; anything else is a failure.
    async fn health_check(&self) -> Result<(), StorageError> {
        match self.op.stat("halation-health-probe").await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StorageError::Operation(e.into())),
        }
    }
}
