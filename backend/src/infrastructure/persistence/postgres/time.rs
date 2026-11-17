//! Timestamp conversions at the repository boundary.
//!
//! sqlx has no jiff support of its own, so `sqlx.toml` maps every `TIMESTAMPTZ`
//! column and parameter to [`jiff_sqlx::Timestamp`]. Domain code holds
//! [`jiff::Timestamp`]; these convert either way, `Option`s included.

/// Converts a domain timestamp (or `Option` of one) to what sqlx binds.
pub trait ToDb {
    /// The `jiff_sqlx` form.
    type Db;
    /// Wraps for sqlx.
    fn to_db(self) -> Self::Db;
}

impl ToDb for jiff::Timestamp {
    type Db = jiff_sqlx::Timestamp;
    fn to_db(self) -> Self::Db {
        self.into()
    }
}

impl ToDb for Option<jiff::Timestamp> {
    type Db = Option<jiff_sqlx::Timestamp>;
    fn to_db(self) -> Self::Db {
        self.map(Into::into)
    }
}

/// Converts a decoded column (or `Option` of one) back to a domain timestamp.
pub trait ToDomain {
    /// The `jiff` form.
    type Domain;
    /// Unwraps from sqlx.
    fn to_domain(self) -> Self::Domain;
}

impl ToDomain for jiff_sqlx::Timestamp {
    type Domain = jiff::Timestamp;
    fn to_domain(self) -> Self::Domain {
        self.to_jiff()
    }
}

impl ToDomain for Option<jiff_sqlx::Timestamp> {
    type Domain = Option<jiff::Timestamp>;
    fn to_domain(self) -> Self::Domain {
        self.map(jiff_sqlx::Timestamp::to_jiff)
    }
}
