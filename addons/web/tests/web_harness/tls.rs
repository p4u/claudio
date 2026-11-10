//! `claudio web --remote` in the suites: a server on a loopback port with
//! the fixture password, and a rustls client that trusts exactly the
//! certificate whose SHA-256 the server printed.

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::{fs, thread};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use tokio_rustls::rustls::crypto::{self, CryptoProvider};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use tokio_rustls::rustls::{self, ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use super::{apply_env, free_port, parse_reply, request_text, Reply, Web};
use crate::common::{ManagerHarness, BINARY, DAEMON_WAIT, WAIT};

/// The committed fixture: an argon2id hash of [`TEST_PASSWORD`].
pub const TEST_PASSWORD_HASH: &str = include_str!("../../web/e2e/fixtures/web-password");
pub const TEST_PASSWORD: &str = "claudio-e2e-password";

/// SHA-256 of a DER certificate as the server prints it.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Write the fixture password file (0600) into `h`'s config dir.
pub fn write_test_password(h: &ManagerHarness) {
    let dir = h.config_home.join("claudio");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("web-password");
    fs::write(&path, TEST_PASSWORD_HASH).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// The `claudio_web` value of a `Set-Cookie` header.
pub fn cookie_of(set_cookie: &str) -> String {
    set_cookie
        .strip_prefix("claudio_web=")
        .and_then(|rest| rest.split(';').next())
        .expect("claudio_web=…")
        .to_owned()
}

/// A `--remote` server and the fingerprint it printed.
pub struct RemoteWeb {
    pub web: Web,
    pub fingerprint: String,
}

impl RemoteWeb {
    /// `claudio web --remote 127.0.0.1:<free port> --no-open` plus `args`,
    /// with the fixture password. Asserts the stdout line
    /// (`https://127.0.0.1:<port>/`, no token) and that the printed
    /// fingerprint is `cert.pem`'s.
    pub fn start(h: ManagerHarness, args: &[&str], setup: impl FnOnce(&mut Command)) -> RemoteWeb {
        write_test_password(&h);
        RemoteWeb::start_keeping_password(h, args, setup)
    }

    /// [`RemoteWeb::start`] with the password file `h` already has.
    pub fn start_keeping_password(
        h: ManagerHarness,
        args: &[&str],
        setup: impl FnOnce(&mut Command),
    ) -> RemoteWeb {
        let port = free_port();
        let mut cmd = Command::new(BINARY);
        cmd.arg("web")
            .arg("--remote")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--no-open")
            .args(args);
        apply_env(&h, &mut cmd);
        setup(&mut cmd);
        let log_path = h.root.join("web.log");
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(log);
        let mut child = cmd.spawn().expect("start claudio web --remote");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let line = rx.recv_timeout(DAEMON_WAIT);
        let token = fs::read_to_string(h.config_home.join("claudio/web-token"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        // Dropped (killed) on every path from here.
        let web = Web {
            h: Some(h),
            child,
            port,
            token,
        };
        let line = line.expect("claudio web --remote printed no URL");
        assert_eq!(line, format!("claudio web: https://127.0.0.1:{port}/"));
        // stderr is written before the stdout line.
        let log = fs::read_to_string(&log_path).unwrap();
        let printed = log
            .lines()
            .rev()
            .find_map(|l| l.strip_prefix("claudio web: TLS certificate SHA-256 "))
            .expect("the fingerprint line")
            .to_owned();
        let pem = web.h().config_home.join("claudio/web-tls/cert.pem");
        let der = CertificateDer::from_pem_file(&pem).unwrap();
        assert_eq!(
            printed,
            fingerprint(&der),
            "the printed fingerprint is cert.pem's"
        );
        RemoteWeb {
            web,
            fingerprint: printed,
        }
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.web.port)
    }

    pub fn origin(&self) -> String {
        format!("https://127.0.0.1:{}", self.web.port)
    }

    /// One request over pinned TLS from `src` with `Host: host`.
    pub async fn req_as(
        &self,
        src: &str,
        host: &str,
        method: &str,
        path: &str,
        extra: &[(&str, &str)],
        body: &str,
    ) -> Reply {
        let request = request_text(method, path, host, extra, body);
        https_from(src, self.web.port, &self.fingerprint, &request)
            .await
            .expect("TLS")
    }

    /// One request from 127.0.0.1 with `Host: 127.0.0.1:<port>`.
    pub async fn req(&self, method: &str, path: &str, extra: &[(&str, &str)], body: &str) -> Reply {
        self.req_as("127.0.0.1", &self.host(), method, path, extra, body)
            .await
    }

    /// A login with `body` from the page's origin.
    pub async fn login(&self, body: &serde_json::Value) -> Reply {
        let origin = self.origin();
        self.req(
            "POST",
            "/api/login",
            &[("Origin", &origin)],
            &body.to_string(),
        )
        .await
    }
}

/// Trusts exactly one certificate, by SHA-256; signatures are still checked.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end_entity) == self.fingerprint {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("not the pinned certificate".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls12_signature(
            msg,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls13_signature(
            msg,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A TLS connection from `src` (a loopback address) to 127.0.0.1:`port`
/// that accepts only the certificate `fingerprint`.
pub async fn tls_connect(
    src: &str,
    port: u16,
    fingerprint: &str,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>, String> {
    let provider = Arc::new(crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            fingerprint: fingerprint.to_owned(),
            provider,
        }))
        .with_no_client_auth();
    let socket = tokio::net::TcpSocket::new_v4().map_err(|e| e.to_string())?;
    socket
        .bind(SocketAddr::new(src.parse().map_err(|_| "bad src")?, 0))
        .map_err(|e| e.to_string())?;
    let tcp = socket
        .connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .map_err(|e| e.to_string())?;
    let name = ServerName::try_from("127.0.0.1").map_err(|e| e.to_string())?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .map_err(|e| e.to_string())
}

/// One raw request over a pinned TLS connection from `src`.
pub async fn https_from(
    src: &str,
    port: u16,
    fingerprint: &str,
    request: &str,
) -> Result<Reply, String> {
    let mut tls = tls_connect(src, port, fingerprint).await?;
    tls.write_all(request.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    let _ = tokio::time::timeout(WAIT, tls.read_to_end(&mut raw)).await;
    Ok(parse_reply(&raw))
}

/// A WebSocket over pinned TLS: the first text frame (the hello), or the
/// HTTP status that refused the upgrade.
pub async fn wss_hello(
    port: u16,
    fingerprint: &str,
    cookie: &str,
    origin: &str,
) -> Result<serde_json::Value, u16> {
    use futures::StreamExt;
    let tls = tls_connect("127.0.0.1", port, fingerprint)
        .await
        .expect("TLS");
    let mut req = format!("wss://127.0.0.1:{port}/ws")
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("Origin", origin.parse().unwrap());
    req.headers_mut()
        .insert("Cookie", format!("claudio_web={cookie}").parse().unwrap());
    let (mut ws, _) = match tokio_tungstenite::client_async(req, tls).await {
        Ok(pair) => pair,
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            return Err(resp.status().as_u16())
        }
        Err(e) => panic!("wss: {e}"),
    };
    let msg = tokio::time::timeout(WAIT, ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    Ok(serde_json::from_str(msg.to_text().unwrap()).unwrap())
}
