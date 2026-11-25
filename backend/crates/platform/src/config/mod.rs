//! Application configuration.

pub mod settings;

pub use settings::{
    BotProtectionSettings, CryptoSettings, DEFAULT_MAX_EVENTS_PER_SEC, DatabaseSettings,
    GoogleOAuthSettings, ServerSettings, Settings, SmtpSettings, TelemetrySettings, is_dev_mode,
};
