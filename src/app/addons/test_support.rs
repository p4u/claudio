//! Fixtures for tests of the addon runtime and the launch composer: a real
//! `Client` whose daemon is a fake over `tokio::io::duplex`.

use tokio::io::DuplexStream;

use crate::client::Client;
use crate::proto::{read_frame, write_frame, Envelope, Frame, HostInfo, Msg, Welcome, PROTO};

/// A `Client` whose daemon is `daemon`, a task answering after the
/// handshake with `features`.
pub(crate) async fn fake_host<F, Fut>(features: &[&str], daemon: F) -> Client
where
    F: FnOnce(DuplexStream) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (ours, mut theirs) = tokio::io::duplex(2 << 20);
    let welcome = Msg::Welcome(Welcome {
        claudio_version: "test".into(),
        proto: PROTO,
        host: HostInfo {
            hostname: "h".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            home: "/home/u".into(),
            claude: None,
        },
        build: None,
        build_time: None,
        features: features.iter().map(|f| f.to_string()).collect(),
    });
    tokio::spawn(async move {
        let _hello = read_frame(&mut theirs).await;
        write_frame(&mut theirs, &Frame::Control(Envelope::event(welcome)))
            .await
            .unwrap();
        daemon(theirs).await;
    });
    Client::handshake(ours).await.unwrap()
}

/// The next request the fake daemon reads.
pub(crate) async fn next_request(stream: &mut DuplexStream) -> Option<(u64, Msg)> {
    match read_frame(stream).await {
        Ok(Some(Frame::Control(Envelope {
            req: Some(req),
            msg,
        }))) => Some((req, msg)),
        _ => None,
    }
}

/// Answer request `req` with `msg`.
pub(crate) async fn reply(stream: &mut DuplexStream, req: u64, msg: Msg) {
    write_frame(stream, &Frame::Control(Envelope::request(req, msg)))
        .await
        .unwrap();
}
