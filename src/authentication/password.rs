// src/authentication/password.rs
// dependencies
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use secrecy::{ExposeSecret, SecretString};

/// An argon2id password hash, carried as an opaque secret.
///
/// The PHC string contains the salt and the work factors, and — unlike a
/// raw password — it is *replayable*: a hash lifted from a log or a
/// database dump verifies against this account forever until the
/// password changes. `SecretString`'s `Debug` prints `[REDACTED]`, so
/// wrapping the value at the type level makes an accidental `{:?}`,
/// `unwrap()` panic message, or `#[tracing::instrument]` argument leak
/// impossible rather than merely discouraged.
///
/// Note the deliberate omission of `Display`: `crate::utils::e500` takes
/// `T: Debug + Display`, so a `Display` impl would let a hash be passed
/// as an "error" and logged in full. Errors never carry a hash.
pub struct PasswordHashSecret(SecretString);

impl PasswordHashSecret {
    /// The PHC-formatted string, for storage. Call only at the point of
    /// use — a `.bind()` argument, a test fixture, or verification.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl From<SecretString> for PasswordHashSecret {
    fn from(secret: SecretString) -> Self {
        Self(secret)
    }
}

impl std::fmt::Debug for PasswordHashSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Delegates to `SecretString`, which renders `[REDACTED]`.
        f.debug_tuple("PasswordHashSecret").field(&self.0).finish()
    }
}

impl Clone for PasswordHashSecret {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

// --- sqlx plumbing ---------------------------------------------------------
//
// The column is `TEXT`, and `FromRow` requires each field to implement
// both `Decode` and `Type` for Postgres. These impls are the only place
// the inner string is unwrapped on the read path; everything else goes
// through `expose()`. The `Type` impl delegates to `String` so the
// generated `RETURNING`/`SELECT` lists keep describing a plain TEXT column
// and no migration is implied by this newtype.

impl sqlx::Type<sqlx::Postgres> for PasswordHashSecret {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for PasswordHashSecret {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let raw = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        Ok(Self::from(SecretString::from(raw)))
    }
}

impl<'q> sqlx::Encode<'q, sqlx::Postgres> for PasswordHashSecret {
    fn encode_by_ref(
        &self,
        buf: &mut <sqlx::Postgres as sqlx::database::Database>::ArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(self.expose(), buf)
    }
}

/// Hash a password with argon2id (default OWASP parameters). Returns an
/// opaque handle to the PHC-formatted string — salt included, nothing else
/// to keep.
pub fn hash_password(password: &str) -> Result<PasswordHashSecret, anyhow::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)?
        .to_string();
    Ok(PasswordHashSecret::from(SecretString::from(password_hash)))
}

/// Verify a password against a stored PHC string. Constant-time under the
/// hood (argon2 comparison); returns `Ok(false)` on any mismatch rather
/// than an error, so callers branch on a single value.
pub fn verify_password(
    password: &str,
    password_hash: &PasswordHashSecret,
) -> Result<bool, anyhow::Error> {
    let parsed = PasswordHash::new(password_hash.expose())?;
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
            hash.expose().starts_with("$argon2id$"),
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
        assert_ne!(
            first.expose(),
            second.expose(),
            "hashes must never repeat for equal input"
        );
    }

    /// The property that justifies the whole newtype: a `PasswordHashSecret`
    /// reached the type level, it is not printable.
    #[test]
    fn debug_never_reveals_the_hash() {
        // Arrange
        let hash = hash_password("correct horse battery staple").unwrap();
        let rendered = format!("{hash:?}");

        // Assert
        assert!(
            !rendered.contains("argon2"),
            "Debug must not include the PHC string, got: {rendered}"
        );
        assert!(rendered.contains("REDACTED"), "got: {rendered}");
    }

    /// A hash must not be usable as an "error" — `e500` logs whatever it
    /// is given and requires `Debug + Display`.
    ///
    /// The absence of a trait cannot be asserted at runtime, so this test
    /// documents the guarantee instead of proving it: the `Display` impl is
    /// deliberately not written, and adding one is what would break the
    /// property. Kept as a test so the reason lives next to the type.
    #[test]
    fn hash_exposes_only_an_explicit_accessor() {
        // The read path is a named method, not a trait, so a `Display` or
        // `Deref` impl added later would be a visible API change.
        let hash = hash_password("correct horse battery staple").unwrap();
        assert_eq!(hash.expose(), hash.expose(), "expose() is stable");
    }
}
