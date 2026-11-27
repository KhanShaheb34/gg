//! Plain explanations of git failures. A failed step carries git's own output as its
//! detail, and a notice: an explanation where the failure is one the core recognises (what
//! went wrong, and under it what to do), else "git merge failed" and git's most telling line.

use std::time::Duration;

use crate::dto::OpHint;
use crate::git::runner::{Expired, GitOutput, git_failed};
use crate::notice::{self, name, said, said_const, telling_line};
use crate::ops::Step;

/// Said when git could not take a lock because its lock file exists.
const LOCKED: &str = said_const!(
    "Another git process is working here",
    "git found its lock file (named \
                      in the log). Try again once it is done."
);

/// Said when HEAD is detached and the operation needs a current branch.
pub(crate) const NO_BRANCH: &str = said_const!(
    "No branch is checked out",
    "HEAD is detached, so there \
                                    is no current branch to do this on. Check out a branch \
                                    first."
);

/// Said when a push is rejected because the remote branch has commits the local one lacks.
pub(crate) const REMOTE_AHEAD: &str = said_const!(
    "Push rejected: the remote has new commits",
    "Pull to \
                                       bring them in first, or force push to discard them."
);

/// Said when a force push with lease is refused because the remote branch moved.
pub(crate) const REMOTE_MOVED: &str = said_const!(
    "Force push refused: the remote branch moved",
    "It \
                                       changed since your last fetch, and pushing would discard \
                                       commits you have not seen. Fetch, look at what is new, \
                                       then decide again."
);

/// Said when a remote cannot be reached at all.
const UNREACHABLE: &str = said_const!(
    "Could not reach the remote",
    "Check the network connection and \
                           the remote's address."
);

/// Said when git needs credentials it has no way to get.
const NEEDS_CREDENTIALS: &str = said_const!(
    "The remote asks for a username and password",
    "git has \
                                 none stored for it, and gg cannot type them in: set up a \
                                 credential helper or an SSH key, or run the command once in a \
                                 terminal to store them."
);

/// Said when the remote refuses the credentials git gave it.
const AUTH_FAILED: &str = said_const!(
    "The remote refused your credentials",
    "Update the username and \
                           password (or token) stored for it, then try again."
);

/// Said when an SSH server refuses every key offered.
const SSH_KEY_REFUSED: &str = said_const!(
    "The SSH server refused your key",
    "Check that it is loaded \
                               (ssh-add) and added to your account on the server."
);

/// Said when ssh does not know, or no longer trusts, the server's host key.
const HOST_KEY: &str = said_const!(
    "ssh does not trust the server's host key",
    "It is not known here, \
                        or it has changed. Connect once with ssh in a terminal to check and \
                        accept it."
);

/// Most file names an explanation lists; the rest are counted.
const MAX_FILES_LISTED: usize = 10;

/// The step for `out`, which git (run with `args`) ended with a non-zero exit code: done if
/// git found nothing to do (see [`nothing_to_do`]), else failed, with an explanation where
/// the failure is one the core recognises.
pub fn explain(args: &[&str], out: &GitOutput) -> Step {
    if let Some(message) = nothing_to_do(args, out) {
        return Step::Done(message);
    }
    let detail = if out.stderr.trim().is_empty() {
        out.text()
    } else {
        out.stderr.clone()
    };
    let all = format!("{}\n{}", out.stderr, out.text());
    // The branch is checked out in another worktree: offer to make that one
    // active.
    let hints: Vec<OpHint> = checked_out_elsewhere(&all)
        .map(|(_, worktree)| OpHint::MakeActive {
            worktree: crate::platform::native_path(worktree),
        })
        .into_iter()
        .collect();
    let message =
        known_failure(args, &all).unwrap_or_else(|| failed_in_gits_words(args, out.code, &detail));
    Step::Failed {
        message,
        detail,
        hints,
    }
}

/// The notice of a failure the core does not recognise, from git (run with `args`) ending with
/// `code` and `output`: "git merge failed", and git's most telling line under it (its whole
/// output is in the log).
pub(crate) fn failed_in_gits_words(args: &[&str], code: i32, output: &str) -> String {
    let what = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .copied()
        .unwrap_or("");
    let line = telling_line(output).unwrap_or_else(|| git_failed(args, code, output).message);
    said(format!("git {what} failed"), line)
}

/// A plain line for `out` if git (run with `args`) found nothing to do: already up to date,
/// or nothing to commit. Such a run is done, not failed, whatever its exit code.
pub(crate) fn nothing_to_do(args: &[&str], out: &GitOutput) -> Option<String> {
    let all = format!("{}\n{}", out.stderr, out.text());
    let up_to_date = all.lines().any(|l| {
        let l = l.trim();
        l.starts_with("Already up to date")
            || l.starts_with("Already up-to-date")
            || (l.starts_with("Current branch ") && l.ends_with(" is up to date."))
    });
    let nothing_to_commit = all.lines().any(|l| {
        let l = l.trim();
        l.starts_with("nothing to commit") || l.starts_with("nothing added to commit")
    });
    let what = args.first().copied().unwrap_or("");
    if up_to_date {
        return Some(match what {
            "rebase" => said("Already up to date", "Nothing to rebase."),
            _ => said("Already up to date", "Nothing to merge."),
        });
    }
    if nothing_to_commit {
        return Some(match what {
            "cherry-pick" => said(
                "Nothing to commit",
                "The commit's changes are already here.",
            ),
            "revert" => said(
                "Nothing to commit",
                "The commit's changes are already undone here.",
            ),
            _ => "Nothing to commit".to_owned(),
        });
    }
    None
}

/// The explanation of a failure the core recognises in git's output `all` (stderr, then
/// stdout), if it recognises it.
pub(crate) fn recognised(args: &[&str], all: &str) -> Option<String> {
    known_failure(args, all)
}

/// The explanation of a failure the core recognises in git's output `all` (stderr, then
/// stdout).
fn known_failure(args: &[&str], all: &str) -> Option<String> {
    if lock_file_exists(all) {
        return Some(LOCKED.to_owned());
    }
    if talks_to_a_remote(args)
        && let Some(network) = network_failure(args, all)
    {
        return Some(network);
    }
    if let Some((what, files)) = files_in_the_way(
        all,
        "Your local changes to the following files would be overwritten by ",
    ) {
        return Some(said(
            "Your uncommitted changes are in the way",
            format!("The {what} would overwrite {files}. Commit or stash them first."),
        ));
    }
    if let Some((what, files)) = files_in_the_way(
        all,
        "The following untracked working tree files would be overwritten by ",
    ) {
        return Some(said(
            "Untracked files are in the way",
            format!("The {what} would overwrite {files}. Move or delete them first."),
        ));
    }
    for line in all.lines().map(str::trim) {
        if let Some((branch, path)) = held_by_worktree(line) {
            let at = notice::path(crate::platform::native_path(path));
            return Some(said(
                format!("{} is checked out in another worktree", name(branch)),
                format!("At {at}. Switch to that worktree, or check out another branch here."),
            ));
        }
        if let Some(n) = between(line, "fatal: a branch named '", "' already exists") {
            return Some(format!("A branch named {} already exists", name(n)));
        }
        if let Some(n) = between(line, "fatal: tag '", "' already exists") {
            return Some(format!("A tag named {} already exists", name(n)));
        }
        if let Some((branch, path)) = delete_refused_by_worktree(line) {
            let at = notice::path(crate::platform::native_path(path));
            return Some(said(
                format!("{} is checked out in another worktree", name(branch)),
                format!(
                    "At {at}, so it cannot be deleted. Check out another branch there, or \
                     delete that worktree, first."
                ),
            ));
        }
        if let Some(n) = between(line, "error: the branch '", "' is not fully merged") {
            return Some(said(
                format!("{} is not fully merged", name(n)),
                "It has commits that are not in its upstream branch (or, without one, in \
                 HEAD), so git keeps it.",
            ));
        }
        if line.ends_with("is a merge but no -m option was given.") {
            let what = match args.first() {
                Some(&"revert") => "revert",
                _ => "cherry-pick",
            };
            return Some(said(
                "That commit is a merge",
                format!(
                    "To {what} it, choose which of its parents is the mainline (usually the \
                     first, the branch it was merged into)."
                ),
            ));
        }
        if line == "You are not currently on a branch." || line == "fatal: No current branch." {
            return Some(NO_BRANCH.to_owned());
        }
    }
    let conflicts: Vec<&str> = all
        .lines()
        .filter(|l| l.starts_with("CONFLICT ("))
        .map(|l| {
            l.split_once("Merge conflict in ")
                .map_or("", |(_, f)| f.trim())
        })
        .collect();
    if !conflicts.is_empty() {
        let named: Vec<&str> = conflicts
            .iter()
            .copied()
            .filter(|f| !f.is_empty())
            .collect();
        let files = if named.len() == conflicts.len() {
            file_list(&named)
        } else {
            format!("{} files", conflicts.len())
        };
        return Some(said(
            format!("Applied with conflicts in {files}"),
            "No commit was made, so there is nothing to continue: resolve the conflicts and \
             commit, or discard the changes.",
        ));
    }
    None
}

/// How a push or fetch was rejected, from git's ` ! [rejected]  a -> b (reason)` lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejected {
    /// The remote branch has commits the pushed one lacks ("fetch first",
    /// "non-fast-forward" on a push).
    RemoteAhead,
    /// `--force-with-lease` found the remote branch moved ("stale info").
    StaleInfo,
    /// The remote has a tag of that name elsewhere ("already exists").
    TagExists,
}

/// The first rejection in a push's output `all`, if any.
pub(crate) fn push_rejected(all: &str) -> Option<Rejected> {
    all.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("! [rejected]") || l.starts_with("! [remote rejected]"))
        .find_map(|l| {
            if l.ends_with("(fetch first)") || l.ends_with("(non-fast-forward)") {
                Some(Rejected::RemoteAhead)
            } else if l.ends_with("(stale info)") {
                Some(Rejected::StaleInfo)
            } else if l.ends_with("(already exists)") {
                Some(Rejected::TagExists)
            } else {
                None
            }
        })
}

/// Whether a pull failed because the branches diverged and no way to reconcile them is
/// configured (`pull.rebase`, `pull.ff`), or only fast-forwards are allowed (`pull.ff=only`,
/// `merge.ff=only`, `--ff-only`): either way merging or rebasing is the user's choice.
pub(crate) fn needs_pull_mode(all: &str) -> bool {
    all.contains("Need to specify how to reconcile divergent branches") || fast_forward_only(all)
}

/// Whether git refused because only a fast-forward was allowed and none was possible.
fn fast_forward_only(all: &str) -> bool {
    all.contains("Not possible to fast-forward")
}

/// Whether git run with `args` talks to a remote. Only then do words like "Connection
/// refused" in its output mean the remote: in a local command they come from a hook.
fn talks_to_a_remote(args: &[&str]) -> bool {
    matches!(
        args.first(),
        Some(&("push" | "fetch" | "pull" | "ls-remote"))
    )
}

/// The explanation of a failure to reach, or be let into, a remote, or of a push or fetch
/// the remote or git rejected.
fn network_failure(args: &[&str], all: &str) -> Option<String> {
    let has = |needle: &str| all.contains(needle);
    if let Some(line) = all
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("Timed out: "))
    {
        let why = line.trim_start_matches("Timed out: ").trim_end_matches('.');
        return Some(said(
            "The remote took too long to answer",
            format!("The command was stopped ({why})."),
        ));
    }
    if has("could not read Username for") || has("could not read Password for") {
        return Some(NEEDS_CREDENTIALS.to_owned());
    }
    if has("Authentication failed for") {
        return Some(AUTH_FAILED.to_owned());
    }
    if has("Permission denied (publickey") {
        return Some(SSH_KEY_REFUSED.to_owned());
    }
    if has("Host key verification failed") {
        return Some(HOST_KEY.to_owned());
    }
    let unreachable = [
        "Could not resolve host",
        "Couldn't connect to server",
        "Connection refused",
        "Connection timed out",
        "Operation timed out",
        "Network is unreachable",
        "No route to host",
        "Failed to connect to",
    ];
    if unreachable.iter().any(|n| has(n)) {
        return Some(UNREACHABLE.to_owned());
    }
    for line in all.lines().map(str::trim) {
        if let Some(at) = between(line, "fatal: '", "' does not appear to be a git repository") {
            return Some(said(
                format!("No git repository at {}", name(at)),
                "There is no remote of that name, or the remote's address is wrong, or the \
                 repository is gone.",
            ));
        }
        if let Some(rest) = line.strip_prefix("fatal: refusing to fetch into branch 'refs/heads/")
            && let Some((branch, path)) = rest.split_once("' checked out at '")
        {
            let at = notice::path(crate::platform::native_path(
                path.strip_suffix('\'').unwrap_or(path),
            ));
            return Some(said(
                format!("{} is checked out in another worktree", name(branch)),
                format!(
                    "At {at}, and git does not move a checked-out branch by fetching into it: \
                     switch to that worktree and pull it there."
                ),
            ));
        }
        if line == "There is no tracking information for the current branch." {
            return Some(said(
                "The current branch has no upstream",
                "There is nothing to pull from: push it with an upstream first, or set one.",
            ));
        }
    }
    if args.first() == Some(&"pull") && fast_forward_only(all) {
        return Some(said(
            "The branch and its upstream have diverged",
            "git is set to pull by fast-forward only (pull.ff), which is not possible here: \
             choose whether to pull by merging or by rebasing.",
        ));
    }
    if needs_pull_mode(all) {
        return Some(said(
            "The branch and its upstream have diverged",
            "Choose whether to pull by merging or by rebasing.",
        ));
    }
    match args.first() {
        Some(&"push") => match push_rejected(all)? {
            Rejected::RemoteAhead => Some(REMOTE_AHEAD.to_owned()),
            Rejected::StaleInfo => Some(REMOTE_MOVED.to_owned()),
            Rejected::TagExists => Some(said(
                "The remote has that tag on another commit",
                "A push does not move tags: delete it on the remote first if it is to change.",
            )),
        },
        Some(&"fetch") => all
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("! [rejected]") && l.ends_with("(non-fast-forward)"))
            .find_map(|l| {
                let (_, dst) = l.split_once("->")?;
                let dst = dst.trim().trim_end_matches("(non-fast-forward)").trim();
                Some(said(
                    format!("{} has commits the remote branch does not have", name(dst)),
                    "It cannot simply be moved forward (not a fast-forward). Check it out and \
                     pull, to merge or rebase.",
                ))
            }),
        _ => None,
    }
}

/// What would overwrite files, and the files git lists (tab-indented lines) after the line
/// containing `lead` ("… would be overwritten by checkout:"), for a message.
fn files_in_the_way(all: &str, lead: &str) -> Option<(String, String)> {
    let mut lines = all.lines();
    let what = lines.by_ref().find_map(|l| {
        let (_, rest) = l.split_once(lead)?;
        Some(rest.trim().trim_end_matches(':').to_owned())
    })?;
    let files: Vec<&str> = lines
        .take_while(|l| l.starts_with('\t'))
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .collect();
    Some((what, file_list(&files)))
}

/// `files` for a sentence: "a, b and c", at most [`MAX_FILES_LISTED`] named.
pub(crate) fn file_list(files: &[&str]) -> String {
    match files {
        [] => "some files".to_owned(),
        [one] => (*one).to_owned(),
        _ if files.len() > MAX_FILES_LISTED => format!(
            "{} and {} more",
            files[..MAX_FILES_LISTED].join(", "),
            files.len() - MAX_FILES_LISTED
        ),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The branch and the worktree holding it, when git refused because a branch is checked
/// out in a worktree: a checkout or a new worktree ([`held_by_worktree`]), a delete
/// ([`delete_refused_by_worktree`]), a fetch into it.
pub(crate) fn checked_out_elsewhere(all: &str) -> Option<(&str, &str)> {
    all.lines().map(str::trim).find_map(|line| {
        held_by_worktree(line)
            .or_else(|| delete_refused_by_worktree(line))
            .or_else(|| {
                let rest =
                    line.strip_prefix("fatal: refusing to fetch into branch 'refs/heads/")?;
                let (branch, path) = rest.split_once("' checked out at '")?;
                Some((branch, path.strip_suffix('\'').unwrap_or(path)))
            })
    })
}

/// The branch and the worktree path in git's "error: cannot delete branch 'x' used by
/// worktree at '/p'" (older git: "error: Cannot delete branch 'x' checked out at '/p'").
fn delete_refused_by_worktree(line: &str) -> Option<(&str, &str)> {
    let rest = line
        .strip_prefix("error: cannot delete branch '")
        .or_else(|| line.strip_prefix("error: Cannot delete branch '"))?;
    for middle in ["' used by worktree at '", "' checked out at '"] {
        if let Some((branch, path)) = rest.split_once(middle) {
            return Some((branch, path.strip_suffix('\'').unwrap_or(path)));
        }
    }
    None
}

/// The branch and the worktree path in git's "'x' is already used by worktree at '/p'"
/// (older git: "'x' is already checked out at '/p'").
fn held_by_worktree(line: &str) -> Option<(&str, &str)> {
    let line = line.strip_prefix("fatal: ").unwrap_or(line);
    let line = line.strip_prefix('\'')?;
    for middle in [
        "' is already used by worktree at '",
        "' is already checked out at '",
    ] {
        if let Some((branch, rest)) = line.split_once(middle) {
            return Some((branch, rest.strip_suffix('\'').unwrap_or(rest)));
        }
    }
    None
}

/// The text of `line` between `start` (at its start) and `end`.
fn between<'l>(line: &'l str, start: &str, end: &str) -> Option<&'l str> {
    line.strip_prefix(start)?.split_once(end).map(|(x, _)| x)
}

/// Whether git failed to create a lock file (`index.lock`, a ref's `.lock`) because it
/// exists: "Unable to create '…/index.lock': File exists."
fn lock_file_exists(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|l| l.contains("Unable to create '") && l.contains(".lock': File exists"))
}

/// The line added to the output of a network command that was given up on.
pub(crate) fn timed_out(limit: Expired) -> String {
    match limit {
        Expired::Idle(idle) => format!("Timed out: no output from git for {}.\n", span(idle)),
        Expired::Total(total) => format!("Timed out: still running after {}.\n", span(total)),
        Expired::Cancelled => "Stopped: it was cancelled.\n".to_owned(),
    }
}

/// `d` for people: whole hours, minutes or seconds where it is one, else milliseconds.
fn span(d: Duration) -> String {
    let (ms, s) = (d.as_millis(), d.as_secs());
    if ms % 1000 != 0 {
        format!("{ms} ms")
    } else if s >= 3600 && s % 3600 == 0 {
        format!("{} h", s / 3600)
    } else if s >= 60 && s % 60 == 0 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(stderr: &str, stdout: &str) -> GitOutput {
        GitOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.to_owned(),
            code: 128,
        }
    }

    #[test]
    fn unknown_failures_quote_gits_telling_line() {
        let step = explain(
            &["branch", "--", "x", "nope"],
            &failed("\nfatal: not a valid object name: 'nope'\nmore\n", ""),
        );
        assert_eq!(
            step,
            Step::Failed {
                message: said("git branch failed", "Not a valid object name: 'nope'"),
                detail: "\nfatal: not a valid object name: 'nope'\nmore\n".into(),
                hints: vec![],
            }
        );
    }

    #[test]
    fn output_on_stdout_is_the_detail_when_stderr_is_empty() {
        let Step::Failed {
            message, detail, ..
        } = explain(&["commit"], &failed("", "something odd\n"))
        else {
            panic!("not failed");
        };
        assert_eq!(message, said("git commit failed", "something odd"));
        assert_eq!(detail, "something odd\n");
        let Step::Failed { message, .. } = explain(&["commit"], &failed("", "")) else {
            panic!("not failed");
        };
        assert_eq!(
            message,
            said("git commit failed", "git commit exited with code 128")
        );
    }

    #[test]
    fn network_wording_counts_only_for_commands_that_talk_to_a_remote() {
        let out = failed("hook: Connection refused\nCould not resolve host: ci\n", "");
        for args in [
            &["merge", "x"][..],
            &["commit"],
            &["rebase", "x"],
            &["tag", "v"],
        ] {
            let Step::Failed { message, .. } = explain(args, &out) else {
                panic!("not failed");
            };
            assert_ne!(message, UNREACHABLE, "{args:?}");
        }
        for args in [
            &["push", "o"][..],
            &["fetch"],
            &["pull"],
            &["ls-remote", "o"],
        ] {
            let Step::Failed { message, .. } = explain(args, &out) else {
                panic!("not failed");
            };
            assert_eq!(message, UNREACHABLE, "{args:?}");
        }
    }

    #[test]
    fn fetching_into_a_branch_checked_out_elsewhere_names_that_worktree() {
        let out = failed(
            "fatal: refusing to fetch into branch 'refs/heads/topic' checked out at '/w/topic'\n",
            "",
        );
        let Step::Failed { message, .. } = explain(&["fetch", "origin", "x:y"], &out) else {
            panic!("not failed");
        };
        let at = crate::platform::native_path("/w/topic");
        assert!(message.contains(&at), "{message}");
        assert!(message.contains("that worktree"), "{message}");
        assert!(!message.contains("pull it instead"), "{message}");
    }

    /// Every refusal over a branch checked out in another worktree says which one, and offers
    /// to make it active.
    #[test]
    fn a_branch_checked_out_elsewhere_offers_its_worktree() {
        let cases: [(&[&str], &str); 5] = [
            (
                &["checkout", "--no-guess", "f", "--"],
                "fatal: 'f' is already used by worktree at '/w/f'\n",
            ),
            (
                &["worktree", "add", "--", "/w/g", "f"],
                "Preparing worktree (checking out 'f')\nfatal: 'f' is already used by worktree at '/w/f'\n",
            ),
            (
                &["branch", "-d", "--", "f"],
                "error: cannot delete branch 'f' used by worktree at '/w/f'\n",
            ),
            (
                &["branch", "-D", "--", "f"],
                "error: Cannot delete branch 'f' checked out at '/w/f'\n",
            ),
            (
                &["fetch", "origin", "x:f"],
                "fatal: refusing to fetch into branch 'refs/heads/f' checked out at '/w/f'\n",
            ),
        ];
        // Spelled as the platform spells paths (git for Windows writes `/`).
        let at = crate::platform::native_path("/w/f");
        for (args, stderr) in cases {
            let Step::Failed { message, hints, .. } = explain(args, &failed(stderr, "")) else {
                panic!("not failed");
            };
            assert!(message.contains(&at), "{args:?}: {message}");
            assert!(message.contains(&name("f")), "{args:?}: {message}");
            // Worktrees are deleted, in the app's words, never "removed".
            assert!(
                !message.to_lowercase().contains("remove"),
                "{args:?}: {message}"
            );
            assert_eq!(
                hints,
                [OpHint::MakeActive {
                    worktree: at.clone()
                }],
                "{args:?}"
            );
        }
        let Step::Failed { hints, .. } = explain(&["merge"], &failed("fatal: other\n", "")) else {
            panic!("not failed");
        };
        assert!(hints.is_empty());
    }

    #[test]
    fn a_held_ref_lock_is_explained_too() {
        let stderr = "error: cannot lock ref 'refs/heads/x': Unable to create \
                      '/r/.git/refs/heads/x.lock': File exists.\n";
        let Step::Failed { message, .. } = explain(&["branch"], &failed(stderr, "")) else {
            panic!("not failed");
        };
        assert_eq!(message, LOCKED);
        // The app never suggests deleting git's lock files: the process holding one may be
        // running.
        assert!(!LOCKED.to_lowercase().contains("delete"), "{LOCKED}");
        assert!(LOCKED.contains("Try again"), "{LOCKED}");
    }

    #[test]
    fn file_lists_read_as_a_sentence() {
        assert_eq!(file_list(&["a"]), "a");
        assert_eq!(file_list(&["a", "b", "c"]), "a, b and c");
        let many: Vec<String> = (0..12).map(|i| format!("f{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(
            file_list(&many),
            "f0, f1, f2, f3, f4, f5, f6, f7, f8, f9 and 2 more"
        );
    }

    fn message(args: &[&str], stderr: &str) -> String {
        match explain(args, &failed(stderr, "")) {
            Step::Failed { message, .. } => message,
            other => panic!("not failed: {other:?}"),
        }
    }

    /// git's wording for failures that cannot be reproduced without a real server: an SSH
    /// server refusing the key, an unknown host key, a host name that does not resolve and
    /// a connection that times out (from git 2.54 and OpenSSH, as printed on macOS).
    #[test]
    fn ssh_and_dns_failures_are_explained() {
        let push = [
            "push",
            "--progress",
            "origin",
            "refs/heads/main:refs/heads/main",
        ];
        let publickey = "git@github.com: Permission denied (publickey).\r\n\
                         fatal: Could not read from remote repository.\n\n\
                         Please make sure you have the correct access rights\n\
                         and the repository exists.\n";
        assert_eq!(message(&push, publickey), SSH_KEY_REFUSED);
        let host_key = "No ED25519 host key is known for example.com and you have requested \
                        strict checking.\r\nHost key verification failed.\r\n\
                        fatal: Could not read from remote repository.\n";
        assert_eq!(message(&push, host_key), HOST_KEY);
        let ssh_dns = "ssh: Could not resolve hostname nowhere.invalid: nodename nor servname \
                       provided, or not known\r\nfatal: Could not read from remote \
                       repository.\n";
        assert_eq!(message(&push, ssh_dns), UNREACHABLE);
        let https_dns = "fatal: unable to access 'https://nowhere.invalid/r.git/': Could not \
                         resolve host: nowhere.invalid\n";
        assert_eq!(message(&push, https_dns), UNREACHABLE);
        let ssh_timeout = "ssh: connect to host example.com port 22: Operation timed out\r\n\
                           fatal: Could not read from remote repository.\n";
        assert_eq!(message(&push, ssh_timeout), UNREACHABLE);
        let https_timeout = "fatal: unable to access 'https://example.com/r.git/': Failed to \
                             connect to example.com port 443 after 75004 ms: Couldn't connect \
                             to server\n";
        assert_eq!(message(&push, https_timeout), UNREACHABLE);
    }

    #[test]
    fn a_timed_out_network_command_is_explained() {
        let out = format!(
            "Writing objects:  10% (1/10)\n{}",
            timed_out(Expired::Idle(Duration::from_secs(120)))
        );
        assert_eq!(
            message(&["push"], &out),
            said(
                "The remote took too long to answer",
                "The command was stopped (no output from git for 2 min)."
            )
        );
    }

    #[test]
    fn push_rejections_are_told_apart() {
        let line = |reason: &str| format!("To o\n ! [rejected]        main -> main ({reason})\n");
        assert_eq!(
            push_rejected(&line("fetch first")),
            Some(Rejected::RemoteAhead)
        );
        assert_eq!(
            push_rejected(&line("non-fast-forward")),
            Some(Rejected::RemoteAhead)
        );
        assert_eq!(
            push_rejected(&line("stale info")),
            Some(Rejected::StaleInfo)
        );
        assert_eq!(
            push_rejected(&line("already exists")),
            Some(Rejected::TagExists)
        );
        assert_eq!(push_rejected(&line("something new")), None);
        assert_eq!(push_rejected("Everything up-to-date\n"), None);
    }

    #[test]
    fn timeouts_say_which_limit() {
        assert_eq!(
            timed_out(Expired::Idle(Duration::from_secs(120))),
            "Timed out: no output from git for 2 min.\n"
        );
        assert_eq!(
            timed_out(Expired::Total(Duration::from_secs(7200))),
            "Timed out: still running after 2 h.\n"
        );
        assert_eq!(span(Duration::from_millis(1500)), "1500 ms");
        assert_eq!(span(Duration::from_secs(45)), "45 s");
    }
}
