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

/// The failed step for `out`, which git (run with `args`) ended with a non-zero exit code.
pub fn explain(args: &[&str], out: &GitOutput) -> Step {
    let detail = if out.stderr.trim().is_empty() {
        out.text()
    } else {
        out.stderr.clone()
    };
    let hints: Vec<OpHint> = Vec::new();
    let message = if lock_file_exists(&detail) {
        LOCKED.to_owned()
    } else {
        git_failed(args, out.code, &detail).message
    };
    Step::Failed {
        message,
        detail,
        hints,
    }
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
        } = explain(&["commit"], &failed("", "nothing to commit\n"))
        else {
            panic!("not failed");
        };
        assert_eq!(message, "nothing to commit");
        assert_eq!(detail, "nothing to commit\n");
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
