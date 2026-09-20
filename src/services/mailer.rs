// src/services/mailer.rs

// dependencies
use anyhow::anyhow;
use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MailError {
    #[error("email delivery is not configured")]
    NotConfigured,

    #[error(transparent)]
    Operation(#[from] anyhow::Error),
}

/// Delivery-agnostic email seam. v1 ships `NoopMailer`; the Mailtrap/SMTP
/// implementation slots in behind this trait later without touching callers.
#[async_trait]
pub trait Mailer: Send + Sync {
    async fn send(&self, to: &str, subject: &str, body: &str) -> Result<(), MailError>;
}

/// Logs the message and reports success. Never delivers anything.
pub struct NoopMailer;

#[async_trait]
impl Mailer for NoopMailer {
    async fn send(&self, to: &str, subject: &str, _body: &str) -> Result<(), MailError> {
        if to.is_empty() {
            return Err(MailError::Operation(anyhow!("recipient address is empty")));
        }
        tracing::debug!(to, subject, "noop mailer: message discarded");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn noop_mailer_accepts_a_message() {
        // Arrange
        let mailer = NoopMailer;

        // Act
        let result = mailer.send("jeff@example.com", "Hello", "Body").await;

        // Assert
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn noop_mailer_rejects_an_empty_recipient() {
        // Arrange
        let mailer = NoopMailer;

        // Act
        let result = mailer.send("", "Hello", "Body").await;

        // Assert
        assert!(matches!(result, Err(MailError::Operation(_))));
    }
}
