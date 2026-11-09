//! The `--remote` listener: an `axum::serve::Listener` that accepts TCP,
//! runs each TLS handshake in its own task (10 s deadline, at most 64 at
//! once) and hands finished streams to `accept` through a bounded channel,
//! so one slow client never stalls the others. Open connections are capped
//! (256 overall, 32 per peer but loopback); a connection that has not sent
//! the end of its first request's headers within 30 s is closed (axum's
//! hyper builder has no timer).

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tokio::time::Sleep;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

use crate::peer::{Peer, PeerKey};

/// The listener's bounds; [`Limits::DEFAULT`] in a server, shorter in tests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub handshake: Duration,
    pub handshakes: usize,
    pub connections: usize,
    pub per_peer: usize,
    pub headers: Duration,
}

impl Limits {
    pub(crate) const DEFAULT: Limits = Limits {
        handshake: Duration::from_secs(10),
        handshakes: 64,
        connections: 256,
        per_peer: 32,
        headers: Duration::from_secs(30),
    };
}

/// Open connections per peer key (loopback is not counted).
type PeerCounts = Arc<Mutex<HashMap<PeerKey, usize>>>;

pub(crate) struct TlsListener {
    rx: mpsc::Receiver<(TlsIo, Peer)>,
    local: SocketAddr,
    accept_loop: tokio::task::JoinHandle<()>,
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

impl TlsListener {
    pub(crate) fn new(
        tcp: TcpListener,
        acceptor: TlsAcceptor,
        limits: Limits,
    ) -> io::Result<TlsListener> {
        let local = tcp.local_addr()?;
        let (tx, rx) = mpsc::channel(limits.handshakes);
        let accept_loop = tokio::spawn(accept_loop(tcp, acceptor, tx, limits));
        Ok(TlsListener {
            rx,
            local,
            accept_loop,
        })
    }
}

async fn accept_loop(
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    tx: mpsc::Sender<(TlsIo, Peer)>,
    limits: Limits,
) {
    let handshakes = Arc::new(Semaphore::new(limits.handshakes));
    let connections = Arc::new(Semaphore::new(limits.connections));
    let per_peer: PeerCounts = Arc::default();
    loop {
        let (stream, addr) = match tcp.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                // EMFILE and friends: back off as axum's own listener does.
                tracing::debug!(error = %e, "web: tcp accept failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let peer = Peer(addr);
        // Every refusal drops the stream: the client sees a reset.
        let Ok(handshake) = Arc::clone(&handshakes).try_acquire_owned() else {
            continue;
        };
        let Ok(open) = Arc::clone(&connections).try_acquire_owned() else {
            continue;
        };
        let Some(slot) = PeerSlot::take(&per_peer, peer, limits.per_peer) else {
            continue;
        };
        let (acceptor, tx) = (acceptor.clone(), tx.clone());
        tokio::spawn(async move {
            let tls = match tokio::time::timeout(limits.handshake, acceptor.accept(stream)).await {
                Ok(Ok(tls)) => tls,
                Ok(Err(e)) => {
                    tracing::debug!(peer = %peer.0, error = %e, "web: TLS handshake failed");
                    return;
                }
                Err(_) => {
                    tracing::debug!(peer = %peer.0, "web: TLS handshake timed out");
                    return;
                }
            };
            drop(handshake);
            let io = TlsIo {
                inner: tls,
                header_deadline: Some(Box::pin(tokio::time::sleep(limits.headers))),
                tail: [0; 3],
                _open: open,
                _slot: slot,
            };
            let _ = tx.send((io, peer)).await;
        });
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsIo;
    type Addr = Peer;

    async fn accept(&mut self) -> (TlsIo, Peer) {
        match self.rx.recv().await {
            Some(pair) => pair,
            // The accept loop ended (only at drop): wait; the graceful
            // shutdown stops the server.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Peer> {
        Ok(Peer(self.local))
    }
}

impl Connected<IncomingStream<'_, TlsListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Peer {
        *stream.remote_addr()
    }
}

/// A peer's share of `Limits::per_peer`, given back on drop.
struct PeerSlot {
    counts: PeerCounts,
    key: Option<PeerKey>,
}

impl PeerSlot {
    fn take(counts: &PeerCounts, peer: Peer, max: usize) -> Option<PeerSlot> {
        let key = peer.key();
        if key == PeerKey::Loopback {
            return Some(PeerSlot {
                counts: Arc::clone(counts),
                key: None,
            });
        }
        let mut map = counts.lock().unwrap_or_else(|e| e.into_inner());
        let n = map.entry(key).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(PeerSlot {
            counts: Arc::clone(counts),
            key: Some(key),
        })
    }
}

impl Drop for PeerSlot {
    fn drop(&mut self) {
        let Some(key) = self.key else { return };
        let mut map = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = map.get_mut(&key) {
            *n -= 1;
            if *n == 0 {
                map.remove(&key);
            }
        }
    }
}

/// A served TLS connection: its permits travel with it, and its first
/// request's headers have `Limits::headers` to arrive.
pub(crate) struct TlsIo {
    inner: TlsStream<TcpStream>,
    /// Until the first `\r\n\r\n` was read.
    header_deadline: Option<Pin<Box<Sleep>>>,
    /// The last three bytes read, to find `\r\n\r\n` across reads.
    tail: [u8; 3],
    _open: OwnedSemaphorePermit,
    _slot: PeerSlot,
}

impl TlsIo {
    /// Whether `\r\n\r\n` ends within `tail` + `new`.
    fn saw_headers_end(&mut self, new: &[u8]) -> bool {
        let mut window = Vec::with_capacity(self.tail.len() + new.len());
        window.extend_from_slice(&self.tail);
        window.extend_from_slice(new);
        let found = window.windows(4).any(|w| w == b"\r\n\r\n");
        // `window` holds at least the three tail bytes.
        self.tail.copy_from_slice(&window[window.len() - 3..]);
        found
    }
}

impl AsyncRead for TlsIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if let Some(deadline) = this.header_deadline.as_mut() {
            if deadline.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no request headers in time",
                )));
            }
        }
        let before = buf.filled().len();
        let res = Pin::new(&mut this.inner).poll_read(cx, buf);
        if this.header_deadline.is_some()
            && matches!(res, Poll::Ready(Ok(())))
            && this.saw_headers_end(&buf.filled()[before..])
        {
            this.header_deadline = None;
        }
        res
    }
}

impl AsyncWrite for TlsIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    use axum::extract::ConnectInfo;
    use axum::routing::get;
    use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
    use rustls::crypto::{self, CryptoProvider};
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::tls::cert;

    /// Trusts exactly one certificate, by SHA-256; signatures are still
    /// checked (the suites' harness has the same verifier).
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
            if cert::fingerprint(end_entity) == self.fingerprint {
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

    struct Server {
        addr: SocketAddr,
        fingerprint: String,
        dir: std::path::PathBuf,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn server(limits: Limits) -> Server {
        let dir = claudio_core::paths::socket_test_dir("web-listener");
        let names = vec!["127.0.0.1".to_owned()];
        let c = cert::load_or_create(&dir, &names, false).unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let listener =
            TlsListener::new(tcp, TlsAcceptor::from(Arc::clone(&c.config)), limits).unwrap();
        let app = axum::Router::new().route(
            "/",
            get(|ConnectInfo(p): ConnectInfo<Peer>| async move { p.0.to_string() }),
        );
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service_with_connect_info::<Peer>()).await;
        });
        Server {
            addr,
            fingerprint: c.fingerprint(),
            dir,
            task,
        }
    }

    async fn connect(
        addr: SocketAddr,
        fingerprint: &str,
    ) -> io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let provider = Arc::new(crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned {
                fingerprint: fingerprint.to_owned(),
                provider,
            }))
            .with_no_client_auth();
        let tcp = TcpStream::connect(addr).await?;
        let name = ServerName::try_from("127.0.0.1").unwrap();
        tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(name, tcp)
            .await
    }

    /// Everything `s` sends until EOF (or an error), within `within`.
    async fn read_all(s: &mut (impl AsyncRead + Unpin), within: Duration) -> Vec<u8> {
        let mut raw = Vec::new();
        let _ = tokio::time::timeout(within, s.read_to_end(&mut raw))
            .await
            .expect("EOF in time");
        raw
    }

    async fn get_root(addr: SocketAddr, fingerprint: &str) -> (String, SocketAddr) {
        let mut tls = connect(addr, fingerprint).await.unwrap();
        let me = tls.get_ref().0.local_addr().unwrap();
        tls.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let raw = read_all(&mut tls, Duration::from_secs(5)).await;
        (String::from_utf8_lossy(&raw).into_owned(), me)
    }

    #[tokio::test]
    async fn the_listener_serves_tls_and_passes_the_peer() {
        let s = server(Limits::DEFAULT).await;
        let (reply, me) = get_root(s.addr, &s.fingerprint).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(reply.ends_with(&me.to_string()), "{reply} / {me}");
        assert!(connect(s.addr, "00:11").await.is_err(), "another pin fails");
        let mut plain = TcpStream::connect(s.addr).await.unwrap();
        plain
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let raw = read_all(&mut plain, Duration::from_secs(5)).await;
        assert!(!raw.starts_with(b"HTTP/"), "plain HTTP is not served");
    }

    #[tokio::test]
    async fn a_connection_without_headers_is_closed() {
        let s = server(Limits {
            headers: Duration::from_millis(300),
            ..Limits::DEFAULT
        })
        .await;
        let mut tls = connect(s.addr, &s.fingerprint).await.unwrap();
        tls.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n")
            .await
            .unwrap();
        let t0 = Instant::now();
        let raw = read_all(&mut tls, Duration::from_secs(2)).await;
        assert!(
            !raw.starts_with(b"HTTP/1.1 200"),
            "no answer without headers"
        );
        assert!(t0.elapsed() < Duration::from_secs(2));
        let (reply, _) = get_root(s.addr, &s.fingerprint).await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    }

    #[tokio::test]
    async fn a_stalled_handshake_does_not_block_others() {
        let s = server(Limits {
            handshake: Duration::from_millis(500),
            ..Limits::DEFAULT
        })
        .await;
        let mut silent = TcpStream::connect(s.addr).await.unwrap();
        let t0 = Instant::now();
        let (reply, _) =
            tokio::time::timeout(Duration::from_secs(1), get_root(s.addr, &s.fingerprint))
                .await
                .expect("served beside a stalled handshake");
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
        let raw = read_all(&mut silent, Duration::from_secs(2)).await;
        assert!(raw.is_empty(), "closed without a byte");
        assert!(t0.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn per_peer_slots_are_counted_and_given_back() {
        let counts: PeerCounts = Arc::default();
        let remote = Peer("10.0.0.1:1".parse().unwrap());
        let a = PeerSlot::take(&counts, remote, 2).expect("first");
        let b = PeerSlot::take(&counts, remote, 2).expect("second");
        assert!(PeerSlot::take(&counts, remote, 2).is_none(), "third");
        let other = PeerSlot::take(&counts, Peer("10.0.0.2:1".parse().unwrap()), 2);
        assert!(other.is_some(), "another address has its own share");
        drop(a);
        let c = PeerSlot::take(&counts, remote, 2).expect("given back");
        let lo = PeerSlot::take(&counts, Peer("127.0.0.1:1".parse().unwrap()), 0);
        assert!(lo.is_some(), "loopback is never refused");
        drop((b, c, other, lo));
        assert!(counts.lock().unwrap().is_empty());
    }
}
