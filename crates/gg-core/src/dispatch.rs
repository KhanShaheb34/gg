//! `Core::dispatch`: the command table the transports call, JSON in and JSON out. Each
//! command's arguments are one struct that rejects unknown fields; arguments that do not fit
//! it, and unknown commands, are `BadRequest`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Core;
use crate::dto::{AppState, Session, ViewOptions};
use crate::error::{CoreError, ErrorCode, Result};
use crate::store::SettingsPatch;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveSessionArgs {
    tabs: Vec<String>,
    active: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateSettingsArgs {
    patch: SettingsPatch,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoveRecentArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenRepoArgs {
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepoArgs {
    repo_id: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GetRowsArgs {
    repo_id: u32,
    generation: u64,
    start: u32,
    count: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HashArgs {
    repo_id: u32,
    hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchArgs {
    repo_id: u32,
    query: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ViewOptionsArgs {
    repo_id: u32,
    options: ViewOptions,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FocusArgs {
    focused: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommandLogArgs {
    since_id: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PerfMarkArgs {
    name: String,
    ms: f64,
}

impl Core {
    /// Runs command `cmd` with `args` (a JSON object) and returns its result as JSON. The
    /// commands and their arguments and results are the Commands table of the boundary
    /// types; `fetch` returns as soon as the fetch has started.
    pub async fn dispatch(self: &Arc<Self>, cmd: &str, args: Value) -> Result<Value> {
        match cmd {
            "get_app_state" => {
                let NoArgs {} = parse(cmd, args)?;
                to_json(self.app_state())
            }
            "save_session" => {
                let a: SaveSessionArgs = parse(cmd, args)?;
                let session = Session {
                    tabs: a.tabs,
                    active: a.active,
                };
                self.blocking(move |core| core.store.save_session(session))
                    .await?;
                Ok(Value::Null)
            }
            "update_settings" => {
                let a: UpdateSettingsArgs = parse(cmd, args)?;
                let settings = self
                    .blocking(move |core| core.store.update_settings(a.patch))
                    .await?;
                if let Ok(repos) = self.repos() {
                    repos.set_fetch_interval(settings.fetch_interval_secs);
                }
                to_json(settings)
            }
            "reset_settings" => {
                let NoArgs {} = parse(cmd, args)?;
                self.blocking(|core| core.store.reset()).await?;
                Ok(Value::Null)
            }
            "remove_recent" => {
                let a: RemoveRecentArgs = parse(cmd, args)?;
                to_json(
                    self.blocking(move |core| core.store.remove_recent(&a.path))
                        .await?,
                )
            }
            "open_repo" => {
                let a: OpenRepoArgs = parse(cmd, args)?;
                let snapshot = self.repos()?.open(&a.path).await?;
                // The repository is open either way; a recent that cannot be saved is only
                // missing from the next start's list.
                let root = snapshot.root_path.clone();
                let _ = self
                    .blocking(move |core| core.store.add_recent(Path::new(&root)))
                    .await;
                to_json(snapshot)
            }
            "close_repo" => {
                let a: RepoArgs = parse(cmd, args)?;
                self.repos()?.close(a.repo_id).await?;
                Ok(Value::Null)
            }
            "get_snapshot" => {
                let a: RepoArgs = parse(cmd, args)?;
                to_json(self.repos()?.snapshot(a.repo_id).await?)
            }
            "get_rows" => {
                let a: GetRowsArgs = parse(cmd, args)?;
                let page = self
                    .repos()?
                    .rows(a.repo_id, a.generation, a.start, a.count)
                    .await?;
                to_json(page)
            }
            "find_row" => {
                let a: HashArgs = parse(cmd, args)?;
                to_json(self.repos()?.find_row(a.repo_id, &a.hash).await?)
            }
            "search_commits" => {
                let a: SearchArgs = parse(cmd, args)?;
                to_json(self.repos()?.search(a.repo_id, &a.query).await?)
            }
            "commit_details" => {
                let a: HashArgs = parse(cmd, args)?;
                to_json(self.repos()?.details(a.repo_id, &a.hash).await?)
            }
            "set_view_options" => {
                let a: ViewOptionsArgs = parse(cmd, args)?;
                let snapshot = self.repos()?.set_view_options(a.repo_id, a.options).await?;
                to_json(snapshot)
            }
            "fetch" => {
                let a: RepoArgs = parse(cmd, args)?;
                let repos = self.repos()?.clone();
                // Fails for an unknown repository; the fetch's own outcome arrives as events.
                repos.check_open(a.repo_id)?;
                // On its own task, so a caller that goes away (a closed HTTP request) does
                // not cancel it.
                tokio::spawn(async move {
                    let _ = repos.fetch(a.repo_id).await;
                });
                Ok(Value::Null)
            }
            "refresh" => {
                let a: RepoArgs = parse(cmd, args)?;
                to_json(self.repos()?.refresh(a.repo_id).await?)
            }
            "set_window_focused" => {
                let a: FocusArgs = parse(cmd, args)?;
                // Without git there is nothing that depends on focus.
                if let Ok(repos) = self.repos() {
                    repos.set_window_focused(a.focused);
                }
                Ok(Value::Null)
            }
            "command_log" => {
                let a: CommandLogArgs = parse(cmd, args)?;
                to_json(self.log.since(a.since_id))
            }
            "perf_mark" => {
                let a: PerfMarkArgs = parse(cmd, args)?;
                self.blocking(move |core| core.perf_mark(&a.name, a.ms))
                    .await?;
                Ok(Value::Null)
            }
            _ => Err(CoreError::new(
                ErrorCode::BadRequest,
                format!("Unknown command: {cmd}"),
            )),
        }
    }

    /// Runs `f`, which does file I/O (the store writes and syncs `state.json` under its
    /// lock), on Tokio's blocking pool rather than on an async worker, after the calls made
    /// before it (a fair queue), so writes land in call order.
    async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Core) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let _turn = self.io_turn.lock().await;
        let core = self.clone();
        tokio::task::spawn_blocking(move || f(&core))
            .await
            .unwrap_or_else(|e| {
                Err(CoreError::new(ErrorCode::Io, "Could not save").with_detail(e.to_string()))
            })
    }

    fn app_state(&self) -> AppState {
        AppState {
            recents: self.store.recents(),
            session: self.store.session(),
            settings: self.store.settings(),
            launch_path: self.launch_path.clone(),
            home_dir: home_dir(),
            settings_problem: self.store.problem(),
        }
    }
}

/// The user's home folder from the environment (`USERPROFILE` on Windows, `HOME` elsewhere),
/// if set and not empty.
fn home_dir() -> Option<String> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
}

/// `args` as `cmd`'s argument struct; `BadRequest` with serde's message as `detail` if they
/// do not fit.
fn parse<T: DeserializeOwned>(cmd: &str, args: Value) -> Result<T> {
    let bad = || CoreError::new(ErrorCode::BadRequest, format!("Bad arguments for {cmd}"));
    // Serde would also take an array for a struct; arguments are always named.
    if !args.is_object() {
        return Err(bad().with_detail(format!("expected a JSON object, got {args}")));
    }
    serde_json::from_value(args).map_err(|e| bad().with_detail(e.to_string()))
}

fn to_json(value: impl Serialize) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| {
        CoreError::new(ErrorCode::Io, "Could not encode the result").with_detail(e.to_string())
    })
}
