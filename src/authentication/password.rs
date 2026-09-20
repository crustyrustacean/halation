// src/authentication/password.rs

// dependencies
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

/// Hash a password with argon2id (default OWASP parameters). Returns the
/// PHC-formatted string for storage — salt included, nothing else to keep.
pub fn hash_password(password: &str) -> Result<String, anyhow::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string();
    Ok(password_hash)
}

/// Verify a password against a stored PHC string. Constant-time under the
/// hood (argon2 comparison); returns `Ok(false)` on any mismatch rather
/// than an error, so callers branch on a single value.
pub fn verify_password(password: &str, password_hash: &str) -> Result<bool, anyhow::Error> {
    let parsed = PasswordHash::new(password_hash)?;
    let is_valid = Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok();
    Ok(is_valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_then_verify_round_trips() {
        // Arrange
        let password = "correct horse battery staple";

        // Act
        let hash = hash_password(password).expect("Failed to hash password");
        let is_valid = verify_password(password, &hash).expect("Failed to verify password");

        // Assert
        assert!(is_valid, "the original password should verify");
        assert!(
            hash.starts_with("$argon2id$"),
            "stored hash should be an argon2id PHC string"
        );
    }

    #[tokio::test]
    async fn wrong_password_fails_verification() {
        // Arrange
        let hash = hash_password("correct horse battery staple").unwrap();

        // Act
        let is_valid =
            verify_password("Tr0ub4dor&3", &hash).expect("Verification should not error");

        // Assert
        assert!(!is_valid, "a wrong password must not verify");
    }

    #[tokio::test]
    async fn hashes_are_salted_per_call() {
        // Act
        let first = hash_password("same password").unwrap();
        let second = hash_password("same password").unwrap();

        // Assert — same input, different salts, different stored strings
        assert_ne!(first, second, "hashes must never repeat for equal input");
    }
}
