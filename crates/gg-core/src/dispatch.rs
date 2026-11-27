//! `Core::dispatch`: the command table the transports call, JSON in and JSON out. Each
//! command's arguments are one struct that rejects unknown fields; arguments that do not fit
//! it, and unknown commands, are `BadRequest`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Core;
use crate::dto::{AppState, Confirmed, OpRequest, Session, ViewOptions};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::LogFilter;
use crate::launch::{AppLink, External, Programs};
use crate::mask;
use crate::store::{RepoPrefsPatch, SettingsPatch};

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
    /// A tab restored from the last session: the main worktree's folder then keeps the
    /// worktree remembered as active (opened by the user, it makes the main one active).
    #[serde(default)]
    restore: bool,
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
    #[serde(default)]
    repo_id: Option<u32>,
    #[serde(default)]
    writes_only: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunOpArgs {
    repo_id: u32,
    request: OpRequest,
    /// The dangers the user confirmed, each with its token.
    confirmed: Vec<Confirmed>,
    /// The page load (or window) that numbers its requests.
    client_id: String,
    /// The UI's number for this request, per repository and client.
    client_seq: u64,
}

/// What `open_repo` returns: the repository's snapshot, and the worktree the path asked for
/// is in when it could not be made active (an operation runs or waits in the repository):
/// the tab goes on in `activeWorktree`. A path in a linked worktree otherwise makes that
/// worktree active.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Opened {
    #[serde(flatten)]
    snapshot: crate::dto::RepoSnapshot,
    opened_worktree: Option<String>,
}

/// What `uninstall_cli` returns: the command's state, the files it removed (none when
/// there was nothing to remove), and what became of gg's line in a shell file.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Uninstalled {
    #[serde(flatten)]
    state: crate::dto::CliState,
    removed: Vec<String>,
    #[serde(flatten)]
    line: crate::dto::ShellLineRemoved,
}

/// What `install_cli` and `turn_off_shell_gg` return: the command's state, and what adding
/// gg's line to a shell file did (`null` when it was not asked for, or there was nothing to
/// turn off).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CliWithLine {
    #[serde(flatten)]
    state: crate::dto::CliState,
    shell_line_added: Option<crate::dto::ShellLineAdded>,
}

/// What `remove_shell_line` returns: the command's state, and what taking the line out did.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CliLineRemoved {
    #[serde(flatten)]
    state: crate::dto::CliState,
    #[serde(flatten)]
    line: crate::dto::ShellLineRemoved,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RefreshArgs {
    repo_id: u32,
    /// The user asked (the Refresh button or shortcut): the pull requests load too.
    #[serde(default)]
    prs: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrSuggestArgs {
    repo_id: u32,
    branch: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrDetailsArgs {
    repo_id: u32,
    number: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetActiveWorktreeArgs {
    repo_id: u32,
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorktreeFilesArgs {
    repo_id: u32,
    path: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefaultPathArgs {
    repo_id: u32,
    branch: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StopOpArgs {
    repo_id: u32,
    /// The operation to stop (`RunningOp.id`): nothing happens if another one runs by then.
    op_id: u64,
}

/// Just the numbering of a `run_op`, read before the rest so that a request whose other
/// arguments are wrong still gives up its number.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunOpNumber {
    repo_id: u32,
    client_id: String,
    client_seq: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepoPrefsArgs {
    repo_id: u32,
    patch: RepoPrefsPatch,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenInEditorArgs {
    repo_id: u32,
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenExternalArgs {
    repo_id: u32,
    target: External,
    /// A worktree of the repository (editor, terminal, reveal); the active one when absent.
    #[serde(default)]
    path: Option<PathBuf>,
    /// The web page (url).
    #[serde(default)]
    url: Option<String>,
}

/// What `open_external` returns: a note for the user when something else than asked was
/// opened.
#[derive(serde::Serialize)]
struct OpenedExternal {
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppLinkArgs {
    link: AppLink,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstallCliArgs {
    dir: PathBuf,
    /// The user ticked "Turn off that alias for me": add gg's line to their shell file too.
    #[serde(default)]
    turn_off: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AiToolsArgs {
    /// Also ask each tool found its version (the settings); else only look for them.
    #[serde(default)]
    versions: bool,
    /// Read the login shell's `PATH` again first ("Check again": a tool installed since).
    #[serde(default)]
    recheck: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AiGenerateArgs {
    repo_id: u32,
    /// The UI's name for this generation, to cancel it by.
    request_id: String,
    task: crate::dto::AiTask,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AiCancelArgs {
    request_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PerfMarkArgs {
    name: String,
    ms: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PrInboxArgs {
    /// Ask gh again (at most once every 10 s); else what gg kept.
    #[serde(default)]
    refresh: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InboxKeyArgs {
    key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InboxPrArgs {
    host: String,
    owner: String,
    name: String,
    number: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InboxWriteArgs {
    host: String,
    owner: String,
    name: String,
    /// `pr_reply`, `pr_resolve` or `pr_comment`, as the Pull request tab sends it.
    request: OpRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InboxMatchArgs {
    host: String,
    owner: String,
    name: String,
    /// The pull request's head repository, `owner/name`.
    #[serde(default)]
    head_repo: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenWebArgs {
    target: External,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CloneArgs {
    clone_id: String,
    host: String,
    owner: String,
    name: String,
    parent: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CloneCancelArgs {
    clone_id: String,
}

impl Core {
    /// Runs command `cmd` with `args` (a JSON object) and returns its result as JSON. The
    /// commands and their arguments and results are the Commands table of the boundary
    /// types; `fetch` returns as soon as the fetch has started.
    ///
    /// Credentials in URLs are masked in the result (see [`crate::mask`]) and in errors.
    pub async fn dispatch(self: &Arc<Self>, cmd: &str, args: Value) -> Result<Value> {
        match self.run_command(cmd, args).await {
            Ok(value) if mask::masks_result_of(cmd) => Ok(mask::mask_json(value)),
            Ok(value) => Ok(value),
            Err(e) => Err(e.masked()),
        }
    }

    async fn run_command(self: &Arc<Self>, cmd: &str, args: Value) -> Result<Value> {
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
                // Applied in the same turn as the write, so two quick changes apply in the
                // order they are saved.
                let settings = self
                    .blocking(move |core| {
                        let settings = core.store.update_settings(a.patch)?;
                        core.apply_settings(&settings);
                        Ok(settings)
                    })
                    .await?;
                to_json(settings)
            }
            "restore_default_settings" => {
                let NoArgs {} = parse(cmd, args)?;
                let settings = self
                    .blocking(|core| {
                        let settings = core.store.restore_default_settings()?;
                        core.apply_settings(&settings);
                        Ok(settings)
                    })
                    .await?;
                to_json(settings)
            }
            "reset_settings" => {
                let NoArgs {} = parse(cmd, args)?;
                self.blocking(|core| core.store.reset()).await?;
                Ok(Value::Null)
            }
            "take_opened_paths" => {
                let NoArgs {} = parse(cmd, args)?;
                to_json(self.take_opened_paths())
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
                let (snapshot, opened_worktree) = self
                    .repos()?
                    .open_noting_worktree(&a.path, !a.restore)
                    .await?;
                // The repository is open either way; a recent that cannot be saved is only
                // missing from the next start's list.
                let root = snapshot.root_path.clone();
                let _ = self
                    .blocking(move |core| core.store.add_recent(Path::new(&root)))
                    .await;
                to_json(Opened {
                    snapshot,
                    opened_worktree,
                })
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
            "search_bodies" => {
                let a: SearchArgs = parse(cmd, args)?;
                to_json(self.repos()?.search_bodies(a.repo_id, &a.query).await?)
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
                // Said now: the fetch itself is refused too, but on its own task.
                repos.refuse_if_restarting()?;
                // On its own task, so a caller that goes away (a closed HTTP request) does
                // not cancel it.
                tokio::spawn(async move {
                    let _ = repos.fetch(a.repo_id).await;
                });
                Ok(Value::Null)
            }
            "refresh" => {
                let a: RefreshArgs = parse(cmd, args)?;
                let repos = self.repos()?;
                if a.prs {
                    to_json(repos.refresh_by_user(a.repo_id).await?)
                } else {
                    to_json(repos.refresh(a.repo_id).await?)
                }
            }
            "pr_suggest" => {
                let a: PrSuggestArgs = parse(cmd, args)?;
                to_json(self.repos()?.pr_suggest(a.repo_id, &a.branch).await?)
            }
            "pr_details" => {
                let a: PrDetailsArgs = parse(cmd, args)?;
                to_json(self.repos()?.pr_details(a.repo_id, a.number).await?)
            }
            "pr_inbox" => {
                let a: PrInboxArgs = parse(cmd, args)?;
                to_json(self.repos()?.pr_inbox(a.refresh).await?)
            }
            "pr_inbox_seen" => {
                let a: InboxKeyArgs = parse(cmd, args)?;
                self.repos()?.pr_inbox_seen(&a.key).await?;
                Ok(Value::Null)
            }
            "inbox_pr_details" => {
                let a: InboxPrArgs = parse(cmd, args)?;
                to_json(
                    self.repos()?
                        .inbox_pr_details(&a.host, &a.owner, &a.name, a.number)
                        .await?,
                )
            }
            "inbox_pr_write" => {
                let a: InboxWriteArgs = parse(cmd, args)?;
                to_json(
                    self.repos()?
                        .inbox_pr_write(&a.host, &a.owner, &a.name, &a.request)
                        .await?,
                )
            }
            "inbox_repo_match" => {
                let a: InboxMatchArgs = parse(cmd, args)?;
                to_json(
                    self.repos()?
                        .inbox_repo_match(&a.host, &a.owner, &a.name, a.head_repo.as_deref())
                        .await?,
                )
            }
            "open_web" => {
                let a: OpenWebArgs = parse(cmd, args)?;
                let repos = self.repos()?;
                let hosts = repos.inbox_host_names();
                to_json(OpenedExternal {
                    note: repos.open_web(a.target, &a.url, &hosts)?,
                })
            }
            "clone_parent" => {
                let NoArgs {} = parse(cmd, args)?;
                to_json(self.repos()?.clone_parent())
            }
            "clone_repo" => {
                let a: CloneArgs = parse(cmd, args)?;
                let repos = self.repos()?.clone();
                // On its own task, so a caller that goes away does not leave git cloning
                // unseen: it ends, or Cancel ends it.
                let run = tokio::spawn(async move {
                    repos
                        .clone_repo(&a.clone_id, &a.host, &a.owner, &a.name, &a.parent)
                        .await
                });
                let cloned = run.await.map_err(|e| {
                    CoreError::new(ErrorCode::Io, "The clone failed").with_detail(e.to_string())
                })??;
                to_json(cloned)
            }
            "clone_cancel" => {
                let a: CloneCancelArgs = parse(cmd, args)?;
                to_json(self.repos()?.clone_cancel(&a.clone_id))
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
                to_json(self.log.query(LogFilter {
                    since_id: a.since_id,
                    repo_id: a.repo_id,
                    writes_only: a.writes_only,
                }))
            }
            "run_op" => {
                let number = serde_json::from_value::<RunOpNumber>(args.clone()).ok();
                let a: RunOpArgs = match parse(cmd, args) {
                    Ok(a) => a,
                    Err(e) => {
                        if let (Some(n), Ok(repos)) = (number, self.repos()) {
                            repos.release_number(n.repo_id, &n.client_id, n.client_seq);
                        }
                        return Err(e);
                    }
                };
                to_json(
                    self.repos()?
                        .run_op_in_order(
                            a.repo_id,
                            &a.client_id,
                            a.client_seq,
                            a.request,
                            &a.confirmed,
                        )
                        .await?,
                )
            }
            "set_active_worktree" => {
                let a: SetActiveWorktreeArgs = parse(cmd, args)?;
                to_json(
                    self.repos()?
                        .set_active_worktree(a.repo_id, &a.path)
                        .await?,
                )
            }
            "worktree_files" => {
                let a: WorktreeFilesArgs = parse(cmd, args)?;
                to_json(self.repos()?.worktree_files(a.repo_id, &a.path).await?)
            }
            "branch_files" => {
                let a: WorktreeFilesArgs = parse(cmd, args)?;
                to_json(self.repos()?.branch_files(a.repo_id, &a.path).await?)
            }
            "worktree_default_path" => {
                let a: DefaultPathArgs = parse(cmd, args)?;
                let pattern = self.store.settings().worktree_pattern;
                to_json(
                    self.repos()?
                        .worktree_default_path(a.repo_id, &a.branch, &pattern)
                        .await?,
                )
            }
            "stop_op" => {
                let a: StopOpArgs = parse(cmd, args)?;
                to_json(self.repos()?.stop_op(a.repo_id, a.op_id)?)
            }
            "get_repo_prefs" => {
                let a: RepoArgs = parse(cmd, args)?;
                let root = self.repo_root(a.repo_id)?;
                to_json(self.store.repo_prefs(&root))
            }
            "set_repo_prefs" => {
                let a: RepoPrefsArgs = parse(cmd, args)?;
                let root = self.repo_root(a.repo_id)?;
                to_json(
                    self.blocking(move |core| core.store.set_repo_prefs(&root, a.patch))
                        .await?,
                )
            }
            "open_in_editor" => {
                let a: OpenInEditorArgs = parse(cmd, args)?;
                let command = self.store.settings().editor_command;
                let note = self
                    .repos()?
                    .open_in_editor(a.repo_id, a.path.as_deref(), command.as_deref())
                    .await?;
                to_json(note)
            }
            "open_external" => {
                let a: OpenExternalArgs = parse(cmd, args)?;
                let settings = self.store.settings();
                let programs = Programs {
                    editor: settings.editor_command.as_deref(),
                    terminal: settings.terminal_command.as_deref(),
                };
                let note = self
                    .repos()?
                    .open_external(
                        a.repo_id,
                        a.target,
                        a.path.as_deref(),
                        a.url.as_deref(),
                        programs,
                    )
                    .await?;
                to_json(OpenedExternal { note })
            }
            "open_app_link" => {
                let a: AppLinkArgs = parse(cmd, args)?;
                let file = self.store.file();
                let note = self.repos()?.open_app_link(a.link, &file)?;
                to_json(OpenedExternal { note })
            }
            "install_cli" => {
                let a: InstallCliArgs = parse(cmd, args)?;
                let (state, shell_line_added) = self
                    .off_runtime(move |core| core.install_cli(&a.dir, a.turn_off))
                    .await?;
                to_json(CliWithLine {
                    state,
                    shell_line_added,
                })
            }
            "turn_off_shell_gg" => {
                let NoArgs {} = parse(cmd, args)?;
                let (state, added) = self.off_runtime(|core| core.turn_off_shell_gg()).await?;
                to_json(CliWithLine {
                    state,
                    shell_line_added: Some(added),
                })
            }
            "remove_shell_line" => {
                let NoArgs {} = parse(cmd, args)?;
                let (state, line) = self
                    .off_runtime(|core| Ok(core.remove_shell_line()))
                    .await?;
                to_json(CliLineRemoved { state, line })
            }
            "uninstall_cli" => {
                let NoArgs {} = parse(cmd, args)?;
                let (removed, line, state) = self.off_runtime(|core| core.uninstall_cli()).await?;
                to_json(Uninstalled {
                    state,
                    removed: removed
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                    line,
                })
            }
            "update_status" => {
                let NoArgs {} = parse(cmd, args)?;
                to_json(self.updates.status())
            }
            "start_update_checks" => {
                let NoArgs {} = parse(cmd, args)?;
                self.updates.start();
                to_json(self.updates.status())
            }
            "check_for_updates" => {
                let NoArgs {} = parse(cmd, args)?;
                // On its own task: the status arrives as `update-changed` events.
                self.updates.check_soon();
                Ok(Value::Null)
            }
            "restart_to_update" => {
                let NoArgs {} = parse(cmd, args)?;
                // From here until gg exits, or the install fails, no operation or fetch the
                // user asks for is admitted; refused while one runs.
                let repos = self.repos().ok().cloned();
                if let Some(repos) = &repos {
                    repos.begin_restart()?;
                }
                // The store's writes asked for before this (the open tabs) land first.
                let installed = {
                    let _turn = self.io_turn.lock().await;
                    self.off_runtime(|core| core.updates.install_and_restart())
                        .await
                };
                if installed.is_err()
                    && let Some(repos) = &repos
                {
                    repos.end_restart();
                }
                installed?;
                Ok(Value::Null)
            }
            "ai_tools" => {
                let a: AiToolsArgs = parse(cmd, args)?;
                let settings = self.store.settings();
                to_json(
                    self.repos()?
                        .ai_tools(&settings, a.versions, a.recheck)
                        .await,
                )
            }
            "ai_generate" => {
                let a: AiGenerateArgs = parse(cmd, args)?;
                let settings = self.store.settings();
                let repos = self.repos()?.clone();
                // On its own task, so a caller that goes away (a closed HTTP request) does not
                // leave the tool running unseen: it ends, or Cancel ends it.
                let run = tokio::spawn(async move {
                    repos
                        .ai_generate(a.repo_id, &a.request_id, a.task, &settings)
                        .await
                });
                let outcome = run.await.map_err(|e| {
                    CoreError::new(ErrorCode::Io, "The generation failed")
                        .with_detail(e.to_string())
                })??;
                to_json(outcome)
            }
            "ai_cancel" => {
                let a: AiCancelArgs = parse(cmd, args)?;
                to_json(self.repos()?.ai_cancel(&a.request_id))
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

    /// Puts settings just saved into effect: the fetch interval, the automatic update checks.
    fn apply_settings(&self, settings: &crate::dto::Settings) {
        if let Ok(repos) = self.repos() {
            repos.set_fetch_interval(settings.fetch_interval_secs);
        }
        self.updates.set_enabled(settings.check_for_updates);
    }

    /// Whether automatic update checks are on, as applied (tests).
    #[cfg(any(test, feature = "testutil"))]
    pub fn update_checks_on(&self) -> bool {
        self.updates.automatic()
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

    /// Runs `f`, which blocks (writes a file, waits for the login shell), on Tokio's blocking
    /// pool, without waiting for the store's turn: it does not touch the store.
    async fn off_runtime<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Core) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let core = self.clone();
        tokio::task::spawn_blocking(move || f(&core))
            .await
            .unwrap_or_else(|e| {
                Err(CoreError::new(ErrorCode::Io, "The task failed").with_detail(e.to_string()))
            })
    }

    /// The main worktree of open repository `repo_id`: what its preferences are kept by.
    fn repo_root(&self, repo_id: u32) -> Result<PathBuf> {
        let repos = self.repos()?;
        repos.check_open(repo_id)?;
        repos
            .git_dirs(repo_id)
            .map(|(_, root)| root)
            .ok_or_else(|| {
                CoreError::new(
                    ErrorCode::UnknownRepo,
                    format!("No open repository has id {repo_id}."),
                )
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
            settings_file: self.store.file().to_string_lossy().into_owned(),
            git_problem: self.repos().ok().and_then(|r| r.git_problem()),
            cli: self.cli_state(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
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
