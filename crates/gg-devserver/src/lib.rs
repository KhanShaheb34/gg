//! Serves gg-core over HTTP so the UI can run in a plain browser, where Playwright and other
//! tools can drive it. A development and test tool only: the app never includes it.
//!
//! - `POST /invoke` with `{cmd, args}` as `application/json` → 200 `{ok: value}` or 200
//!   `{err: CoreErrorDto}`; any other content type → 415.
//! - `GET /events` → server-sent events, each `data: {"name": .., "payload": ..}`, with a
//!   keep-alive comment every 15 s.
//! - `GET /health` → `ok`.
//!
//! The API can open any folder and run git in it, so web pages the developer happens to have
//! open must not reach it:
//! - every request whose `Host` is not a loopback name gets 403 (DNS rebinding), as does
//!   every request with an `Origin` outside the allow-list;
//! - CORS answers only the allowed origins, and since `/invoke` takes only JSON, a browser
//!   must preflight before it can post.
//!
//! The caller binds the listener (the binary binds 127.0.0.1 only).

pub mod fake_updates;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use gg_core::{Core, CoreError, ErrorCode, EventSink};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt as _, wrappers::BroadcastStream};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Origins allowed by default: the vite dev server that serves the browser build.
pub const DEFAULT_ORIGINS: [&str; 2] = ["http://127.0.0.1:1420", "http://localhost:1420"];

/// Events buffered per `/events` client. A client that falls further behind misses the
/// oldest ones.
const EVENT_BUFFER: usize = 1024;
/// How often an idle event stream sends a comment, so proxies and clients keep it open.
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// How long browsers may cache a preflight answer.
const PREFLIGHT_MAX_AGE: Duration = Duration::from_secs(600);

/// The core's events, fanned out to every `/events` client. Create it first, give
/// [`Events::sink`] to [`Core::new`], then pass it to [`serve`].
#[derive(Clone)]
pub struct Events {
    /// Each event as its SSE data: `{"name": .., "payload": ..}`.
    tx: broadcast::Sender<String>,
}

impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

impl Events {
    pub fn new() -> Self {
        Self {
            tx: broadcast::channel(EVENT_BUFFER).0,
        }
    }

    /// The sink for [`Core::new`].
    pub fn sink(&self) -> Arc<dyn EventSink> {
        Arc::new(self.clone())
    }
}

impl EventSink for Events {
    fn emit(&self, name: &str, payload: Value) {
        // No clients is not an error.
        let _ = self
            .tx
            .send(json!({ "name": name, "payload": payload }).to_string());
    }
}

#[derive(Clone)]
struct AppState {
    core: Arc<Core>,
    events: Events,
}

/// Serves `core` on `listener` until the process ends (or the listener fails). Browsers
/// may call it only from `allowed_origins` (exact origins such as `http://127.0.0.1:1420`;
/// see [`DEFAULT_ORIGINS`]). Fails at once with `InvalidInput` if an origin is not a valid
/// header value or is `*`.
pub async fn serve(
    listener: TcpListener,
    core: Arc<Core>,
    events: Events,
    allowed_origins: Vec<String>,
) -> std::io::Result<()> {
    let origins = allowed_origins
        .iter()
        .map(|o| match HeaderValue::from_str(o) {
            Ok(v) if o != "*" => Ok(v),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("not an allowed origin: {o:?}"),
            )),
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    axum::serve(listener, router(core, events, origins)).await
}

fn router(core: Arc<Core>, events: Events, origins: Vec<HeaderValue>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins.clone()))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([CONTENT_TYPE])
        .max_age(PREFLIGHT_MAX_AGE);
    let origins = Arc::new(origins);
    Router::new()
        .route("/invoke", post(invoke))
        .route("/events", get(stream_events))
        .route("/health", get(|| async { "ok" }))
        .layer(cors)
        // Outermost, so refused requests (preflights included) never get CORS headers.
        .layer(middleware::from_fn(move |request: Request, next: Next| {
            let origins = origins.clone();
            async move { guard(&origins, request, next).await }
        }))
        .with_state(AppState { core, events })
}

/// Refuses (403) requests whose `Host` is not a loopback name, or whose `Origin` is present
/// but not allowed.
async fn guard(origins: &[HeaderValue], request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host_ok = headers
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(is_loopback_host);
    let origin_ok = headers.get(ORIGIN).is_none_or(|o| origins.contains(o));
    if !host_ok || !origin_ok {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    next.run(request).await
}

/// Whether `host` (a `Host` header) is `127.0.0.1`, `localhost` or `[::1]`, with or without
/// a port.
fn is_loopback_host(host: &str) -> bool {
    let (name, port) = match host.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((name, after)) => match after {
                "" => (name, None),
                _ => match after.strip_prefix(':') {
                    Some(port) => (name, Some(port)),
                    None => return false,
                },
            },
            None => return false,
        },
        None => match host.split_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host, None),
        },
    };
    let port_ok = port.is_none_or(|p| p.parse::<u16>().is_ok());
    let name_ok = name == "127.0.0.1" || name == "::1" || name.eq_ignore_ascii_case("localhost");
    port_ok && name_ok
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeRequest {
    cmd: String,
    #[serde(default = "empty_object")]
    args: Value,
}

fn empty_object() -> Value {
    json!({})
}

/// Whether the request says its body is JSON (`application/json`, parameters allowed).
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

/// `POST /invoke`.
async fn invoke(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !is_json(&headers) {
        let err = CoreError::new(
            ErrorCode::BadRequest,
            "Send the request as application/json",
        );
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(json!({ "err": err })),
        )
            .into_response();
    }
    let request: InvokeRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            let err = CoreError::new(ErrorCode::BadRequest, "Malformed request body")
                .with_detail(e.to_string());
            return Json(json!({ "err": err })).into_response();
        }
    };
    let core = state.core;
    // On its own task, so a client that disconnects does not cancel a command halfway.
    let done = tokio::spawn(async move { core.dispatch(&request.cmd, request.args).await }).await;
    Json(match done {
        Ok(Ok(value)) => json!({ "ok": value }),
        Ok(Err(err)) => json!({ "err": err }),
        Err(e) => {
            let err = CoreError::new(ErrorCode::Io, "The command failed unexpectedly")
                .with_detail(e.to_string());
            json!({ "err": err })
        }
    })
    .into_response()
}

/// `GET /events`.
async fn stream_events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = BroadcastStream::new(state.events.tx.subscribe())
        // An error means this client fell behind and missed some events; carry on.
        .filter_map(|message| message.ok().map(|data| Ok(Event::default().data(data))));
    Sse::new(events).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:1421",
            "localhost",
            "LOCALHOST:80",
            "[::1]",
            "[::1]:1421",
        ] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "",
            "evil.example",
            "localhost.evil.example",
            "127.0.0.1.evil.example",
            "127.0.0.2",
            "localhost:99999",
            "localhost:x",
            "[::1]x",
            "[::2]:1421",
            "::1",
            "[::1",
            "0.0.0.0:1421",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }
}
