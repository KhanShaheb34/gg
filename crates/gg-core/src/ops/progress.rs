//! What git stopped in (a merge, rebase, cherry-pick or revert, started here or outside the
//! app): continuing it, aborting it, refusing what would change HEAD or the working tree
//! meanwhile, and opening files in an editor to resolve conflicts.
//!
//! The app never edits conflicted files: continuing stages the paths that are unmerged at
//! that moment (the user resolved them; the UI asked them to confirm first) and nothing
//! else.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use crate::dto::{InProgressKind, OpRequest};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::progress::read_kind;
use crate::git::status::read_conflicted;
use crate::ops::explain::{explain, recognised};
use crate::ops::local::{current_branch, kind_name, stopped_message, stopped_without_conflicts};
use crate::ops::{OpCtx, Step};
use crate::repo::RepoManager;

/// Most paths given to one `git add`, so the command line stays short on any platform.
const ADD_CHUNK: usize = 100;

/// The error for continue or abort with nothing in progress.
pub(crate) fn nothing_in_progress() -> CoreError {
    CoreError::new(
        ErrorCode::NothingToDo,
        "Nothing is in progress: there is no merge, rebase, cherry-pick or revert to continue \
         or abort.",
    )
}

/// Why `request` cannot run while git is stopped in `kind`, if it cannot: it would move
/// HEAD or change the index or working tree under the stopped operation. `head` is the
/// branch HEAD is on. Continue and abort are not refused here.
pub(crate) fn refused_while(
    request: &OpRequest,
    kind: InProgressKind,
    head: Option<&str>,
) -> Option<String> {
    let changes_head_or_tree = match request {
        OpRequest::CheckoutBranch { .. }
        | OpRequest::CheckoutRemote { .. }
        | OpRequest::CheckoutCommit { .. }
        | OpRequest::Merge { .. }
        | OpRequest::Rebase { .. }
        | OpRequest::CherryPick { .. }
        | OpRequest::Revert { .. }
        | OpRequest::Reset { .. }
        | OpRequest::StashSave { .. }
        | OpRequest::StashApply { .. }
        | OpRequest::StashPop { .. }
        | OpRequest::StashBranch { .. } => true,
        OpRequest::CreateBranch { checkout, .. } => *checkout,
        // Pulling a branch that is not checked out only fetches into it.
        OpRequest::Pull { branch, .. } => head == Some(branch.as_str()),
        _ => false,
    };
    changes_head_or_tree.then(|| {
        format!(
            "A {} is in progress: continue it or abort it first. Until then this would \
             change what it is working on.",
            kind_name(kind)
        )
    })
}

/// The title of continue (`verb` "Continue") or abort ("Abort") for what is in progress.
pub(crate) fn title(verb: &str, kind: Option<InProgressKind>) -> String {
    match kind {
        Some(kind) => format!("{verb} the {}", kind_name(kind)),
        None => verb.to_owned(),
    }
}

/// Continues what git stopped in: stages the paths unmerged now (`add -- <paths>`; a path
/// resolved by deleting it stages the deletion), then `<kind> --continue` with editors
/// suppressed. Stopped if git stops on conflicts again (a rebase's next step), failed with
/// a plain line if it stops otherwise.
pub(crate) async fn continue_op(ctx: &OpCtx<'_>) -> Result<Step> {
    let Some(kind) = read_kind(&ctx.runner, &ctx.cwd).await? else {
        return Err(nothing_in_progress());
    };
    let unmerged = read_conflicted(&ctx.runner, &ctx.cwd).await?;
    for chunk in unmerged.chunks(ADD_CHUNK) {
        // Literal: a path like `[ab].txt` must not match other files as a glob.
        let mut args = vec!["--literal-pathspecs", "add", "--"];
        args.extend(chunk.iter().map(String::as_str));
        let out = ctx.git(&args).await?;
        if out.code != 0 {
            return Ok(explain(&args, &out));
        }
    }
    let args: &[&str] = match kind {
        // `merge --continue` takes no options; it commits with the prepared message.
        InProgressKind::Merge => &["merge", "--continue"],
        InProgressKind::Rebase => &["rebase", "--continue"],
        InProgressKind::CherryPick => &["cherry-pick", "--continue", "--no-edit"],
        InProgressKind::Revert => &["revert", "--continue", "--no-edit"],
    };
    let out = ctx.git(args).await?;
    let after = read_kind(&ctx.runner, &ctx.cwd).await?;
    let what = kind_name(kind);
    let detail = if out.stderr.trim().is_empty() {
        out.text()
    } else {
        format!("{}\n{}", out.stderr.trim_end(), out.text())
            .trim()
            .to_owned()
    };
    let Some(still) = after else {
        if out.code != 0 {
            return Ok(explain(args, &out));
        }
        let branch = current_branch(ctx).await?;
        return Ok(Step::Done(match (kind, branch) {
            (InProgressKind::Rebase, Some(branch)) => format!("Finished rebasing {branch}."),
            _ => format!("Finished the {what}: it is committed."),
        }));
    };
    let conflicted = read_conflicted(&ctx.runner, &ctx.cwd).await?;
    if !conflicted.is_empty() {
        return Ok(Step::Stopped {
            message: stopped_message(still),
            detail,
        });
    }
    if out.code == 0 {
        // An interactive rebase stopping where it was told to (`edit`, `break`).
        return Ok(Step::DoneWith {
            message: format!(
                "Continued the {what}; git stopped again without conflicts. It is still in \
                 progress: continue it, or abort it."
            ),
            detail: Some(detail),
            hints: Vec::new(),
        });
    }
    let all = format!("{}\n{}", out.stderr, out.text());
    let message = if comes_out_empty(&all) {
        format!(
            "With the conflicts resolved this way the commit being applied has no changes \
             left (they are already here), and git will not continue with an empty commit: \
             abort the {what}, or skip this commit in a terminal (git says how below)."
        )
    } else {
        recognised(args, &all).unwrap_or_else(|| stopped_without_conflicts(still))
    };
    Ok(Step::Failed {
        message,
        detail,
        hints: Vec::new(),
    })
}

/// Whether git's output `all` says the commit being continued came out empty.
fn comes_out_empty(all: &str) -> bool {
    all.contains("is now empty")
        || all.contains("No changes - did you forget to use 'git add'?")
        || all.contains("nothing to commit")
}

/// Aborts what git stopped in: `<kind> --abort`, which puts HEAD, the index and the working
/// tree back as they were before it started.
pub(crate) async fn abort(ctx: &OpCtx<'_>) -> Result<Step> {
    let Some(kind) = read_kind(&ctx.runner, &ctx.cwd).await? else {
        return Err(nothing_in_progress());
    };
    let what = kind_name(kind);
    let args = [what, "--abort"];
    let out = ctx.git(&args).await?;
    if out.code != 0 {
        return Ok(explain(&args, &out));
    }
    let branch = current_branch(ctx).await?;
    let back = branch.as_deref().unwrap_or("HEAD");
    Ok(Step::Done(format!(
        "Aborted the {what}: {back} is back as it was before it."
    )))
}

impl RepoManager {
    /// Opens `path` (relative to the active worktree; the worktree folder itself when
    /// `None`) with `command` (the editor command from the settings: a program and its
    /// arguments, split on whitespace with simple quotes, the path added as the last
    /// argument), or with the platform's opener when `command` is `None`. The program is
    /// started directly, never through a shell, and not waited for.
    ///
    /// `bad_request` for a path that does not stay inside the worktree (`..`, an absolute
    /// path, a symlink that leads out) or a command with an unclosed quote; `path_missing`
    /// for a file that does not exist; `io` if the program cannot be started.
    pub async fn open_in_editor(
        &self,
        repo_id: u32,
        path: Option<&str>,
        command: Option<&str>,
    ) -> Result<()> {
        let worktree = self.get(repo_id)?.worktree.clone();
        let target = inside_worktree(&worktree, path)?;
        let argv = editor_argv(command, &target)?;
        #[cfg(any(test, feature = "testutil"))]
        if let Some(launches) = crate::lock_ignoring_poison(&self.editor_launches).as_mut() {
            launches.push(
                argv.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
            );
            return Ok(());
        }
        launch(&argv, &worktree, self.login_path())
    }

    /// From now on, editor launches are recorded instead of run (tests only; see
    /// [`RepoManager::editor_launches`]).
    #[cfg(any(test, feature = "testutil"))]
    pub fn record_editor_launches(&self) {
        crate::lock_ignoring_poison(&self.editor_launches).get_or_insert_with(Vec::new);
    }

    /// The argument vectors of the editor launches recorded so far (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn editor_launches(&self) -> Vec<Vec<String>> {
        crate::lock_ignoring_poison(&self.editor_launches)
            .clone()
            .unwrap_or_default()
    }
}

/// `rel` (a path relative to `worktree`; `None` for the worktree itself), resolved, if it
/// stays inside `worktree`: no `..`, no absolute path, and no symlink leading out.
fn inside_worktree(worktree: &Path, rel: Option<&str>) -> Result<PathBuf> {
    let root = dunce::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    let Some(rel) = rel else {
        return Ok(root);
    };
    let outside = || {
        CoreError::new(
            ErrorCode::BadRequest,
            format!("\u{201c}{rel}\u{201d} is not a path inside the worktree."),
        )
    };
    let path = Path::new(rel);
    let plain = path
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if rel.is_empty() || !plain {
        return Err(outside());
    }
    let resolved = dunce::canonicalize(root.join(path))
        .map_err(|_| CoreError::new(ErrorCode::PathMissing, format!("File not found: {rel}")))?;
    if !resolved.starts_with(&root) {
        return Err(outside());
    }
    Ok(resolved)
}

/// The program and arguments that open `target`: `command` split into words, then the
/// path; or, without a command, the platform's opener.
fn editor_argv(command: Option<&str>, target: &Path) -> Result<Vec<OsString>> {
    let words = match command.map(split_command).transpose()? {
        Some(words) if !words.is_empty() => words.into_iter().map(OsString::from).collect(),
        _ => platform_opener(),
    };
    let mut argv: Vec<OsString> = words;
    argv.push(target.as_os_str().to_owned());
    Ok(argv)
}

/// The program (and leading arguments) that opens a file or folder with the system's
/// default application.
fn platform_opener() -> Vec<OsString> {
    let words: &[&str] = if cfg!(target_os = "macos") {
        &["open"]
    } else if cfg!(windows) {
        // `start`'s first quoted argument is a window title: an empty one.
        &["cmd", "/C", "start", ""]
    } else {
        &["xdg-open"]
    };
    words.iter().map(OsString::from).collect()
}

/// `command` split into words on whitespace; text in single or double quotes is one word
/// (or part of one), taken literally. Nothing else is special: no escapes, no variables.
fn split_command(command: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote = None;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            "The editor command has a quote that is not closed. Check it in Settings.",
        ));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// Starts `argv` in `cwd` with no input or output, in a process group of its own (so it
/// outlives a terminal's Ctrl-C), and does not wait for it; a thread reaps it when it ends.
/// `path` is the `PATH` to find the program with (the login shell's), if known.
fn launch(argv: &[OsString], cwd: &Path, path: Option<OsString>) -> Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Err(CoreError::new(ErrorCode::BadRequest, "No editor to run."));
    };
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let name = program.to_string_lossy();
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CoreError::new(
            ErrorCode::Io,
            format!(
                "The program \u{201c}{name}\u{201d} was not found, so nothing was opened. Set \
                 the editor command in Settings."
            ),
        )
        .with_detail(e.to_string())),
        Err(e) => Err(CoreError::new(
            ErrorCode::Io,
            format!("Could not start \u{201c}{name}\u{201d} to open the file."),
        )
        .with_detail(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_on_whitespace_with_simple_quotes() {
        let split = |c: &str| split_command(c).unwrap();
        assert_eq!(split("code -w"), ["code", "-w"]);
        assert_eq!(split("  subl\t--wait  "), ["subl", "--wait"]);
        assert_eq!(
            split(r#""/Applications/My Editor.app/x" 'a "b"' c"d e"f"#),
            ["/Applications/My Editor.app/x", "a \"b\"", "cd ef"]
        );
        assert_eq!(split("x '' y"), ["x", "", "y"]);
        assert_eq!(split("a\\ b $HOME"), ["a\\", "b", "$HOME"]);
        assert!(split("").is_empty());
        assert_eq!(
            split_command("vim \"x").unwrap_err().code,
            ErrorCode::BadRequest
        );
    }

    #[test]
    fn a_blank_command_uses_the_platform_opener() {
        let argv = editor_argv(Some("   "), Path::new("/r/a.txt")).unwrap();
        let mut expected = platform_opener();
        expected.push("/r/a.txt".into());
        assert_eq!(argv, expected);
    }
}
