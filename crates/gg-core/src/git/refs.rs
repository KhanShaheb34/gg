//! Reads branches, remote-tracking branches, tags, remotes and HEAD.

use std::path::Path;

use serde::Serialize;

pub use crate::dto::{HeadInfo, RefKind};
use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner, git_failed};

/// `for-each-ref` format: NUL-separated fields (NUL cannot occur in ref names), one ref per
/// line. Fields: refname, object type, object, peeled type, peeled object (both empty unless
/// the ref is an annotated tag; git peels nested tags fully), upstream, upstream tracking
/// (`[ahead N, behind M]`, `[gone]` or empty), symref target.
const REF_FORMAT: &str = "--format=%(refname)%00%(objecttype)%00%(objectname)%00%(*objecttype)%00%(*objectname)%00%(upstream)%00%(upstream:track)%00%(symref)";
const REF_FIELDS: usize = 8;

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
    /// Commits on this branch that are not on its upstream (0 when there is none).
    pub ahead: u32,
    /// Commits on the upstream that are not on this branch (0 when there is none).
    pub behind: u32,
}

/// Local branches, remote-tracking branches and tags, sorted by full ref name. Remote
/// `HEAD` symrefs and refs that do not point at a commit are left out.
pub async fn read_refs(r: &GitRunner, cwd: &Path) -> Result<Vec<RefInfo>> {
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
    Ok(parse_refs(&out.stdout))
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
    out.split(|&b| b == b'\n')
        .filter_map(parse_ref_line)
        .collect()
}

fn parse_ref_line(line: &[u8]) -> Option<RefInfo> {
    let fields: Vec<String> = line
        .splitn(REF_FIELDS, |&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let [refname, ty, obj, peeled_ty, peeled, upstream, track, symref] =
        <[String; REF_FIELDS]>::try_from(fields).ok()?;
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
    Some(RefInfo {
        kind,
        name: name.to_owned(),
        hash,
        upstream: non_empty(short_name(&upstream)),
        ahead,
        behind,
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

    /// One `for-each-ref` line in `REF_FORMAT` field order.
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
        format!("{refname}\0{ty}\0{obj}\0{peeled_ty}\0{peeled}\0{upstream}\0{track}\0{symref}\n")
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
                RefInfo {
                    kind: RefKind::Branch,
                    name: "main".into(),
                    hash: C.into(),
                    upstream: Some("origin/main".into()),
                    ahead: 1,
                    behind: 0,
                },
                RefInfo {
                    kind: RefKind::Branch,
                    name: "feature/ünï-✨".into(),
                    hash: C.into(),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                },
                RefInfo {
                    kind: RefKind::Remote,
                    name: "origin/main".into(),
                    hash: C.into(),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                },
                RefInfo {
                    kind: RefKind::Tag,
                    name: "v1".into(),
                    hash: C.into(),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                },
                RefInfo {
                    kind: RefKind::Tag,
                    name: "light".into(),
                    hash: C.into(),
                    upstream: None,
                    ahead: 0,
                    behind: 0,
                },
            ]
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

    #[test]
    fn skips_tags_that_do_not_point_at_commits() {
        let out = [
            line("refs/tags/tree", "tree", C, "", "", "", ""),
            format!("refs/tags/blobtag\0tag\0{T}\0blob\0{C}\0\0\0\n"),
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
        assert!(parse_refs(b"refs/heads/main\0commit\0\0\0\0\0\0\n").is_empty());
        assert!(parse_refs(format!("refs/notes/x\0commit\0{C}\0\0\0\0\0\n").as_bytes()).is_empty());
        assert!(parse_refs(format!("refs/heads/\0commit\0{C}\0\0\0\0\0\n").as_bytes()).is_empty());
        // Non-UTF-8 names decode lossily.
        let refs = parse_refs(format!("refs/heads/a\u{FFFD}\0commit\0{C}\0\0\0\0\0\n").as_bytes());
        assert_eq!(refs.len(), 1);
        let mut raw = b"refs/heads/b\xFF".to_vec();
        raw.extend_from_slice(format!("\0commit\0{C}\0\0\0\0\0").as_bytes());
        assert_eq!(parse_refs(&raw)[0].name, "b\u{FFFD}");
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
