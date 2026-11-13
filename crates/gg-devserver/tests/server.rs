//! Runs the dev server in-process on an ephemeral port and talks raw HTTP/1.1 to it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use gg_core::Core;
use gg_core::testutil::TestRepo;
use gg_devserver::{DEFAULT_ORIGINS, Events, serve};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TIMEOUT: Duration = Duration::from_secs(20);
const JSON: (&str, &str) = ("Content-Type", "application/json");
const EVIL: &str = "http://evil.example";

struct Server {
    addr: SocketAddr,
    _config: TempDir,
}

/// A response: status line, headers (lowercased) and body.
struct Reply {
    status: String,
    headers: String,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("JSON body")
    }

    fn allow_origin(&self) -> Option<&str> {
        self.headers
            .lines()
            .find_map(|l| l.strip_prefix("access-control-allow-origin: "))
    }
}

/// Starts a server allowing the default origins and `extra`.
async fn start_allowing(extra: &[&str]) -> Server {
    let config = tempfile::tempdir().unwrap();
    let events = Events::new();
    let core: Arc<Core> = Core::new(config.path().to_path_buf(), None, events.sink())
        .await
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let origins = DEFAULT_ORIGINS
        .iter()
        .chain(extra)
        .map(|o| (*o).to_owned())
        .collect();
    tokio::spawn(serve(listener, core, events, origins));
    Server {
        addr,
        _config: config,
    }
}

async fn start() -> Server {
    start_allowing(&[]).await
}

impl Server {
    /// Sends one request with `headers` (plus `Host: <addr>` unless they name a host).
    async fn send(&self, head: &str, headers: &[(&str, &str)], body: &str) -> Reply {
        let mut stream = TcpStream::connect(self.addr).await.unwrap();
        let mut request = format!("{head} HTTP/1.1\r\nConnection: close\r\n");
        if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("host")) {
            request.push_str(&format!("Host: {}\r\n", self.addr));
        }
        for (k, v) in headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        tokio::time::timeout(TIMEOUT, stream.read_to_string(&mut response))
            .await
            .expect("response arrives")
            .unwrap();
        let (head, body) = response.split_once("\r\n\r\n").expect("headers end");
        let (status, headers) = head.split_once("\r\n").unwrap_or((head, ""));
        Reply {
            status: status.to_owned(),
            headers: headers.to_lowercase(),
            body: body.to_owned(),
        }
    }

    /// `POST /invoke` as curl would send it (JSON, no `Origin`); expects 200.
    async fn invoke(&self, body: Value) -> Value {
        let reply = self.send("POST /invoke", &[JSON], &body.to_string()).await;
        assert_eq!(reply.status, "HTTP/1.1 200 OK", "{}", reply.body);
        reply.json()
    }
}

#[tokio::test]
async fn health_is_ok() {
    let server = start().await;
    let reply = server.send("GET /health", &[], "").await;
    assert_eq!(reply.status, "HTTP/1.1 200 OK");
    assert_eq!(reply.body, "ok");
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
    let reply = server.send("POST /invoke", &[JSON], "{oops").await;
    assert_eq!(reply.status, "HTTP/1.1 200 OK");
    assert_eq!(reply.json()["err"]["code"], "bad_request");
}

const GET_APP_STATE: &str = r#"{"cmd":"get_app_state","args":{}}"#;

#[tokio::test]
async fn invoke_requires_a_json_content_type() {
    let server = start().await;
    for headers in [
        &[("Content-Type", "text/plain;charset=UTF-8")][..],
        &[("Content-Type", "application/x-www-form-urlencoded")][..],
        &[][..],
    ] {
        let reply = server.send("POST /invoke", headers, GET_APP_STATE).await;
        assert!(
            reply.status.starts_with("HTTP/1.1 415"),
            "{headers:?}: {}",
            reply.status
        );
    }
    let charset = ("Content-Type", "application/json; charset=utf-8");
    let reply = server.send("POST /invoke", &[charset], GET_APP_STATE).await;
    assert_eq!(reply.status, "HTTP/1.1 200 OK");
}

#[tokio::test]
async fn allowed_origins_get_cors_headers() {
    let server = start_allowing(&["http://127.0.0.1:5173"]).await;
    for origin in [
        "http://127.0.0.1:1420",
        "http://localhost:1420",
        "http://127.0.0.1:5173",
    ] {
        let preflight = [
            ("Origin", origin),
            ("Access-Control-Request-Method", "POST"),
            ("Access-Control-Request-Headers", "content-type"),
        ];
        let reply = server.send("OPTIONS /invoke", &preflight, "").await;
        assert!(
            reply.status.starts_with("HTTP/1.1 200"),
            "{origin}: {}",
            reply.status
        );
        assert_eq!(reply.allow_origin(), Some(origin));

        let reply = server
            .send("POST /invoke", &[("Origin", origin), JSON], GET_APP_STATE)
            .await;
        assert_eq!(reply.status, "HTTP/1.1 200 OK");
        assert_eq!(reply.allow_origin(), Some(origin));
        assert!(reply.json()["ok"].is_object());
    }
}

#[tokio::test]
async fn other_origins_are_refused() {
    let server = start().await;
    let preflight = [
        ("Origin", EVIL),
        ("Access-Control-Request-Method", "POST"),
        ("Access-Control-Request-Headers", "content-type"),
    ];
    let reply = server.send("OPTIONS /invoke", &preflight, "").await;
    assert!(reply.status.starts_with("HTTP/1.1 403"), "{}", reply.status);
    assert_eq!(reply.allow_origin(), None);

    for origin in [EVIL, "null", "http://127.0.0.1:1421.evil.example"] {
        let reply = server
            .send("POST /invoke", &[("Origin", origin), JSON], GET_APP_STATE)
            .await;
        assert!(
            reply.status.starts_with("HTTP/1.1 403"),
            "{origin}: {}",
            reply.status
        );
        assert_eq!(reply.allow_origin(), None);
        assert!(!reply.body.contains("recents"), "no data leaked");
        let reply = server.send("GET /events", &[("Origin", origin)], "").await;
        assert!(
            reply.status.starts_with("HTTP/1.1 403"),
            "{origin}: {}",
            reply.status
        );
    }
}

#[tokio::test]
async fn only_loopback_host_names_are_served() {
    let server = start().await;
    let port = server.addr.port();
    for host in [
        "evil.example",
        "evil.example:1421",
        "127.0.0.2",
        "localhost.evil.example",
    ] {
        let reply = server
            .send("POST /invoke", &[("Host", host), JSON], GET_APP_STATE)
            .await;
        assert!(
            reply.status.starts_with("HTTP/1.1 403"),
            "{host}: {}",
            reply.status
        );
        let reply = server.send("GET /events", &[("Host", host)], "").await;
        assert!(
            reply.status.starts_with("HTTP/1.1 403"),
            "{host}: {}",
            reply.status
        );
    }
    for host in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        "LocalHost".to_owned(),
        format!("[::1]:{port}"),
        "127.0.0.1".to_owned(),
    ] {
        let reply = server
            .send("POST /invoke", &[("Host", &host), JSON], GET_APP_STATE)
            .await;
        assert_eq!(reply.status, "HTTP/1.1 200 OK", "{host}");
    }
}

#[tokio::test]
async fn events_stream_repo_changed_after_open() {
    let repo = TestRepo::new();
    repo.commit("one");
    let server = start().await;

    let mut events = TcpStream::connect(server.addr).await.unwrap();
    // As the browser build's EventSource sends it.
    let get = format!(
        "GET /events HTTP/1.1\r\nHost: {}\r\nOrigin: http://127.0.0.1:1420\r\n\r\n",
        server.addr
    );
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
