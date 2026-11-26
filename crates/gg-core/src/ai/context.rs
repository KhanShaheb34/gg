//! What gg reads from the repository for a prompt, with git, as every other reader does:
//! for a commit message, what a commit of the ticked files would record (each ticked file
//! as the working tree has it, against HEAD, as `ops/commit.rs` commits it); for a pull
//! request, the branch's commits and changes since it left its base. Only reads: nothing is
//! staged, fetched or written.
//!
//! Nothing of the repository's configuration runs while gg reads: no external diff or text
//! conversion program (`--no-ext-diff`, `--no-textconv`), and no clean or smudge filter (each
//! filter driver configured, in any scope, is turned off for these reads: [`no_filters`]),
//! which git would otherwise run to read a changed file of the working tree. A binary file is
//! named, never sent: any part of a patch that holds a NUL byte is replaced by a line saying
//! so, whatever the repository's attributes say ([`withhold_binary`]; `.gitattributes` can
//! make git print a binary file as text). Cancel ends git at once, also while it is still
//! working out a patch.

use std::io;
use std::path::Path;
use std::time::Instant;

use tokio::io::AsyncReadExt;

use super::prompt::{CommitContext, PrContext};
use crate::dto::{EntryStatus, FileEntry};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::{CmdKind, GitRunner};
use crate::ops::commit::path_chunks;

/// Most bytes of a patch read from git (a prompt takes far less: see
/// [`super::prompt::PATCH_BUDGET`]); git is stopped past it.
pub(crate) const READ_CAP: usize = 8 * 1024 * 1024;
/// Most bytes of a new (untracked) file read for its part of the patch.
const NEW_FILE_CAP: usize = 256 * 1024;
/// Most commits listed for a pull request (Orca's 50).
const MAX_COMMITS: usize = 50;

/// The options every patch is read with: plain text, no programs of the repository's
/// configuration (external diff, text conversion), renames found.
const DIFF_OPTIONS: [&str; 5] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--minimal",
    "-M",
];

/// Said in place of a binary file's part of a patch.
const BINARY_WITHHELD: &str = "Binary file: its contents are not sent.\n";

/// Why there is nothing to describe, for the user.
pub(crate) struct Nothing(pub String);

/// `-c` options that turn off every filter driver configured for the repository at `cwd`, in
/// any scope (`filter.<name>.clean`, `.smudge`, `.process` empty, `.required` false), so that
/// a read of the working tree runs none of them.
pub(crate) async fn no_filters(runner: &GitRunner, cwd: &Path) -> Result<Vec<String>> {
    let out = runner
        .run_unchecked(
            cwd,
            &["config", "--name-only", "--get-regexp", r"^filter\."],
            CmdKind::Read,
        )
        .await?;
    let mut names: Vec<String> = Vec::new();
    for key in out.text().lines() {
        let Some(rest) = key.trim().strip_prefix("filter.") else {
            continue;
        };
        let Some((name, _)) = rest.rsplit_once('.') else {
            continue;
        };
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    let mut args = Vec::new();
    for name in names {
        for (key, value) in [
            ("clean", ""),
            ("smudge", ""),
            ("process", ""),
            ("required", "false"),
        ] {
            args.push("-c".to_owned());
            args.push(format!("filter.{name}.{key}={value}"));
        }
    }
    Ok(args)
}

/// The changed files of the worktree at `worktree` that `paths` tick (a rename by either of
/// its names; a conflicted file never), read with `git status` with no filter running.
pub(crate) async fn ticked_entries(
    runner: &GitRunner,
    worktree: &Path,
    paths: &[String],
    filters: &[String],
) -> Result<Vec<FileEntry>> {
    let mut args: Vec<&str> = filters.iter().map(String::as_str).collect();
    args.extend(crate::git::status::STATUS_ARGS);
    let out = runner.run(worktree, &args, CmdKind::Read).await?;
    let ticked: std::collections::BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    Ok(crate::git::status::parse_files(&out.stdout)
        .into_iter()
        .filter(|e| e.status != EntryStatus::U)
        .filter(|e| {
            ticked.contains(e.path.as_str())
                || (e.status == EntryStatus::R
                    && e.orig_path.as_deref().is_some_and(|o| ticked.contains(o)))
        })
        .collect())
}

/// `patch` with each file's part that holds a NUL byte (a binary file, even one the
/// repository's attributes make git print as text) replaced by its header and a line saying
/// its contents are not sent.
pub(crate) fn withhold_binary(patch: &[u8]) -> Vec<u8> {
    const BOUNDARY: &[u8] = b"\ndiff --git ";
    let mut out = Vec::with_capacity(patch.len());
    let mut start = 0;
    while start < patch.len() {
        let end = patch[start..]
            .windows(BOUNDARY.len())
            .position(|w| w == BOUNDARY)
            .map_or(patch.len(), |at| start + at + 1);
        let section = &patch[start..end];
        if section.contains(&0) {
            let header = section
                .iter()
                .position(|&b| b == b'\n')
                .map_or(section, |nl| &section[..nl]);
            out.extend_from_slice(header);
            out.push(b'\n');
            out.extend_from_slice(BINARY_WITHHELD.as_bytes());
        } else {
            out.extend_from_slice(section);
        }
        start = end;
    }
    out
}

/// What a commit of `entries` (the ticked files of the worktree at `worktree`) would record;
/// `filters` turn off the repository's filter drivers ([`no_filters`]).
pub(crate) async fn commit_context(
    runner: &GitRunner,
    worktree: &Path,
    entries: &[&FileEntry],
    filters: &[String],
) -> Result<std::result::Result<CommitContext, Nothing>> {
    if entries.is_empty() {
        return Ok(Err(Nothing(
            "Nothing to describe: no file is ticked.".to_owned(),
        )));
    }
    let branch = runner
        .run_unchecked(
            worktree,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            CmdKind::Read,
        )
        .await?;
    let branch = (branch.code == 0)
        .then(|| branch.text().trim().to_owned())
        .filter(|b| !b.is_empty());
    let head = runner
        .run_unchecked(
            worktree,
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            CmdKind::Read,
        )
        .await?;
    let base = if head.code == 0 {
        head.text().trim().to_owned()
    } else {
        // An unborn branch: everything is new, against the empty tree (of this repository's
        // hash; nothing is written).
        let empty = runner
            .run(
                worktree,
                &["hash-object", "-t", "tree", "--stdin"],
                CmdKind::Read,
            )
            .await?;
        empty.text().trim().to_owned()
    };

    let mut files = Vec::new();
    let mut tracked: Vec<&str> = Vec::new();
    let mut untracked: Vec<&str> = Vec::new();
    for e in entries {
        let letter = match e.status {
            EntryStatus::Untracked => "A",
            EntryStatus::M => "M",
            EntryStatus::A => "A",
            EntryStatus::D => "D",
            EntryStatus::R => "R",
            EntryStatus::C => "C",
            EntryStatus::T => "T",
            EntryStatus::U => "U",
        };
        match (&e.orig_path, e.status) {
            (Some(orig), EntryStatus::R | EntryStatus::C) => {
                files.push(format!("{letter}\t{orig}\t{}", e.path));
            }
            _ => files.push(format!("{letter}\t{}", e.path)),
        }
        if e.status == EntryStatus::Untracked {
            untracked.push(&e.path);
        } else {
            tracked.push(&e.path);
            if e.status == EntryStatus::R
                && let Some(orig) = &e.orig_path
            {
                tracked.push(orig);
            }
        }
    }

    let mut patch: Vec<u8> = Vec::new();
    for chunk in path_chunks(&tracked) {
        if patch.len() >= READ_CAP {
            break;
        }
        let mut args: Vec<&str> = filters.iter().map(String::as_str).collect();
        args.extend(["--literal-pathspecs", "diff"]);
        args.extend(DIFF_OPTIONS);
        args.push(&base);
        args.push("--");
        args.extend_from_slice(chunk);
        let (out, _) = read_capped(runner, worktree, &args, READ_CAP - patch.len()).await?;
        patch.extend_from_slice(&out);
    }
    let mut patch = String::from_utf8_lossy(&withhold_binary(&patch)).into_owned();
    for path in untracked {
        if patch.len() >= READ_CAP {
            break;
        }
        patch.push_str(&new_file_diff(worktree, path));
    }
    if patch.trim().is_empty() {
        return Ok(Err(Nothing(
            "Nothing to describe: the ticked files have no changes now.".to_owned(),
        )));
    }
    Ok(Ok(CommitContext {
        branch,
        files: files.join("\n"),
        patch,
    }))
}

/// The patch part of a new file of the worktree at `worktree`, written as `git diff` writes
/// one: its lines added, or "Binary files … differ", a link's target; read up to
/// [`NEW_FILE_CAP`] bytes. Not read below a folder that is a link (git does not look there).
fn new_file_diff(worktree: &Path, path: &str) -> String {
    let header = format!("diff --git a/{path} b/{path}\n");
    let full = worktree.join(path);
    let mut at = std::path::PathBuf::new();
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        at.push(part);
        if std::fs::symlink_metadata(worktree.join(&at)).is_ok_and(|m| m.file_type().is_symlink()) {
            return format!("{header}new file\n(not read: it is below a symbolic link)\n");
        }
    }
    let Ok(meta) = std::fs::symlink_metadata(&full) else {
        return String::new();
    };
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full)
            .map(|t| t.to_string_lossy().into_owned())
            .unwrap_or_default();
        return format!(
            "{header}new file mode 120000\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n+{target}\n\\ No newline at end of file\n"
        );
    }
    if !meta.is_file() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&full).and_then(|f| {
        use std::io::Read;
        f.take(NEW_FILE_CAP as u64 + 1).read_to_end(&mut bytes)
    });
    if read.is_err() {
        return format!("{header}new file\n(could not be read)\n");
    }
    // A NUL anywhere in what was read: binary (git looks at the first 8000 bytes only).
    if bytes.contains(&0) {
        return format!(
            "{header}new file mode 100644\nBinary files /dev/null and b/{path} differ\n"
        );
    }
    let cut = bytes.len() > NEW_FILE_CAP;
    bytes.truncate(NEW_FILE_CAP);
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = format!(
        "{header}new file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in &lines {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if cut {
        out.push_str(&format!(
            "...(file truncated: only its first {NEW_FILE_CAP} bytes were read)\n"
        ));
    }
    out
}

/// What a pull request of local branch `branch` into `base` (on remote `base_remote`, else
/// the local branch of that name) would hold.
pub(crate) async fn pr_context(
    runner: &GitRunner,
    root: &Path,
    branch: &str,
    base: &str,
    base_remote: Option<&str>,
) -> Result<std::result::Result<PrContext, Nothing>> {
    let bad = |m: String| CoreError::new(ErrorCode::BadRequest, m);
    crate::git::names::check_branch_name(branch).map_err(|e| bad(e.message))?;
    crate::git::names::check_branch_name(base).map_err(|e| bad(e.message))?;
    let exists = |full: String| async move {
        runner
            .run_unchecked(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{full}^{{commit}}"),
                ],
                CmdKind::Read,
            )
            .await
            .is_ok_and(|o| o.code == 0)
    };
    let head = format!("refs/heads/{branch}");
    if !exists(head.clone()).await {
        return Err(bad(format!(
            "There is no local branch named \u{201c}{branch}\u{201d}."
        )));
    }
    let remotes = runner.run(root, &["remote"], CmdKind::Read).await?;
    let remote = base_remote.filter(|r| remotes.text().lines().any(|l| l == *r));
    let mut base_ref = None;
    if let Some(r) = remote {
        let full = format!("refs/remotes/{r}/{base}");
        if exists(full.clone()).await {
            base_ref = Some(full);
        }
    }
    if base_ref.is_none() && exists(format!("refs/heads/{base}")).await {
        base_ref = Some(format!("refs/heads/{base}"));
    }
    let Some(base_ref) = base_ref else {
        return Err(bad(format!(
            "There is no branch \u{201c}{base}\u{201d} to compare {branch} with."
        )));
    };
    let mb = runner
        .run_unchecked(root, &["merge-base", &base_ref, &head], CmdKind::Read)
        .await?;
    let from = if mb.code == 0 {
        mb.text().trim().to_owned()
    } else {
        base_ref.clone()
    };
    let max = format!("--max-count={MAX_COMMITS}");
    let not_base = format!("^{base_ref}");
    let log = runner
        .run(
            root,
            &[
                "log",
                "--no-color",
                "--format=- %s",
                &max,
                &head,
                &not_base,
                "--",
            ],
            CmdKind::Read,
        )
        .await?;
    let commits = log.text().trim().to_owned();
    if commits.is_empty() {
        return Ok(Err(Nothing(format!(
            "Nothing to describe: {branch} has no commits that are not on {base}."
        ))));
    }
    let names = runner
        .run(
            root,
            &[
                "diff",
                "--no-color",
                "--name-status",
                "-M",
                &from,
                &head,
                "--",
            ],
            CmdKind::Read,
        )
        .await?;
    let mut args = vec!["diff"];
    args.extend(DIFF_OPTIONS);
    args.extend([from.as_str(), head.as_str(), "--"]);
    let (patch, _) = read_capped(runner, root, &args, READ_CAP).await?;
    Ok(Ok(PrContext {
        branch: branch.to_owned(),
        base: base.to_owned(),
        commits,
        files: names.text().trim().to_owned(),
        patch: String::from_utf8_lossy(&withhold_binary(&patch)).into_owned(),
        ..PrContext::default()
    }))
}

/// Runs git `args` in `cwd` and reads at most `cap` bytes of its output (git is stopped
/// then); also whether there was more. Logged as a read, without its output.
async fn read_capped(
    runner: &GitRunner,
    cwd: &Path,
    args: &[&str],
    cap: usize,
) -> Result<(Vec<u8>, bool)> {
    let started = Instant::now();
    let _live = runner.live_child();
    let mut child = runner.spawn_stream(cwd, args)?;
    let (Some(mut stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(CoreError::new(ErrorCode::Io, "git output was not piped"));
    };
    // stdout and stderr are read together (git blocks on a full pipe); past the cap, or on a
    // read error, stdout is closed (git ends on its next write), so that stderr ends too.
    let read_out = async move {
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = stdout.read(&mut buf).await?;
            if n == 0 {
                return io::Result::Ok((out, false));
            }
            let room = cap - out.len();
            if n > room {
                out.extend_from_slice(&buf[..room]);
                return Ok((out, true));
            }
            out.extend_from_slice(&buf[..n]);
        }
    };
    let read_err = async move {
        let mut err = Vec::new();
        let _ = stderr.read_to_end(&mut err).await;
        err
    };
    let both = async { tokio::join!(read_out, read_err) };
    tokio::pin!(both);
    // Cancel ends git with all it started, at once, even before it has written anything.
    let ended = tokio::select! {
        done = &mut both => Some(done),
        () = runner.stopped() => None,
    };
    let Some((read, err)) = ended else {
        child.kill_tree();
        let status = child.wait().await;
        let code = status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);
        runner.log_stream_done(cwd, args, started, code, crate::git::runner::STOPPED);
        return Err(CoreError::new(ErrorCode::Io, crate::git::runner::STOPPED));
    };
    if !matches!(read, Ok((_, false))) {
        // Past the cap, or stdout failed: git is not waited out.
        child.kill_tree();
    }
    let status = child.wait().await;
    let code = status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&err).into_owned();
    runner.log_stream_done(cwd, args, started, code, &stderr);
    let (out, more) = read.map_err(|e| {
        CoreError::new(ErrorCode::Io, "could not read git output").with_detail(e.to_string())
    })?;
    if !more && code != 0 {
        return Err(crate::git::runner::git_failed(args, code, &stderr));
    }
    Ok((out, more))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file's part that holds a NUL byte is withheld, whatever made git print it as text
    /// (a `diff` attribute); the other parts are kept as they are.
    #[test]
    fn binary_parts_of_a_patch_are_withheld() {
        let patch = b"diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-one\n+two\ndiff --git a/key.bin b/key.bin\n--- a/key.bin\n+++ b/key.bin\n@@ -1 +1 @@\n-SECRET\0old\n+SECRET\0new\ndiff --git a/z.txt b/z.txt\n+last\n";
        let out = String::from_utf8(withhold_binary(patch)).unwrap();
        assert!(!out.contains("SECRET"), "{out}");
        assert!(!out.contains('\0'));
        assert!(
            out.contains(
                "diff --git a/key.bin b/key.bin\nBinary file: its contents are not sent.\n"
            )
        );
        assert!(out.starts_with("diff --git a/a.txt b/a.txt\n--- a/a.txt"));
        assert!(out.ends_with("diff --git a/z.txt b/z.txt\n+last\n"));
        assert_eq!(withhold_binary(b"no diff at all\n"), b"no diff at all\n");
        assert_eq!(withhold_binary(b""), b"");
    }

    /// Cancel ends a streamed git at once, also one that has written nothing yet (here,
    /// blocked reading a FIFO no one writes to).
    #[cfg(unix)]
    #[tokio::test]
    async fn cancel_ends_a_streamed_git_that_has_written_nothing() {
        use crate::git::runner::{CommandLog, STOPPED, Stop};
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("fifo");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        let stop = Stop::new();
        let runner = GitRunner::detect(CommandLog::new())
            .await
            .unwrap()
            .for_op(1, stop.clone());
        let fifo_arg = fifo.to_string_lossy().into_owned();
        let args = ["diff", "--no-index", "--", "/dev/null", fifo_arg.as_str()];
        let read = read_capped(&runner, dir.path(), &args, READ_CAP);
        let stopper = async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            stop.stop();
        };
        let started = std::time::Instant::now();
        let (got, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(read, stopper)
        })
        .await
        .expect("cancel ends it");
        assert_eq!(got.unwrap_err().message, STOPPED);
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
