//! Serves gg-core over HTTP so the UI can run in a plain browser, where Playwright and agents
//! can drive it. A development and test tool only: the app never includes it.
//!
//! - `POST /invoke` with `{cmd, args}` → 200 `{ok: value}` or 200 `{err: CoreErrorDto}`.
//! - `GET /events` → server-sent events, each `data: {"name": .., "payload": ..}`, with a
//!   keep-alive comment every 15 s.
//! - `GET /health` → `ok`.
//!
//! CORS is permissive; the caller binds the listener (the binary binds 127.0.0.1 only).

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::response::Json;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use gg_core::{Core, CoreError, ErrorCode, EventSink};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt as _, wrappers::BroadcastStream};
use tower_http::cors::CorsLayer;

/// Events buffered per `/events` client. A client that falls further behind misses the
/// oldest ones.
const EVENT_BUFFER: usize = 1024;
/// How often an idle event stream sends a comment, so proxies and clients keep it open.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

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

/// Serves `core` on `listener` until the process ends (or the listener fails).
pub async fn serve(listener: TcpListener, core: Arc<Core>, events: Events) -> std::io::Result<()> {
    axum::serve(listener, router(core, events)).await
}

fn router(core: Arc<Core>, events: Events) -> Router {
    Router::new()
        .route("/invoke", post(invoke))
        .route("/events", get(stream_events))
        .route("/health", get(|| async { "ok" }))
        .layer(CorsLayer::permissive())
        .with_state(AppState { core, events })
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

/// `POST /invoke`. The body is read as JSON whatever its content type, so browsers can send
/// it as a "simple" request without a CORS preflight.
async fn invoke(State(state): State<AppState>, body: Bytes) -> Json<Value> {
    let request: InvokeRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            let err = CoreError::new(ErrorCode::BadRequest, "Malformed request body")
                .with_detail(e.to_string());
            return Json(json!({ "err": err }));
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
