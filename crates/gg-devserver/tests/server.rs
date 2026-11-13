//! Runs the dev server in-process on an ephemeral port and talks raw HTTP/1.1 to it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use gg_core::Core;
use gg_core::testutil::TestRepo;
use gg_devserver::{Events, serve};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TIMEOUT: Duration = Duration::from_secs(20);

struct Server {
    addr: SocketAddr,
    _config: TempDir,
}

async fn start() -> Server {
    let config = tempfile::tempdir().unwrap();
    let events = Events::new();
    let core: Arc<Core> = Core::new(config.path().to_path_buf(), None, events.sink())
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, core, events));
    Server {
        addr,
        _config: config,
    }
}

impl Server {
    /// Sends one request and returns the status line, the headers and the body.
    async fn request(&self, head: &str, body: &str) -> (String, String, String) {
        self.request_with(head, "", body).await
    }

    /// Like [`Server::request`], with `headers` (each ending in CRLF) added.
    async fn request_with(
        &self,
        head: &str,
        headers: &str,
        body: &str,
    ) -> (String, String, String) {
        let mut stream = TcpStream::connect(self.addr).await.unwrap();
        let request = format!(
            "{head} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{headers}Content-Length: {}\r\n\r\n{body}",
            self.addr,
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::time::timeout(TIMEOUT, stream.read_to_string(&mut response))
            .await
            .expect("response arrives")
            .unwrap();
        let (head, body) = response.split_once("\r\n\r\n").expect("headers end");
        let (status, headers) = head.split_once("\r\n").unwrap_or((head, ""));
        (status.to_owned(), headers.to_lowercase(), body.to_owned())
    }

    async fn invoke(&self, body: Value) -> Value {
        let (status, _, body) = self.request("POST /invoke", &body.to_string()).await;
        assert_eq!(status, "HTTP/1.1 200 OK");
        serde_json::from_str(&body).expect("JSON body")
    }
}

#[tokio::test]
async fn health_is_ok() {
    let server = start().await;
    let (status, _, body) = server.request("GET /health", "").await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn invoke_returns_ok() {
    let server = start().await;
    let reply = server
        .invoke(json!({"cmd": "get_app_state", "args": {}}))
        .await;
    assert_eq!(reply["ok"]["settings"]["fetchIntervalSecs"], 300);
    assert_eq!(reply["ok"]["launchPath"], Value::Null);
    assert!(reply.get("err").is_none());
    // `args` may be left out for commands that take none.
    let reply = server.invoke(json!({"cmd": "get_app_state"})).await;
    assert!(reply["ok"].is_object());
}

#[tokio::test]
async fn invoke_error_shape() {
    let server = start().await;
    let reply = server.invoke(json!({"cmd": "nope", "args": {}})).await;
    assert_eq!(reply["err"]["code"], "bad_request");
    assert!(reply["err"]["message"].is_string());
    assert!(
        reply["err"].get("detail").is_some(),
        "detail is present (null)"
    );
    assert!(reply.get("ok").is_none());

    let not_repo = tempfile::tempdir().unwrap();
    let reply = server
        .invoke(json!({"cmd": "open_repo", "args": {"path": not_repo.path()}}))
        .await;
    assert_eq!(reply["err"]["code"], "not_a_repo");

    // A body that is not a request is a bad request too, in the same shape.
    let (status, _, body) = server.request("POST /invoke", "{oops").await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    let reply: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(reply["err"]["code"], "bad_request");
}

#[tokio::test]
async fn cors_is_permissive() {
    let server = start().await;
    let preflight = "Origin: http://127.0.0.1:1420\r\n\
                     Access-Control-Request-Method: POST\r\n\
                     Access-Control-Request-Headers: content-type\r\n";
    let (status, headers, _) = server.request_with("OPTIONS /invoke", preflight, "").await;
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert!(
        headers.contains("access-control-allow-origin: *"),
        "{headers}"
    );
}

#[tokio::test]
async fn events_stream_repo_changed_after_open() {
    let repo = TestRepo::new();
    repo.commit("one");
    let server = start().await;

    let mut events = TcpStream::connect(server.addr).await.unwrap();
    let get = format!("GET /events HTTP/1.1\r\nHost: {}\r\n\r\n", server.addr);
    events.write_all(get.as_bytes()).await.unwrap();
    // Read until the headers are in: the server has subscribed by then.
    let mut received = Vec::new();
    let mut buf = [0u8; 8192];
    while !String::from_utf8_lossy(&received).contains("\r\n\r\n") {
        let n = tokio::time::timeout(TIMEOUT, events.read(&mut buf))
            .await
            .expect("headers arrive")
            .unwrap();
        assert!(n > 0, "stream closed");
        received.extend_from_slice(&buf[..n]);
    }
    let headers = String::from_utf8_lossy(&received).to_lowercase();
    assert!(headers.starts_with("http/1.1 200"), "{headers}");
    assert!(
        headers.contains("content-type: text/event-stream"),
        "{headers}"
    );

    let opened = server
        .invoke(json!({"cmd": "open_repo", "args": {"path": repo.path()}}))
        .await;
    let repo_id = opened["ok"]["repoId"].clone();
    assert!(repo_id.is_number(), "{opened}");

    let event = tokio::time::timeout(TIMEOUT, async {
        loop {
            let text = String::from_utf8_lossy(&received).into_owned();
            let found = text
                .lines()
                .filter_map(|l| l.strip_prefix("data: "))
                .filter_map(|d| serde_json::from_str::<Value>(d).ok())
                .find(|e| e["name"] == "repo-changed");
            if let Some(event) = found {
                return event;
            }
            let n = events.read(&mut buf).await.unwrap();
            assert!(n > 0, "stream closed");
            received.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .expect("a repo-changed event arrives");
    assert_eq!(event["payload"]["repoId"], repo_id);
    assert!(event["payload"]["totalRows"].is_number());
}
