#![allow(
    clippy::double_must_use,
    reason = "`#[async_trait]` marks its boxed futures `#[must_use]`; not actionable here"
)]

pub mod code_generator;

pub mod email_service;
pub mod email_templates;

pub mod jwt_service;

pub mod mx_validator;

pub mod oauth;
pub mod password_hasher;

pub mod token_service;
pub mod turnstile_service;

pub use code_generator::RandomCodeGenerator;

pub use email_service::{ConsoleEmailService, SmtpEmailService};

pub use jwt_service::CryptoJwtService;

pub use mx_validator::{HickoryMxValidator, NoOpMxValidator};

pub use oauth::GoogleOAuthClient;

pub use password_hasher::Argon2Hasher;

pub use token_service::CryptoTokenService;
pub use turnstile_service::{CloudflareTurnstileService, NoOpTurnstileService};
