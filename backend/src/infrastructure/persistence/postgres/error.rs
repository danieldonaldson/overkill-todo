//! Shared mapping from `sqlx` errors to [`DomainError`].
//!
//! Repositories funnel their query errors through [`persist_err`] instead of
//! `DomainError::Persistence(e.to_string())` so that the structured information a
//! stringified error would discard — the SQLSTATE class and the missing-row signal —
//! survives into the domain layer. A unique-violation (SQLSTATE `23505`) becomes a
//! [`DomainError::Conflict`] and a missing row becomes a [`DomainError::NotFound`],
//! both of which the HTTP layer maps to the correct status (409 / 404) rather than a
//! blanket 500.
//!
//! Kept here in the persistence module (rather than as a `From<sqlx::Error>` impl on
//! `DomainError`) so the domain layer keeps no dependency on `sqlx`.

use crate::domain::errors::DomainError;

/// Maps a [`sqlx::Error`] to a [`DomainError`], preserving constraint classification.
///
/// - `RowNotFound` → [`DomainError::NotFound`]
/// - unique violation (`23505`) → [`DomainError::Conflict`]
/// - everything else → [`DomainError::Persistence`] (stringified)
pub(crate) fn persist_err(error: sqlx::Error) -> DomainError {
    match error {
        sqlx::Error::RowNotFound => DomainError::NotFound("Resource not found".into()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            DomainError::Conflict("Resource already exists".into())
        }
        other => DomainError::Persistence(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_not_found_maps_to_not_found() {
        let err = persist_err(sqlx::Error::RowNotFound);
        assert!(matches!(err, DomainError::NotFound(_)));
    }

    #[test]
    fn protocol_error_maps_to_persistence() {
        let err = persist_err(sqlx::Error::Protocol("boom".into()));
        assert!(matches!(err, DomainError::Persistence(_)));
    }
}
