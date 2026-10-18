//! The server over real TCP, from start to graceful shutdown.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use trackdo_server::auth::TokenDigest;
use trackdo_server::config::Config;
use trackdo_server::server::{ServeError, serve};

/// Fails a test that would otherwise hang.
const DEADLINE: Duration = Duration::from_secs(5);

/// A database directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("trackdo-serve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Running {
    addr: SocketAddr,
    stop: oneshot::Sender<()>,
    server: JoinHandle<Result<(), ServeError>>,
}

async fn start(database: PathBuf) -> Running {
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        database,
        tokens: vec![TokenDigest::of("secret")],
        cors_origins: Vec::new(),
    };
    let listener = TcpListener::bind(config.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let server = tokio::spawn(async move {
        serve(&config, listener, async {
            let _ = stopped.await;
        })
        .await
    });
    Running { addr, stop, server }
}

async fn request(addr: SocketAddr, lines: &[&str]) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!("{}\r\n\r\n", lines.join("\r\n"));
    stream.write_all(request.as_bytes()).await.unwrap();
    stream
}

async fn read_until(stream: &mut TcpStream, marker: &str) -> String {
    let mut received = Vec::new();
    let mut buffer = [0; 1024];
    while !String::from_utf8_lossy(&received).contains(marker) {
        let read = timeout(DEADLINE, stream.read(&mut buffer))
            .await
            .expect("no data in time")
            .unwrap();
        assert_ne!(read, 0, "closed before {marker:?}");
        received.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(received).unwrap()
}

#[tokio::test]
async fn answers_over_tcp() {
    let dir = TempDir::new("answers");
    let running = start(dir.0.join("trackdo.db")).await;

    let mut stream = request(
        running.addr,
        &["GET /healthz HTTP/1.1", "Host: test", "Connection: close"],
    )
    .await;

    let response = read_until(&mut stream, "protocol_version").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    running.stop.send(()).unwrap();
    running.server.await.unwrap().unwrap();
}

#[tokio::test]
async fn shuts_down_while_event_streams_are_open() {
    let dir = TempDir::new("shutdown");
    let running = start(dir.0.join("trackdo.db")).await;
    let mut events = request(
        running.addr,
        &[
            "GET /events HTTP/1.1",
            "Host: test",
            "Authorization: Bearer secret",
            "Trackdo-Protocol: 2",
        ],
    )
    .await;
    read_until(&mut events, "text/event-stream").await;

    running.stop.send(()).unwrap();

    timeout(DEADLINE, running.server)
        .await
        .expect("shutdown waited for the stream")
        .unwrap()
        .unwrap();
    let mut rest = Vec::new();
    timeout(DEADLINE, events.read_to_end(&mut rest))
        .await
        .expect("stream left open")
        .unwrap();
}

#[tokio::test]
async fn fails_when_the_database_cannot_be_opened() {
    let dir = TempDir::new("nodb");
    let running = start(dir.0.join("missing").join("trackdo.db")).await;

    let result = timeout(DEADLINE, running.server).await.unwrap().unwrap();

    assert!(matches!(result, Err(ServeError::Store(_))));
}
