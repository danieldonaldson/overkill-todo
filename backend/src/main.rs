use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use app::{
    application::{
        ports::{
            AccountLifecycleRepository, EmailService, MxValidator, OAuthAccountRepository,
            OAuthProviderClient, TurnstileService,
        },
        use_cases::{
            CancelAccountDeletionUseCase, ChangePasswordUseCase, CompleteSignupUseCase,
            ExportUserDataUseCase, GetCurrentUserUseCase, ListUsersUseCase, LoginUserUseCase,
            OAuthCallbackUseCase, OAuthUnlinkUseCase, PurgeScheduledAccountsUseCase,
            RefreshTokenUseCase, RegisterUserUseCase, RequestAccountDeletionUseCase,
            RequestPasswordResetUseCase, ResendVerificationUseCase, ResetPasswordUseCase,
            RevokeTokenUseCase, SendVerificationCodeUseCase, SetPasswordUseCase,
            SetUserRoleUseCase, ValidateResetCodeUseCase, VerifyEmailUseCase,
            VerifyMagicLinkUseCase,
        },
    },
    domain::entities::OAuthProvider,
    infrastructure::{
        api::{
            RouterDeps, create_router,
            handlers::{
                admin_handler::AdminHandlerDeps, auth_handler::AuthHandlerDeps,
                oauth_handler::OAuthHandlerDeps, profile_handler::ProfileHandlerDeps,
                verification_handler::VerificationHandlerDeps,
            },
        },
        config::Settings,
        oauth::GoogleOAuthClient,
        persistence::{
            Database, PostgresEmailVerificationRepository, PostgresOAuthAccountRepository,
            PostgresPasswordResetRepository, PostgresRefreshTokenRepository,
            PostgresUserRepository, run_migrations,
        },
        services::{
            Argon2Hasher, CloudflareTurnstileService, ConsoleEmailService, CryptoJwtService,
            CryptoTokenService, HickoryMxValidator, NoOpMxValidator, NoOpTurnstileService,
            RandomCodeGenerator, SmtpEmailService,
        },
    },
    shared::crypto::Crypto,
};

/// Use mimalloc (secure mode) as the global allocator. Reduces fragmentation
/// under the server's long-lived async workload and hardens allocator metadata.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Dependency-free liveness probe used by the container `HEALTHCHECK`.
///
/// The production image is `FROM scratch` — no shell, no curl — so the binary
/// probes itself: a raw HTTP/1.0 GET to `/health` on the loopback port. Exits 0
/// on a `200`, non-zero otherwise. Mirrors `/health` (liveness, not readiness).
fn run_healthcheck() -> std::process::ExitCode {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let port = std::env::var("SERVER_PORT").unwrap_or_else(|_| "4322".to_string());
    let addr = format!("127.0.0.1:{port}");
    let timeout = Duration::from_secs(5);

    let result = (|| -> std::io::Result<bool> {
        let socket = addr.parse().map_err(std::io::Error::other)?;
        let mut stream = TcpStream::connect_timeout(&socket, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream
            .write_all(b"GET /health HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response.starts_with("HTTP/1.0 200") || response.starts_with("HTTP/1.1 200"))
    })();

    match result {
        Ok(true) => std::process::ExitCode::SUCCESS,
        Ok(false) => std::process::ExitCode::FAILURE,
        Err(e) => {
            eprintln!("healthcheck failed: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<std::process::ExitCode> {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return Ok(run_healthcheck());
    }

    let settings = Settings::from_env()?;

    // Build the crypto handle from validated settings and inject it (as
    // `Arc<Crypto>`) into the adapters that need it — no global singleton.
    let crypto = Arc::new(
        Crypto::from_settings(&settings.crypto)
            .map_err(|e| anyhow::anyhow!("Crypto initialization failed: {e}"))?,
    );

    assert!(
        std::env::var("TEST_OAUTH_BYPASS").is_err() || settings.server.dev_mode,
        "TEST_OAUTH_BYPASS is set but DEV_MODE=false — this bypass must not run in production"
    );

    // Default verbosity follows the environment: debug in dev, info in prod.
    // Always overridable via RUST_LOG.
    let default_filter = if settings.server.dev_mode {
        "app=debug,tower_http=debug"
    } else {
        "app=info,tower_http=info"
    };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter.parse().unwrap());

    if settings.server.dev_mode {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json())
            .init();
    }

    tracing::info!(
        "Starting {} on {}",
        settings.server.app_name,
        settings.server.address()
    );

    // Database
    let db = Database::new(&settings.database.url, settings.database.max_connections).await?;
    run_migrations(db.pool()).await?;
    tracing::info!("Database migrations applied");

    let db = Arc::new(db);

    // Services
    let code_generator = Arc::new(if settings.server.dev_mode {
        RandomCodeGenerator::development()
    } else {
        RandomCodeGenerator::production()
    });
    let email_service: Arc<dyn EmailService> = if let (
        Some(host),
        Some(username),
        Some(password),
        Some(from),
    ) = (
        settings.smtp.host.as_deref(),
        settings.smtp.username.as_deref(),
        settings.smtp.password.as_deref(),
        settings.smtp.from_address.as_deref(),
    ) {
        tracing::info!("SMTP email enabled (host={})", host);
        Arc::new(
            SmtpEmailService::new(
                host,
                settings.smtp.port,
                username.to_string(),
                password.to_string(),
                from,
                settings.smtp.from_name.as_deref(),
                settings.server.app_name.clone(),
            )
            .map_err(|e| anyhow::anyhow!("Failed to configure SMTP: {e}"))?,
        )
    } else if settings.server.dev_mode {
        tracing::warn!("SMTP not configured — emails printed to console only");
        Arc::new(ConsoleEmailService::new(settings.server.app_name.clone()))
    } else {
        anyhow::bail!(
            "SMTP is required in production. Set SMTP_HOST, SMTP_USERNAME, SMTP_PASSWORD, and SMTP_FROM_ADDRESS."
        );
    };
    let jwt_service = Arc::new(CryptoJwtService::new(crypto.clone()));
    let password_hasher = Arc::new(Argon2Hasher::new(crypto.clone()));
    let token_service = Arc::new(CryptoTokenService::new(crypto.clone()));
    let turnstile_service: Arc<dyn TurnstileService> = if let Some(secret) =
        settings.bot_protection.turnstile_secret_key.clone()
    {
        tracing::info!("Turnstile enabled");
        Arc::new(CloudflareTurnstileService::new(secret))
    } else if settings.server.dev_mode {
        tracing::warn!("TURNSTILE_SECRET_KEY not set — bot protection disabled");
        Arc::new(NoOpTurnstileService) as Arc<dyn TurnstileService>
    } else {
        anyhow::bail!(
            "TURNSTILE_SECRET_KEY is required in production. Set it to your Cloudflare Turnstile secret key."
        );
    };

    let mx_validator: Arc<dyn MxValidator> = if settings.bot_protection.mx_validation_enabled {
        tracing::info!("MX validation enabled");
        Arc::new(
            HickoryMxValidator::new()
                .map_err(|e| anyhow::anyhow!("Failed to initialise DNS resolver: {e}"))?,
        )
    } else {
        tracing::warn!("MX_VALIDATION_ENABLED not set — MX validation disabled");
        Arc::new(NoOpMxValidator)
    };

    // Repositories
    let user_repo = Arc::new(PostgresUserRepository::new(db.clone()));
    let oauth_account_repo: Arc<dyn OAuthAccountRepository> =
        Arc::new(PostgresOAuthAccountRepository::new(db.clone()));
    let refresh_token_repo = Arc::new(PostgresRefreshTokenRepository::new(db.clone()));
    let email_verification_repo = Arc::new(PostgresEmailVerificationRepository::new(db.clone()));
    let password_reset_repo = Arc::new(PostgresPasswordResetRepository::new(db.clone()));

    // Use cases
    let register_uc = Arc::new(RegisterUserUseCase::new(user_repo.clone()));
    let login_uc = Arc::new(LoginUserUseCase::new(user_repo.clone()));
    let get_current_user_uc = Arc::new(GetCurrentUserUseCase::new(user_repo.clone()));
    let refresh_token_uc = Arc::new(RefreshTokenUseCase::new(
        refresh_token_repo.clone(),
        user_repo.clone(),
        token_service.clone(),
    ));
    let revoke_token_uc = Arc::new(RevokeTokenUseCase::new(
        refresh_token_repo.clone(),
        token_service.clone(),
    ));
    let send_verification_uc = Arc::new(SendVerificationCodeUseCase::new(
        email_verification_repo.clone(),
        email_service.clone(),
        code_generator.clone(),
        password_hasher.clone(),
        settings.server.frontend_base_url.clone(),
    ));
    let verify_email_uc = Arc::new(VerifyEmailUseCase::new(
        user_repo.clone(),
        email_verification_repo.clone(),
        password_hasher.clone(),
    ));
    let verify_magic_link_uc = Arc::new(VerifyMagicLinkUseCase::new(
        user_repo.clone(),
        email_verification_repo.clone(),
    ));
    let resend_verification_uc = Arc::new(ResendVerificationUseCase::new(
        user_repo.clone(),
        email_verification_repo.clone(),
        email_service.clone(),
        code_generator.clone(),
        password_hasher.clone(),
    ));
    let complete_signup_uc = Arc::new(CompleteSignupUseCase::new(
        user_repo.clone(),
        jwt_service.clone(),
    ));
    let request_password_reset_uc = Arc::new(RequestPasswordResetUseCase::new(
        user_repo.clone(),
        password_reset_repo.clone(),
        email_service.clone(),
        code_generator.clone(),
        password_hasher.clone(),
    ));
    let validate_reset_code_uc = Arc::new(ValidateResetCodeUseCase::new(
        user_repo.clone(),
        password_reset_repo.clone(),
        password_hasher.clone(),
        jwt_service.clone(),
    ));
    let reset_password_uc = Arc::new(ResetPasswordUseCase::new(
        user_repo.clone(),
        password_reset_repo.clone(),
        refresh_token_repo.clone(),
        jwt_service.clone(),
    ));
    let oauth_callback_uc = Arc::new(OAuthCallbackUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
    ));
    let oauth_unlink_uc = Arc::new(OAuthUnlinkUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
    ));
    let change_password_uc = Arc::new(ChangePasswordUseCase::new(
        user_repo.clone(),
        password_hasher.clone(),
        refresh_token_repo.clone(),
        token_service.clone(),
    ));
    let set_password_uc = Arc::new(SetPasswordUseCase::new(
        user_repo.clone(),
        password_hasher.clone(),
    ));
    let export_user_data_uc = Arc::new(ExportUserDataUseCase::new(
        user_repo.clone(),
        oauth_account_repo.clone(),
    ));
    let request_account_deletion_uc = Arc::new(RequestAccountDeletionUseCase::new(
        user_repo.clone(),
        password_hasher.clone(),
    ));
    let cancel_account_deletion_uc = Arc::new(CancelAccountDeletionUseCase::new(user_repo.clone()));
    // Finalizes deletion for accounts past their 14-day grace window.
    let account_lifecycle_repo: Arc<dyn AccountLifecycleRepository> = user_repo.clone();
    let purge_accounts_uc = Arc::new(PurgeScheduledAccountsUseCase::new(account_lifecycle_repo));

    // OAuth providers, registered by provider in the dispatch map.
    let mut oauth_clients: HashMap<OAuthProvider, Arc<dyn OAuthProviderClient>> = HashMap::new();
    if settings.google_oauth.enabled {
        if let (Some(id), Some(secret), Some(uri)) = (
            settings.google_oauth.client_id.as_ref(),
            settings.google_oauth.client_secret.as_ref(),
            settings.google_oauth.redirect_uri.as_ref(),
        ) {
            oauth_clients.insert(
                OAuthProvider::Google,
                Arc::new(GoogleOAuthClient::new(
                    id.clone(),
                    secret.clone(),
                    uri.clone(),
                )),
            );
        } else {
            tracing::warn!("GOOGLE_OAUTH_ENABLED=true but credentials missing");
        }
    }

    let auth_deps = Arc::new(AuthHandlerDeps {
        register_use_case: register_uc,
        login_use_case: login_uc,
        get_current_user_use_case: get_current_user_uc,
        refresh_token_use_case: refresh_token_uc.clone(),
        revoke_token_use_case: revoke_token_uc,
        send_verification_code_use_case: send_verification_uc.clone(),
        turnstile_service: turnstile_service.clone(),
        mx_validator,
        oauth_account_repo,
        crypto: crypto.clone(),
    });

    let verification_deps = Arc::new(VerificationHandlerDeps {
        verify_email_use_case: verify_email_uc,
        verify_magic_link_use_case: verify_magic_link_uc,
        resend_verification_use_case: resend_verification_uc,
        complete_signup_use_case: complete_signup_uc,
        request_password_reset_use_case: request_password_reset_uc,
        validate_reset_code_use_case: validate_reset_code_uc,
        reset_password_use_case: reset_password_uc,
        refresh_token_use_case: refresh_token_uc.clone(),
        turnstile_service,
        frontend_base_url: settings.server.frontend_base_url.clone(),
        crypto: crypto.clone(),
    });

    let oauth_deps = Arc::new(OAuthHandlerDeps {
        callback_use_case: oauth_callback_uc,
        unlink_use_case: oauth_unlink_uc,
        refresh_token_use_case: refresh_token_uc,
        oauth_clients,
        crypto: crypto.clone(),
    });

    let profile_deps = Arc::new(ProfileHandlerDeps {
        change_password_use_case: change_password_uc,
        set_password_use_case: set_password_uc,
        export_user_data_use_case: export_user_data_uc,
        request_account_deletion_use_case: request_account_deletion_uc,
        cancel_account_deletion_use_case: cancel_account_deletion_uc,
    });

    let admin_deps = Arc::new(AdminHandlerDeps {
        list_users_use_case: Arc::new(ListUsersUseCase::new(user_repo.clone())),
        set_user_role_use_case: Arc::new(SetUserRoleUseCase::new(user_repo.clone())),
    });

    // Singleton background workers run under Postgres advisory-lock leader
    // election so exactly one replica drives them, however many replicas serve
    // HTTP. Anything that must not double-run (pollers with a shared cursor,
    // reconcilers, purges) belongs in this set.
    // Shutdown broadcast: an OS signal flips this watch to `true`, draining both
    // the HTTP server and the background leader task together (see below).
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    let leader_handle = {
        use app::infrastructure::tasks;
        use std::time::Duration;

        // Stable per-deployment key for the background-workers advisory lock.
        // Pick a different value per app if several share one database.
        const BACKGROUND_WORKERS_LOCK: i64 = 0x6170_705f_776f_726b; // "app_work"

        let leader_db = db.clone();
        let purge = purge_accounts_uc;
        let maintenance_pool = db.pool().clone();
        let leader_shutdown = shutdown_rx.clone();

        tokio::spawn(async move {
            tasks::leader::run_as_leader(
                leader_db,
                BACKGROUND_WORKERS_LOCK,
                "background-workers",
                leader_shutdown,
                move || {
                    let purge = purge.clone();
                    let maintenance_pool = maintenance_pool.clone();
                    Box::pin(async move {
                        tokio::join!(
                            tasks::account_purger::run(purge, Duration::from_hours(1)),
                            tasks::database_maintenance::run(
                                maintenance_pool,
                                Duration::from_hours(1),
                            ),
                        );
                    })
                },
            )
            .await;
        })
    };

    let app = create_router(
        RouterDeps {
            auth: auth_deps,
            verification: verification_deps,
            oauth: oauth_deps,
            profile: profile_deps,
            admin: admin_deps,
            db_pool: db.pool().clone(),
            crypto: crypto.clone(),
            users: user_repo.clone(),
        },
        &settings.server.frontend_origin,
    )
    .layer(TraceLayer::new_for_http());

    let addr = settings.server.address();
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("Listening on {addr}");

    // Translate OS signals into the shutdown watch so the HTTP server and the
    // background leader task drain together rather than the workers being
    // abandoned mid-flight when the process exits.
    tokio::spawn(async move {
        shutdown_signal().await;
        // Pre-drain delay: keep accepting connections for a few seconds after the
        // signal so a load balancer (or Kubernetes Service) can stop routing here
        // before we stop serving. Otherwise endpoint removal races SIGTERM and new requests
        // can still be routed to a pod that has already begun draining. The scratch
        // image has no shell for a preStop hook, so this lives here instead. Kept
        // well under the pod's terminationGracePeriodSeconds.
        tracing::info!("Shutdown signal received; waiting 5s for endpoint deregistration");
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let _ = shutdown_tx.send(true);
    });

    let server_shutdown = shutdown_rx.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let mut rx = server_shutdown;
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
            tracing::info!("Shutdown signal received, draining connections");
        })
        .await?;

    // Server drained. Give the leader task a bounded window to cancel its
    // workers and release the advisory lock before the process exits.
    if tokio::time::timeout(std::time::Duration::from_secs(10), leader_handle)
        .await
        .is_ok()
    {
        tracing::info!("Background workers stopped cleanly");
    } else {
        tracing::warn!("Background workers did not stop within 10s; exiting anyway");
    }

    Ok(std::process::ExitCode::SUCCESS)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = sigterm => {},
    }
}
