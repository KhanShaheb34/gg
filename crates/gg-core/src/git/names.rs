//! Checks on branch and tag names the user typed, before any git command sees them: the
//! rules of `git check-ref-format --branch`, plus no leading `-` (so a name can never be
//! taken for an option) and not `HEAD`. A rejected name is a `bad_name` error that says
//! why in plain words.

use crate::error::{CoreError, ErrorCode, Result};

/// `Ok` if `name` can be a new branch's name.
pub fn check_branch_name(name: &str) -> Result<()> {
    check_name("branch", name)
}

/// `Ok` if `name` can be a new tag's name.
pub fn check_tag_name(name: &str) -> Result<()> {
    check_name("tag", name)
}

fn check_name(what: &str, name: &str) -> Result<()> {
    match problem(name) {
        None => Ok(()),
        Some(why) => Err(CoreError::new(
            ErrorCode::BadName,
            format!("{} can't be a {what} name: {why}.", shown(name)),
        )),
    }
}

/// `name` quoted for a message; an empty one is said as such.
fn shown(name: &str) -> String {
    if name.is_empty() {
        "An empty name".to_owned()
    } else {
        format!("\u{201c}{name}\u{201d}")
    }
}

/// Why `name` is not a usable ref name, or `None` if it is.
fn problem(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("a name is needed");
    }
    if name.starts_with('-') {
        return Some("it can't start with a hyphen");
    }
    if name == "HEAD" {
        return Some("HEAD is git's name for the current commit");
    }
    if name == "@" {
        return Some("\u{201c}@\u{201d} alone is git's name for HEAD");
    }
    if let Some(c) = name.chars().find(|&c| c.is_ascii_control() || c == ' ') {
        return Some(if c == ' ' {
            "it can't contain spaces"
        } else {
            "it can't contain control characters"
        });
    }
    if name
        .chars()
        .any(|c| matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
    {
        return Some("it can't contain any of: ~ ^ : ? * [ \\ or a space");
    }
    if name.contains("..") {
        return Some("it can't contain two dots in a row");
    }
    if name.contains("@{") {
        return Some("it can't contain \u{201c}@{\u{201d} anywhere");
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Some("it can't start or end with a slash, or have two in a row");
    }
    if name.ends_with('.') {
        return Some("it can't end with a dot");
    }
    for part in name.split('/') {
        if part.starts_with('.') {
            return Some("no part of it can start with a dot");
        }
        if part.ends_with(".lock") {
            return Some("no part of it can end in \u{201c}.lock\u{201d}, git's own ending");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    const GOOD: [&str; 8] = [
        "main",
        "feature/x",
        "feature/émoji-🚀",
        "a.b",
        "v1.2.3",
        "a@b",
        "under_score-dash",
        "deep/er/path",
    ];
    const BAD: [&str; 26] = [
        "",
        "-f",
        "--force",
        "HEAD",
        "@",
        "has space",
        "a..b",
        "a~1",
        "a^",
        "a:b",
        "a?",
        "a*",
        "a[b",
        "a\\b",
        "a@{1}",
        "/a",
        "a/",
        "a//b",
        "a.",
        ".a",
        "a/.b",
        "a.lock",
        "a.lock/b",
        "tab\there",
        "nl\nhere",
        "del\u{7f}",
    ];

    #[test]
    fn good_and_bad_names() {
        for name in GOOD {
            assert!(check_branch_name(name).is_ok(), "{name:?} rejected");
            assert!(check_tag_name(name).is_ok(), "{name:?} rejected as a tag");
        }
        for name in BAD {
            let err = check_branch_name(name).expect_err(name);
            assert_eq!(err.code, ErrorCode::BadName);
            assert!(check_tag_name(name).is_err(), "{name:?} accepted as a tag");
        }
    }

    #[test]
    fn messages_say_why() {
        let msg = |n: &str| check_branch_name(n).unwrap_err().message;
        assert!(msg("-f").contains("start with"), "{}", msg("-f"));
        assert!(msg("has space").contains("spaces"));
        assert!(msg("").starts_with("An empty name"));
        assert!(msg("a..b").contains(".."));
        assert!(
            check_tag_name("x y")
                .unwrap_err()
                .message
                .contains("tag name")
        );
    }

    /// The rules agree with git's own: every name accepted here is accepted by
    /// `git check-ref-format --branch`, and every name git rejects is rejected here. (The
    /// leading `-` and `HEAD` are rejected here on purpose, beyond git.)
    #[test]
    fn agrees_with_git_check_ref_format() {
        let repo = TestRepo::new();
        for name in GOOD.iter().chain(BAD.iter()) {
            if name.starts_with('-') {
                continue; // git would read it as an option
            }
            let git_ok = repo.git_succeeds(&["check-ref-format", "--branch", name]);
            let ours = check_branch_name(name).is_ok();
            if ours {
                assert!(git_ok, "{name:?} accepted here, rejected by git");
            }
            if !git_ok {
                assert!(!ours, "{name:?} rejected by git, accepted here");
            }
        }
    }
}
