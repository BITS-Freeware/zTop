// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one accept loop the agent and the hub serve through, over plain
//! TCP or TLS (decision 0064). A server that answers a network has to
//! survive the network: a failed accept is logged and retried, never
//! fatal; a peer that opens a connection and then goes quiet is
//! dropped at a deadline, whether it stalls in the TLS handshake, in
//! the request head, or in the request body; and the number of
//! connections served at once is capped, so a flood waits in the
//! kernel's backlog instead of exhausting file descriptors.
//!
//! Only HTTP/1.1 is spoken. Every zTop client sends HTTP/1.1, TLS
//! offers no ALPN, and leaving out HTTP/2 also leaves out the
//! protocol sniff that read from a connection with no deadline.

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Semaphore;

/// The deadlines and the connection cap one server runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// A TLS handshake must finish within this.
    pub handshake: Duration,
    /// A request head must arrive in full within this, counted from
    /// when the server starts waiting for it, so an idle keep-alive
    /// connection closes at the same deadline.
    pub headers: Duration,
    /// A request, body included, must be answered within this. A
    /// WebSocket answers at once (the upgrade), so a stream is not
    /// cut by it.
    pub request: Duration,
    /// How many connections are served at once.
    pub connections: usize,
}

impl Limits {
    /// What `serve` uses. zTop clients send one short request per
    /// connection, so ten seconds is generous for any honest peer.
    pub const DEFAULT: Limits = Limits {
        handshake: Duration::from_secs(10),
        headers: Duration::from_secs(10),
        request: Duration::from_secs(30),
        connections: 256,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Limits::DEFAULT
    }
}

/// The first wait after a failed accept, doubled on every failure in
/// a row up to `MAX_ACCEPT_BACKOFF`. Running out of file descriptors
/// is the usual cause, and it clears as connections close.
const FIRST_ACCEPT_BACKOFF: Duration = Duration::from_millis(50);
const MAX_ACCEPT_BACKOFF: Duration = Duration::from_secs(1);

/// Serve `router` on `listener` under the default limits, over TLS
/// when `tls` is given. Returns only if the listener can no longer be
/// read at all, which a `TcpListener` never reports.
pub async fn serve(
    listener: tokio::net::TcpListener,
    tls: Option<rustls::ServerConfig>,
    router: Router,
) -> io::Result<()> {
    serve_with(listener, tls, router, Limits::DEFAULT).await
}

/// `serve` under explicit limits.
pub async fn serve_with(
    listener: tokio::net::TcpListener,
    tls: Option<rustls::ServerConfig>,
    router: Router,
    limits: Limits,
) -> io::Result<()> {
    serve_accepted(listener, tls, router, limits).await
}

/// Where connections come from: a listener in service, a channel of
/// in-memory streams in the tests. `None` means no more will come.
pub(crate) trait Incoming {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn next(&mut self) -> impl Future<Output = Option<io::Result<Self::Stream>>> + Send;
}

impl Incoming for tokio::net::TcpListener {
    type Stream = tokio::net::TcpStream;

    async fn next(&mut self) -> Option<io::Result<Self::Stream>> {
        Some(self.accept().await.map(|(stream, _)| stream))
    }
}

/// The accept loop itself. A slot is taken before each accept, so at
/// the cap the loop stops accepting until a connection ends.
pub(crate) async fn serve_accepted<I: Incoming + Send>(
    mut incoming: I,
    tls: Option<rustls::ServerConfig>,
    router: Router,
    limits: Limits,
) -> io::Result<()> {
    let acceptor = tls.map(|config| tokio_rustls::TlsAcceptor::from(Arc::new(config)));
    let router = router.layer(middleware::from_fn_with_state(
        limits.request,
        answer_within,
    ));
    let slots = Arc::new(Semaphore::new(limits.connections.max(1)));
    let mut backoff: Option<Duration> = None;
    loop {
        let Ok(slot) = slots.clone().acquire_owned().await else {
            return Ok(());
        };
        let stream = match incoming.next().await {
            None => return Ok(()),
            Some(Ok(stream)) => {
                if backoff.take().is_some() {
                    eprintln!("ztop: accepting connections again");
                }
                stream
            }
            Some(Err(error)) => {
                let wait = match backoff {
                    None => {
                        eprintln!("ztop: accepting a connection failed: {error}; retrying");
                        FIRST_ACCEPT_BACKOFF
                    }
                    Some(previous) => (previous * 2).min(MAX_ACCEPT_BACKOFF),
                };
                backoff = Some(wait);
                drop(slot);
                tokio::time::sleep(wait).await;
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let router = router.clone();
        tokio::spawn(async move {
            // Held for the life of the connection: its drop frees the slot.
            let _slot = slot;
            match acceptor {
                None => serve_connection(stream, router, limits).await,
                Some(acceptor) => {
                    let handshake =
                        tokio::time::timeout(limits.handshake, acceptor.accept(stream)).await;
                    if let Ok(Ok(stream)) = handshake {
                        serve_connection(stream, router, limits).await;
                    }
                }
            }
        });
    }
}

/// HTTP/1.1 on one connection, with the header deadline armed. Ends
/// when the peer closes, a deadline passes, or a WebSocket upgrade
/// hands the connection to its handler.
async fn serve_connection<S>(stream: S, router: Router, limits: Limits)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = hyper_util::service::TowerToHyperService::new(router);
    let _ = hyper::server::conn::http1::Builder::new()
        .timer(hyper_util::rt::TokioTimer::new())
        .header_read_timeout(limits.headers)
        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
        .with_upgrades()
        .await;
}

/// Answer a request within the limit, or with 408: the body is read
/// inside the handler, so this is what stops a peer that drips one.
async fn answer_within(State(limit): State<Duration>, request: Request, next: Next) -> Response {
    match tokio::time::timeout(limit, next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::mpsc;

    impl Incoming for mpsc::UnboundedReceiver<io::Result<DuplexStream>> {
        type Stream = DuplexStream;

        async fn next(&mut self) -> Option<io::Result<DuplexStream>> {
            self.recv().await
        }
    }

    fn a_router() -> Router {
        Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route("/echo", post(|body: String| async move { body }))
    }

    /// Short deadlines, so a test that waits for one waits briefly.
    fn tight() -> Limits {
        Limits {
            handshake: Duration::from_millis(200),
            headers: Duration::from_millis(200),
            request: Duration::from_millis(300),
            connections: 8,
        }
    }

    type Sender = mpsc::UnboundedSender<io::Result<DuplexStream>>;

    fn a_server(tls: Option<rustls::ServerConfig>, limits: Limits) -> Sender {
        let (sender, receiver) = mpsc::unbounded_channel();
        tokio::spawn(serve_accepted(receiver, tls, a_router(), limits));
        sender
    }

    /// Hand the server a fresh connection and return the client's end.
    fn connect(sender: &Sender) -> DuplexStream {
        let (client, server) = tokio::io::duplex(64 * 1024);
        sender.send(Ok(server)).expect("server running");
        client
    }

    const GET_HEALTHZ: &[u8] = b"GET /healthz HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n";

    /// Everything the server sends until it closes, or `None` when it
    /// is still holding the connection open after `within`.
    async fn read_until_closed(client: &mut DuplexStream, within: Duration) -> Option<String> {
        let mut raw = Vec::new();
        match tokio::time::timeout(within, client.read_to_end(&mut raw)).await {
            Ok(_) => Some(String::from_utf8_lossy(&raw).into_owned()),
            Err(_) => None,
        }
    }

    /// A failed accept (out of file descriptors, a connection reset
    /// before it was taken) used to end the whole server. Now it is
    /// retried, and the next connection is served.
    #[tokio::test]
    async fn the_server_keeps_accepting_after_an_accept_error() {
        let sender = a_server(None, tight());
        sender
            .send(Err(io::Error::other("too many open files")))
            .expect("server running");
        let mut client = connect(&sender);
        client.write_all(GET_HEALTHZ).await.expect("write");
        let answer = read_until_closed(&mut client, Duration::from_secs(5))
            .await
            .expect("answered after the failed accept");
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    }

    /// A peer that connects and sends nothing is dropped at the
    /// header deadline instead of holding the connection forever.
    #[tokio::test]
    async fn a_silent_client_is_dropped() {
        let sender = a_server(None, tight());
        let mut client = connect(&sender);
        let answer = read_until_closed(&mut client, Duration::from_secs(3)).await;
        assert!(answer.is_some(), "a silent connection must be closed");
    }

    /// Half a request head, then nothing: the slow-headers attack.
    #[tokio::test]
    async fn a_half_sent_request_head_is_dropped() {
        let sender = a_server(None, tight());
        let mut client = connect(&sender);
        client
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: t\r\n")
            .await
            .expect("write");
        let answer = read_until_closed(&mut client, Duration::from_secs(3))
            .await
            .expect("a stalled head must be closed");
        assert!(!answer.contains("200"), "{answer}");
    }

    /// A body promised and then dripped (or never sent) is answered
    /// 408 at the request deadline.
    #[tokio::test]
    async fn a_stalled_request_body_is_answered_408() {
        let sender = a_server(None, tight());
        let mut client = connect(&sender);
        client
            .write_all(b"POST /echo HTTP/1.1\r\nHost: t\r\nContent-Length: 10\r\n\r\nx")
            .await
            .expect("write");
        let mut raw = vec![0u8; 1024];
        let read = tokio::time::timeout(Duration::from_secs(3), client.read(&mut raw))
            .await
            .expect("answered before the test gives up")
            .expect("read");
        let answer = String::from_utf8_lossy(&raw[..read]);
        assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
    }

    /// A peer that opens a TLS connection and never says hello is
    /// dropped at the handshake deadline.
    #[tokio::test]
    async fn a_stalled_tls_handshake_is_dropped() {
        let dir = std::env::temp_dir().join(format!(
            "ztop-serving-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let material = crate::tls::load_or_create(&dir).expect("material");
        let sender = a_server(Some(material.config), tight());
        let mut client = connect(&sender);
        let answer = read_until_closed(&mut client, Duration::from_secs(3)).await;
        let _ = std::fs::remove_dir_all(&dir);
        assert!(answer.is_some(), "a silent TLS peer must be closed");
    }

    /// At the cap, a new connection waits for a slot rather than being
    /// served alongside, and is served as soon as one frees.
    #[tokio::test]
    async fn connections_past_the_cap_wait_for_a_free_slot() {
        let limits = Limits {
            headers: Duration::from_secs(30),
            connections: 1,
            ..tight()
        };
        let sender = a_server(None, limits);
        let holder = connect(&sender);
        // Give the server time to take the first connection.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut waiting = connect(&sender);
        waiting.write_all(GET_HEALTHZ).await.expect("write");
        assert_eq!(
            read_until_closed(&mut waiting, Duration::from_millis(300)).await,
            None,
            "the second connection must wait while the first holds the only slot"
        );
        drop(holder);
        let answer = read_until_closed(&mut waiting, Duration::from_secs(5))
            .await
            .expect("served once the slot frees");
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    }

    #[test]
    fn the_default_limits_are_the_documented_ones() {
        assert_eq!(Limits::default(), Limits::DEFAULT);
        assert_eq!(Limits::DEFAULT.handshake, Duration::from_secs(10));
        assert_eq!(Limits::DEFAULT.headers, Duration::from_secs(10));
        assert_eq!(Limits::DEFAULT.connections, 256);
    }
}
