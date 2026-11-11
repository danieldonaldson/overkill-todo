//! Application configuration.

pub mod settings;

pub use settings::{
    BotProtectionSettings, CryptoSettings, DatabaseSettings, GoogleOAuthSettings, ServerSettings,
    Settings, SmtpSettings, TelemetrySettings, is_dev_mode,
};
