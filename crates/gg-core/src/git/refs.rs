//! Reads branches, remote-tracking branches, tags, remotes and HEAD.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

pub use crate::dto::{HeadInfo, RefKind};
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner, git_failed};

/// `for-each-ref` format: NUL-separated fields (NUL cannot occur in a ref name, and git
/// refuses it in a commit message), each ref's record ending in a NUL before git's own newline, so that the
/// subject, the last field, can never run into the next record. Fields: refname, object
/// type, object, peeled type, peeled object (both empty unless the ref is an annotated tag;
/// git peels nested tags fully), upstream, the upstream's remote and its branch there as
/// configured (`branch.<name>.remote` and `.merge`: `origin` and `refs/heads/main`; `.` for a
/// local branch), upstream tracking (`[ahead N, behind M]`,
/// `[gone]` or empty), symref target; then of the commit a branch points at (empty for an
/// annotated tag, whose object is the tag), its committer time (unix seconds), author name
/// and subject (git joins a first paragraph of several lines with spaces). These last three
/// come from the commit object `%(objecttype)` reads anyway: no other git command is needed
/// for them.
const REF_FORMAT: &str = "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream:track)%00%(symref)%00%(committerdate:unix)%00%(authorname)%00%(contents:subject)%00";
const REF_FIELDS: usize = 13;
/// What ends a record: the format's final NUL and git's newline.
const RECORD_END: &[u8] = b"\0\n";

/// A branch, remote-tracking branch or tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefInfo {
    pub kind: RefKind,
    /// Short name: `main`, `origin/main`, `v1.0`.
    pub name: String,
    /// The commit the ref points at (annotated tags peeled).
    pub hash: String,
    /// Short name of the configured upstream (kept when the upstream is gone).
    pub upstream: Option<String>,
    /// The upstream's remote as configured (`origin`; `.` for a local branch): its short
    /// name cannot say where the remote's name ends when one remote's name starts with
    /// another's (`up` and `up/stream`).
    pub upstream_remote: Option<String>,
    /// The upstream's branch on that remote (`main` of `refs/heads/main`).
    pub upstream_branch: Option<String>,
    /// Commits on this branch that are not on its upstream (0 when there is none).
    pub ahead: u32,
    /// Commits on the upstream that are not on this branch (0 when there is none).
    pub behind: u32,
    /// Of a branch or remote-tracking branch, its commit's committer time, unix seconds (0
    /// when unknown, and for tags).
    pub time: i64,
    /// Of a branch or remote-tracking branch, its commit's author name (empty for tags).
    pub author: String,
    /// Of a branch or remote-tracking branch, its commit's subject (empty for tags).
    pub subject: String,
}

/// Local branches, remote-tracking branches and tags, sorted by full ref name. Remote
/// `HEAD` symrefs and refs that do not point at a commit are left out.
pub async fn read_refs(r: &GitRunner, cwd: &Path) -> Result<Vec<RefInfo>> {
    Ok(read_refs_and_heads(r, cwd).await?.0)
}

/// [`read_refs`], and from the same `for-each-ref`, each remote's default branch as its
/// `HEAD` symref names it (`origin` → `main`; set by a clone or `git remote set-head`).
pub async fn read_refs_and_heads(
    r: &GitRunner,
    cwd: &Path,
) -> Result<(Vec<RefInfo>, BTreeMap<String, String>)> {
    let out = r
        .run(
            cwd,
            &[
                "for-each-ref",
                REF_FORMAT,
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ],
            CmdKind::Read,
        )
        .await?;
    Ok((parse_refs(&out.stdout), parse_remote_heads(&out.stdout)))
}

/// Names of the configured remotes.
pub async fn read_remotes(r: &GitRunner, cwd: &Path) -> Result<Vec<String>> {
    let out = r.run(cwd, &["remote"], CmdKind::Read).await?;
    Ok(parse_remotes(&out.stdout))
}

/// The branch and commit HEAD points at in the worktree containing `cwd`.
pub async fn read_head(r: &GitRunner, cwd: &Path) -> Result<HeadInfo> {
    const SYMREF: [&str; 3] = ["symbolic-ref", "-q", "HEAD"];
    const REV: [&str; 4] = ["rev-parse", "-q", "--verify", "HEAD^{commit}"];
    let (sym, rev) = tokio::join!(
        r.run_unchecked(cwd, &SYMREF, CmdKind::Read),
        r.run_unchecked(cwd, &REV, CmdKind::Read),
    );
    let (sym, rev) = (sym?, rev?);
    // Both commands exit 1 for "no such thing" (detached / unborn) and 128 on real errors.
    let branch = match sym.code {
        0 => non_empty(short_name(sym.text().trim())),
        1 => None,
        code => return Err(git_failed(&SYMREF, code, &sym.stderr)),
    };
    let hash = match rev.code {
        0 => non_empty(rev.text().trim()),
        1 => None,
        code => return Err(git_failed(&REV, code, &rev.stderr)),
    };
    Ok(HeadInfo { branch, hash })
}

fn parse_refs(out: &[u8]) -> Vec<RefInfo> {
    records(out).filter_map(parse_ref_line).collect()
}

/// Each remote's `HEAD` symref (`refs/remotes/origin/HEAD` → `refs/remotes/origin/main`) as
/// remote → branch (`origin` → `main`); one naming a branch of another remote, or none, is
/// left out.
fn parse_remote_heads(out: &[u8]) -> BTreeMap<String, String> {
    records(out)
        .filter_map(|record| {
            let mut fields = record.split(|&b| b == 0);
            let refname = String::from_utf8_lossy(fields.next()?);
            // The symref: the tenth field (see REF_FORMAT), the eighth after the refname.
            let symref = String::from_utf8_lossy(fields.nth(8)?);
            let remote = refname
                .strip_prefix("refs/remotes/")?
                .strip_suffix("/HEAD")?;
            let branch = symref.strip_prefix(&format!("refs/remotes/{remote}/"))?;
            (!remote.is_empty() && !branch.is_empty() && branch != "HEAD")
                .then(|| (remote.to_owned(), branch.to_owned()))
        })
        .collect()
}

/// The records of `out`, each without its [`RECORD_END`].
fn records(out: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = out;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let at = rest.windows(RECORD_END.len()).position(|w| w == RECORD_END);
        let (record, next) = match at {
            Some(at) => (&rest[..at], &rest[at + RECORD_END.len()..]),
            None => (rest, &rest[rest.len()..]),
        };
        rest = next;
        Some(record)
    })
}

fn parse_ref_line(line: &[u8]) -> Option<RefInfo> {
    let fields: Vec<String> = line
        .splitn(REF_FIELDS, |&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let [
        refname,
        ty,
        obj,
        peeled_ty,
        peeled,
        upstream,
        upstream_remote,
        upstream_ref,
        track,
        symref,
        time,
        author,
        subject,
    ] = <[String; REF_FIELDS]>::try_from(fields).ok()?;
    let (kind, name) = [
        ("refs/heads/", RefKind::Branch),
        ("refs/remotes/", RefKind::Remote),
        ("refs/tags/", RefKind::Tag),
    ]
    .into_iter()
    .find_map(|(prefix, kind)| refname.strip_prefix(prefix).map(|n| (kind, n)))?;
    if name.is_empty() {
        return None;
    }
    if kind == RefKind::Remote && (!symref.is_empty() || name.ends_with("/HEAD")) {
        return None;
    }
    let (ty, hash) = if peeled.is_empty() {
        (ty, obj)
    } else {
        (peeled_ty, peeled)
    };
    if ty != "commit" || hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (ahead, behind) = parse_track(&track);
    // A tag's commit is not shown anywhere these are: not kept.
    let (time, author, subject) = if kind == RefKind::Tag {
        (0, String::new(), String::new())
    } else {
        (time.trim().parse().unwrap_or(0), author, subject)
    };
    Some(RefInfo {
        kind,
        name: name.to_owned(),
        hash,
        upstream: non_empty(short_name(&upstream)),
        upstream_remote: non_empty(&upstream_remote),
        upstream_branch: non_empty(
            upstream_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&upstream_ref),
        ),
        ahead,
        behind,
        time,
        author,
        subject,
    })
}

/// Ahead/behind counts from `%(upstream:track)`; anything unreadable counts as 0.
fn parse_track(track: &str) -> (u32, u32) {
    let inner = track.trim().trim_start_matches('[').trim_end_matches(']');
    let (mut ahead, mut behind) = (0, 0);
    for part in inner.split(',') {
        let mut words = part.split_whitespace();
        match (words.next(), words.next()) {
            (Some("ahead"), Some(n)) => ahead = n.parse().unwrap_or(0),
            (Some("behind"), Some(n)) => behind = n.parse().unwrap_or(0),
            _ => {}
        }
    }
    (ahead, behind)
}

fn parse_remotes(out: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(out)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `refs/heads/x` → `x`, `refs/remotes/o/x` → `o/x`; anything else unchanged.
fn short_name(full: &str) -> &str {
    full.strip_prefix("refs/heads/")
        .or_else(|| full.strip_prefix("refs/remotes/"))
        .unwrap_or(full)
}

fn non_empty(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const T: &str = "dddddddddddddddddddddddddddddddddddddddd";

    /// One `for-each-ref` record in `REF_FORMAT` field order, its commit made at time 7 by
    /// Ada with subject "Tip" (none for an annotated tag, as git gives it).
    fn line(
        refname: &str,
        ty: &str,
        obj: &str,
        peeled: &str,
        upstream: &str,
        track: &str,
        symref: &str,
    ) -> String {
        let peeled_ty = if peeled.is_empty() { "" } else { "commit" };
        let commit = if ty == "commit" {
            "7\0Ada\0Tip"
        } else {
            "\0\0"
        };
        format!(
            "{refname}\0{ty}\0{obj}\0{peeled_ty}\0{peeled}\0{upstream}\0\0\0{track}\0{symref}\0{commit}\0\n"
        )
    }

    /// A branch's or remote branch's `RefInfo` from `line`: its commit is Ada's "Tip" at 7.
    fn tip(kind: RefKind, name: &str, hash: &str, upstream: Option<&str>, ahead: u32) -> RefInfo {
        RefInfo {
            kind,
            name: name.into(),
            hash: hash.into(),
            upstream: upstream.map(Into::into),
            upstream_remote: None,
            upstream_branch: None,
            ahead,
            behind: 0,
            time: 7,
            author: "Ada".into(),
            subject: "Tip".into(),
        }
    }

    #[test]
    fn remote_heads_name_each_remotes_default_branch() {
        let out = [
            line("refs/heads/main", "commit", C, "", "", "", ""),
            line(
                "refs/remotes/origin/HEAD",
                "commit",
                C,
                "",
                "",
                "",
                "refs/remotes/origin/main",
            ),
            line("refs/remotes/origin/main", "commit", C, "", "", "", ""),
            line(
                "refs/remotes/my/fork/HEAD",
                "commit",
                C,
                "",
                "",
                "",
                "refs/remotes/my/fork/trunk",
            ),
            // Pointing at another remote's branch, or at nothing: left out.
            line(
                "refs/remotes/odd/HEAD",
                "commit",
                C,
                "",
                "",
                "",
                "refs/remotes/origin/main",
            ),
            line("refs/remotes/bare/HEAD", "commit", C, "", "", "", ""),
        ]
        .concat();
        assert_eq!(
            parse_remote_heads(out.as_bytes()),
            BTreeMap::from([
                ("my/fork".to_owned(), "trunk".to_owned()),
                ("origin".to_owned(), "main".to_owned()),
            ])
        );
        // The refs themselves are as before: no HEAD among them.
        assert!(
            parse_refs(out.as_bytes())
                .iter()
                .all(|r| !r.name.ends_with("HEAD"))
        );
        assert!(parse_remote_heads(b"").is_empty());
    }

    #[test]
    fn parses_kinds_and_short_names() {
        let out = [
            line(
                "refs/heads/main",
                "commit",
                C,
                "",
                "refs/remotes/origin/main",
                "[ahead 1]",
                "",
            ),
            line("refs/heads/feature/ünï-✨", "commit", C, "", "", "", ""),
            line(
                "refs/remotes/origin/HEAD",
                "commit",
                C,
                "",
                "",
                "",
                "refs/remotes/origin/main",
            ),
            line("refs/remotes/origin/main", "commit", C, "", "", "", ""),
            line("refs/tags/v1", "tag", T, C, "", "", ""),
            line("refs/tags/light", "commit", C, "", "", "", ""),
        ]
        .concat();
        let refs = parse_refs(out.as_bytes());
        assert_eq!(
            refs,
            [
                tip(RefKind::Branch, "main", C, Some("origin/main"), 1),
                tip(RefKind::Branch, "feature/ünï-✨", C, None, 0),
                tip(RefKind::Remote, "origin/main", C, None, 0),
                // Tags keep no commit details, annotated or not.
                RefInfo {
                    time: 0,
                    author: String::new(),
                    subject: String::new(),
                    ..tip(RefKind::Tag, "v1", C, None, 0)
                },
                RefInfo {
                    time: 0,
                    author: String::new(),
                    subject: String::new(),
                    ..tip(RefKind::Tag, "light", C, None, 0)
                },
            ]
        );
    }

    #[test]
    fn a_subject_with_line_ends_stays_in_its_record() {
        // git joins a first paragraph's lines; a message made otherwise (or an older git) may
        // still bring a line end: the record ends only at NUL and newline together.
        let out = format!(
            "refs/heads/a\0commit\0{C}\0\0\0\0\0\0\0\01700000000\0Ada Lovelace\0one\ntwo\0\n\
             refs/heads/b\0commit\0{C}\0\0\0\0\0\0\0\0\0Grace\0\0\n"
        );
        let refs = parse_refs(out.as_bytes());
        assert_eq!(refs.len(), 2, "{refs:?}");
        assert_eq!(
            (
                refs[0].time,
                refs[0].author.as_str(),
                refs[0].subject.as_str()
            ),
            (1_700_000_000, "Ada Lovelace", "one\ntwo")
        );
        // No time (unreadable) counts as 0; an empty subject stays empty.
        assert_eq!(
            (
                refs[1].name.as_str(),
                refs[1].time,
                refs[1].author.as_str(),
                refs[1].subject.as_str()
            ),
            ("b", 0, "Grace", "")
        );
    }

    #[test]
    fn local_upstream_is_short_too() {
        let out = line(
            "refs/heads/topic",
            "commit",
            C,
            "",
            "refs/heads/main",
            "[behind 2]",
            "",
        );
        let refs = parse_refs(out.as_bytes());
        assert_eq!(refs[0].upstream.as_deref(), Some("main"));
        assert_eq!((refs[0].ahead, refs[0].behind), (0, 2));
    }

    /// `stream/main` on remote `up` and `main` on remote `up/stream` both read
    /// `up/stream/main`: the configured remote and branch say which it is.
    #[test]
    fn the_upstream_remote_and_branch_are_read_as_configured() {
        let out = format!(
            "refs/heads/main\0commit\0{C}\0\0\0refs/remotes/up/stream/main\0up\0refs/heads/stream/main\0[ahead 1]\0\07\0Ada\0Tip\0\n\
             refs/heads/topic\0commit\0{C}\0\0\0refs/heads/main\0.\0refs/heads/main\0\0\07\0Ada\0Tip\0\n\
             refs/heads/alone\0commit\0{C}\0\0\0\0\0\0\0\07\0Ada\0Tip\0\n"
        );
        let refs = parse_refs(out.as_bytes());
        assert_eq!(
            refs.iter()
                .map(|r| (
                    r.upstream.as_deref(),
                    r.upstream_remote.as_deref(),
                    r.upstream_branch.as_deref(),
                ))
                .collect::<Vec<_>>(),
            [
                (Some("up/stream/main"), Some("up"), Some("stream/main")),
                (Some("main"), Some("."), Some("main")),
                (None, None, None),
            ]
        );
        assert_eq!(refs[0].ahead, 1);
    }

    #[test]
    fn skips_tags_that_do_not_point_at_commits() {
        let out = [
            line("refs/tags/tree", "tree", C, "", "", "", ""),
            format!("refs/tags/blobtag\0tag\0{T}\0blob\0{C}\0\0\0\0\0\0\0\0\0\n"),
        ]
        .concat();
        assert!(parse_refs(out.as_bytes()).is_empty());
    }

    #[test]
    fn parses_track() {
        assert_eq!(parse_track(""), (0, 0));
        assert_eq!(parse_track("[ahead 3]"), (3, 0));
        assert_eq!(parse_track("[behind 4]"), (0, 4));
        assert_eq!(parse_track("[ahead 3, behind 4]"), (3, 4));
        assert_eq!(parse_track("[gone]"), (0, 0));
        assert_eq!(parse_track("[ahead x, behind 99999999999999]"), (0, 0));
        assert_eq!(parse_track("[ahead"), (0, 0));
        assert_eq!(parse_track("ahead 1"), (1, 0));
    }

    #[test]
    fn survives_hostile_input() {
        assert!(parse_refs(b"").is_empty());
        assert!(parse_refs(b"\n\n\0\0\n").is_empty());
        assert!(parse_refs(b"\xFF\xFE\0\0").is_empty());
        // Truncated records: too few fields, missing hash, unknown namespace.
        assert!(parse_refs(b"refs/heads/main\0commit\0").is_empty());
        assert!(parse_refs(b"refs/heads/main\0commit\0\0\0\0\0\0\0\0\0\0\0\0\n").is_empty());
        assert!(
            parse_refs(format!("refs/notes/x\0commit\0{C}\0\0\0\0\0\0\0\0\0\0\0\n").as_bytes())
                .is_empty()
        );
        assert!(
            parse_refs(format!("refs/heads/\0commit\0{C}\0\0\0\0\0\0\0\0\0\0\0\n").as_bytes())
                .is_empty()
        );
        // A record of the format before the commit's fields (8 fields) is not taken.
        assert!(parse_refs(format!("refs/heads/a\0commit\0{C}\0\0\0\0\0\n").as_bytes()).is_empty());
        // Non-UTF-8 names decode lossily.
        let refs = parse_refs(
            format!("refs/heads/a\u{FFFD}\0commit\0{C}\0\0\0\0\0\0\0\0\0\0\0\n").as_bytes(),
        );
        assert_eq!(refs.len(), 1);
        let mut raw = b"refs/heads/b\xFF".to_vec();
        raw.extend_from_slice(format!("\0commit\0{C}\0\0\0\0\0\0\0\0\0").as_bytes());
        raw.extend_from_slice(b"A\xFF\0s\xFE\0\n");
        let refs = parse_refs(&raw);
        assert_eq!(refs[0].name, "b\u{FFFD}");
        assert_eq!(refs[0].author, "A\u{FFFD}");
        assert_eq!(refs[0].subject, "s\u{FFFD}");
    }

    #[test]
    fn ref_kind_serializes_lowercase() {
        assert_eq!(serde_json::to_value(RefKind::Branch).unwrap(), "branch");
        assert_eq!(serde_json::to_value(RefKind::Remote).unwrap(), "remote");
        assert_eq!(serde_json::to_value(RefKind::Tag).unwrap(), "tag");
    }

    #[test]
    fn parses_remotes() {
        assert_eq!(parse_remotes(b"origin\nupstream\n"), ["origin", "upstream"]);
        assert!(parse_remotes(b"").is_empty());
        assert_eq!(parse_remotes(b"\n o \n\xFF\n"), ["o", "\u{FFFD}"]);
    }
}
