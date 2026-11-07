//! Application configuration.

pub mod settings;

pub use settings::{
    BotProtectionSettings, CryptoSettings, DatabaseSettings, GoogleOAuthSettings, ServerSettings,
    Settings, SmtpSettings, is_dev_mode,
};
