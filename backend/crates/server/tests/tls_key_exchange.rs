//! Outgoing TLS offers post-quantum key exchange first (OT-009, spec
//! requirement "Connections that cross a network use post-quantum key
//! exchange").
//!
//! A local rustls server accepts only the groups each test names and reports
//! which one was negotiated. The client is a reqwest client built the way the
//! Google and Turnstile adapters build theirs, after the process-wide provider
//! from `infrastructure::tls` is installed as `main` does. lettre (SMTP) takes
//! the same process default.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rustls::crypto::aws_lc_rs::{self, kx_group};
use rustls::crypto::{CryptoProvider, SupportedKxGroup};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{NamedGroup, ServerConfig, ServerConnection, StreamOwned};

use overkill_todo::infrastructure::tls;

/// Self-signed P-256 certificate for `localhost`. Test use only.
const CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIBdDCCARqgAwIBAgIUfYkdELFT85fNc65dlylposxuI8gwCgYIKoZIzj0EAwIw\nFDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MTExMzA3MjIxNVoYDzIxMjYxMDIw\nMDcyMjE1WjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO\nPQMBBwNCAASTFc/2lsiVsxdIeFnm/DgwGGMySAXBOu8os2ToQ+ExucJ59E72Ktp6\nOn4xB6M6AAuTYcVKsfNqmslsKvao5adpo0gwRjAdBgNVHQ4EFgQUQeyRCxERr3zv\nOwAn4m3/hok/GwowDwYDVR0TAQH/BAUwAwEB/zAUBgNVHREEDTALgglsb2NhbGhv\nc3QwCgYIKoZIzj0EAwIDSAAwRQIhAJBuVgxaqOpd3dkylKcbGB9eH5ALFulv/zCb\nS7USKrWEAiB6Kl0fpWRHpLWTWyHHydAT1xnMh4RSwSrpVGQTD55vIQ==\n-----END CERTIFICATE-----";
/// Its private key (PKCS#8). Test use only.
const KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg9O7hBqfkRqi8godR\nm90mTM1+3Q5sw7vMFPF7hfDdWCahRANCAASTFc/2lsiVsxdIeFnm/DgwGGMySAXB\nOu8os2ToQ+ExucJ59E72Ktp6On4xB6M6AAuTYcVKsfNqmslsKvao5adp\n-----END PRIVATE KEY-----";

/// What the server saw: the negotiated group, or why the handshake failed.
type Outcome = Result<NamedGroup, String>;

/// Serve one HTTPS request on a local port, accepting only `groups`. Returns
/// the URL and a receiver for the outcome.
fn serve_once(groups: Vec<&'static dyn SupportedKxGroup>) -> (String, mpsc::Receiver<Outcome>) {
    let provider = CryptoProvider {
        kx_groups: groups,
        ..aws_lc_rs::default_provider()
    };
    let config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(CERT_PEM.as_bytes()).unwrap()],
            PrivateKeyDer::from_pem_slice(KEY_PEM.as_bytes()).unwrap(),
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "https://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let conn = ServerConnection::new(Arc::new(config)).unwrap();
        let mut tls = StreamOwned::new(conn, tcp);
        let outcome = (|| {
            while tls.conn.is_handshaking() {
                tls.conn
                    .complete_io(&mut tls.sock)
                    .map_err(|e| e.to_string())?;
            }
            let group = tls
                .conn
                .negotiated_key_exchange_group()
                .ok_or("no group negotiated")?
                .name();
            let mut buf = [0u8; 1024];
            let _ = tls.read(&mut buf);
            let _ = tls
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok");
            let _ = tls.flush();
            Ok(group)
        })();
        let _ = tx.send(outcome);
    });
    (url, rx)
}

/// A client built like the adapters' (same builder calls), but accepting the
/// test certificate.
fn adapter_like_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(5))
        .tls_danger_accept_invalid_certs(true)
        .build()
        .unwrap()
}

fn outcome(rx: &mpsc::Receiver<Outcome>) -> Outcome {
    rx.recv_timeout(Duration::from_secs(10))
        .expect("server reported an outcome")
}

#[tokio::test]
async fn backend_clients_negotiate_the_hybrid_with_a_hybrid_only_server() {
    tls::install().unwrap();
    let (url, rx) = serve_once(vec![kx_group::X25519MLKEM768]);

    let resp = adapter_like_client().get(&url).send().await;

    assert!(
        resp.is_ok(),
        "hybrid-only server must be reachable: {resp:?}"
    );
    assert_eq!(outcome(&rx), Ok(NamedGroup::X25519MLKEM768));
}

#[tokio::test]
async fn backend_clients_prefer_the_hybrid_when_the_server_offers_both() {
    tls::install().unwrap();
    let (url, rx) = serve_once(vec![kx_group::X25519, kx_group::X25519MLKEM768]);

    adapter_like_client().get(&url).send().await.unwrap();

    assert_eq!(outcome(&rx), Ok(NamedGroup::X25519MLKEM768));
}

#[tokio::test]
async fn backend_clients_fall_back_to_classical_for_a_classical_only_server() {
    tls::install().unwrap();
    let (url, rx) = serve_once(vec![kx_group::X25519]);

    let resp = adapter_like_client().get(&url).send().await;

    assert!(
        resp.is_ok(),
        "classical-only server must still be reachable: {resp:?}"
    );
    assert_eq!(outcome(&rx), Ok(NamedGroup::X25519));
}

/// The gate means something: a client that doesn't offer the hybrid can't
/// reach the hybrid-only server, so the first test would fail if the
/// installed provider ever lost it.
#[tokio::test]
async fn a_classical_only_client_cannot_reach_the_hybrid_only_server() {
    let classical = CryptoProvider {
        kx_groups: vec![kx_group::X25519],
        ..aws_lc_rs::default_provider()
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(classical))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(config)
        .build()
        .unwrap();
    let (url, rx) = serve_once(vec![kx_group::X25519MLKEM768]);

    assert!(client.get(&url).send().await.is_err());
    let err = outcome(&rx).unwrap_err();
    assert!(
        err.contains("NoKxGroupsInCommon"),
        "server-side failure: {err}"
    );
}
