//! Plain explanations of git failures. A failed step carries git's own output as its
//! detail, and a one-line message above it: an explanation where the failure is one the
//! core recognises, else git's first line.

use std::time::Duration;

use crate::dto::OpHint;
use crate::git::runner::{Expired, GitOutput, git_failed};
use crate::ops::Step;

/// Said when git could not take a lock because its lock file exists.
const LOCKED: &str = "Another git process is running in this repository, or one stopped \
                      without cleaning up: git found its lock file. Try again once it is \
                      done; if no git is running, delete the lock file named below.";

/// Said when HEAD is detached and the operation needs a current branch.
pub(crate) const NO_BRANCH: &str = "No branch is checked out (HEAD is detached), so there \
                                    is no current branch to do this on. Check out a branch \
                                    first.";

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
    let hints: Vec<OpHint> = Vec::new();
    let all = format!("{}\n{}", out.stderr, out.text());
    let message =
        known_failure(args, &all).unwrap_or_else(|| git_failed(args, out.code, &detail).message);
    Step::Failed {
        message,
        detail,
        hints,
    }
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
            "rebase" => "Already up to date: there is nothing to rebase.".to_owned(),
            _ => "Already up to date: there is nothing to merge.".to_owned(),
        });
    }
    if nothing_to_commit {
        return Some(match what {
            "cherry-pick" => "Nothing to commit: the commit's changes are already here.".to_owned(),
            "revert" => {
                "Nothing to commit: the commit's changes are already undone here.".to_owned()
            }
            _ => "Nothing to commit.".to_owned(),
        });
    }
    None
}

/// The explanation of a failure the core recognises in git's output `all` (stderr, then
/// stdout).
fn known_failure(args: &[&str], all: &str) -> Option<String> {
    if lock_file_exists(all) {
        return Some(LOCKED.to_owned());
    }
    if let Some((what, files)) = files_in_the_way(
        all,
        "Your local changes to the following files would be overwritten by ",
    ) {
        return Some(format!(
            "Your uncommitted changes to {files} would be overwritten by {what}. Commit or \
             stash them first."
        ));
    }
    if let Some((what, files)) = files_in_the_way(
        all,
        "The following untracked working tree files would be overwritten by ",
    ) {
        return Some(format!(
            "The untracked files {files} would be overwritten by {what}. Move or delete \
             them first."
        ));
    }
    for line in all.lines().map(str::trim) {
        if let Some((branch, path)) = held_by_worktree(line) {
            return Some(format!(
                "Branch \u{201c}{branch}\u{201d} is checked out in another worktree, at \
                 {path}. Switch to that worktree, or check out another branch here."
            ));
        }
        if let Some(name) = between(line, "fatal: a branch named '", "' already exists") {
            return Some(format!(
                "A branch named \u{201c}{name}\u{201d} already exists."
            ));
        }
        if let Some(name) = between(line, "fatal: tag '", "' already exists") {
            return Some(format!(
                "A tag named \u{201c}{name}\u{201d} already exists."
            ));
        }
        if let Some(name) = between(line, "error: the branch '", "' is not fully merged") {
            return Some(format!(
                "Branch \u{201c}{name}\u{201d} is not fully merged: it has commits that are \
                 not in its upstream branch (or, without one, in HEAD), so git keeps it."
            ));
        }
        if line.ends_with("is a merge but no -m option was given.") {
            let what = match args.first() {
                Some(&"revert") => "revert",
                _ => "cherry-pick",
            };
            return Some(format!(
                "That commit is a merge: to {what} it, choose which of its parents is the \
                 mainline (usually the first, the branch it was merged into)."
            ));
        }
        if line == "You are not currently on a branch." || line == "fatal: No current branch." {
            return Some(NO_BRANCH.to_owned());
        }
    }
    None
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
fn file_list(files: &[&str]) -> String {
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
    fn unknown_failures_lead_with_gits_first_line() {
        let step = explain(
            &["branch", "--", "x", "nope"],
            &failed("\nfatal: not a valid object name: 'nope'\nmore\n", ""),
        );
        assert_eq!(
            step,
            Step::Failed {
                message: "fatal: not a valid object name: 'nope'".into(),
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
        assert_eq!(message, "something odd");
        assert_eq!(detail, "something odd\n");
        let Step::Failed { message, .. } = explain(&["commit"], &failed("", "")) else {
            panic!("not failed");
        };
        assert_eq!(message, "git commit exited with code 128");
    }

    #[test]
    fn a_held_ref_lock_is_explained_too() {
        let stderr = "error: cannot lock ref 'refs/heads/x': Unable to create \
                      '/r/.git/refs/heads/x.lock': File exists.\n";
        let Step::Failed { message, .. } = explain(&["branch"], &failed(stderr, "")) else {
            panic!("not failed");
        };
        assert_eq!(message, LOCKED);
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
