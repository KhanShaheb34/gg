//! Reads one commit's metadata, message and changed files.

use std::collections::HashMap;
use std::path::Path;

use crate::dto::{CommitDetails, FileChange, FileStatus};
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::{CmdKind, GitRunner};

/// NUL-separated: hash, parents, author name, email, time, committer name, email, time,
/// decorations, raw message (last, so a message can hold anything). With every ref and HEAD
/// left out of the decorations (see [`read_details`]), only git's own markers remain, such as
/// `grafted` on the commit where a shallow clone's history is cut.
const META_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%D%x00%B";
const META_FIELDS: usize = 10;

/// Metadata, message and changed files of commit `hash` (a full 40 or 64 hex digit name;
/// anything else is `BadRequest`). Files are compared with the first parent (a root commit
/// with the empty tree), with renames found. External diff and textconv programs configured
/// in the repository are never run.
pub async fn read_details(r: &GitRunner, cwd: &Path, hash: &str) -> Result<CommitDetails> {
    if !is_commit_hash(hash) {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("not a commit hash: {hash:?}"),
        ));
    }
    let rev = format!("{hash}^{{commit}}");
    let out = r
        .run(
            cwd,
            &[
                "show",
                "-s",
                "--no-show-signature",
                "--no-ext-diff",
                "--no-textconv",
                // Decorations without refs: a branch could be named "grafted".
                "--decorate-refs-exclude=refs/*",
                "--decorate-refs-exclude=HEAD",
                META_FORMAT,
                &rev,
                "--",
            ],
            CmdKind::Read,
        )
        .await?;
    let mut details = parse_meta(&out.stdout).ok_or_else(|| {
        CoreError::new(ErrorCode::GitFailed, "could not read commit metadata")
            .with_detail(out.text())
    })?;

    let mut args = vec![
        "diff-tree",
        "-r",
        "-z",
        "-M",
        "--raw",
        "--numstat",
        "--no-commit-id",
        "--no-ext-diff",
        "--no-textconv",
    ];
    match details.parents.first() {
        Some(parent) => args.push(parent),
        None => args.push("--root"),
    }
    args.push(&details.hash);
    let out = r.run(cwd, &args, CmdKind::Read).await?;
    details.files = parse_files(&out.stdout);
    Ok(details)
}

/// A full object name: 40 (SHA-1) or 64 (SHA-256) hex digits. Rejects abbreviations,
/// which can be ambiguous, and anything git could read as an option or a ref name.
fn is_commit_hash(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parses [`META_FORMAT`] output; `files` is left empty. `None` if fields are missing or
/// the hash is not hex.
fn parse_meta(out: &[u8]) -> Option<CommitDetails> {
    let fields: Vec<String> = out
        .splitn(META_FIELDS, |&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    let [
        hash,
        parents,
        author,
        author_email,
        author_time,
        committer,
        committer_email,
        commit_time,
        decorations,
        message,
    ] = <[String; META_FIELDS]>::try_from(fields).ok()?;
    if hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let parents: Vec<String> = parents
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect();
    let grafted = decorations.split(", ").any(|d| d.trim() == "grafted");
    Some(CommitDetails {
        hash,
        shallow_boundary: parents.is_empty() && grafted,
        parents,
        author,
        author_email,
        author_time: author_time.trim().parse().unwrap_or(0),
        committer,
        committer_email,
        commit_time: commit_time.trim().parse().unwrap_or(0),
        message: message.trim_end().to_owned(),
        files: Vec::new(),
    })
}

/// Parses `diff-tree -z --raw --numstat` output. Raw entries come first:
/// `:<modes> <oids> <status>` then one path, or two (source, destination) for a rename or
/// copy. Numstat entries follow: `<added>\t<deleted>\t<path>`, or `<added>\t<deleted>\t`
/// then source and destination for a rename or copy; `-` counts mark binary files.
/// Entries with an unknown status or missing paths are skipped.
fn parse_files(out: &[u8]) -> Vec<FileChange> {
    let mut files = Vec::new();
    let mut counts: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::new();
    let mut tokens = out.split(|&b| b == 0);
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            continue;
        }
        if let Some(header) = token.strip_prefix(b":") {
            let header = String::from_utf8_lossy(header);
            let code = header.split_ascii_whitespace().last().unwrap_or("");
            let status = match code.as_bytes().first() {
                Some(b'A') => Some(FileStatus::A),
                Some(b'M') => Some(FileStatus::M),
                Some(b'D') => Some(FileStatus::D),
                Some(b'R') => Some(FileStatus::R),
                Some(b'C') => Some(FileStatus::C),
                Some(b'T') => Some(FileStatus::T),
                _ => None,
            };
            let Some(first) = next_path(&mut tokens) else {
                break;
            };
            let (path, old_path) = if matches!(status, Some(FileStatus::R | FileStatus::C)) {
                let Some(dest) = next_path(&mut tokens) else {
                    break;
                };
                (dest, Some(first))
            } else {
                (first, None)
            };
            if let Some(status) = status {
                files.push(FileChange {
                    path,
                    old_path,
                    status,
                    additions: None,
                    deletions: None,
                });
            }
        } else {
            let mut parts = token.splitn(3, |&b| b == b'\t');
            let (Some(added), Some(deleted), Some(path)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let path = if path.is_empty() {
                // Rename or copy: source, then destination.
                let _source = tokens.next();
                match next_path(&mut tokens) {
                    Some(dest) => dest,
                    None => break,
                }
            } else {
                lossy(path)
            };
            counts.insert(path, (parse_count(added), parse_count(deleted)));
        }
    }
    for f in &mut files {
        if let Some(&(added, deleted)) = counts.get(&f.path) {
            f.additions = added;
            f.deletions = deleted;
        }
    }
    files
}

/// The next token as a path; `None` if the output ends or the token is empty.
fn next_path<'a>(tokens: &mut impl Iterator<Item = &'a [u8]>) -> Option<String> {
    tokens.next().filter(|p| !p.is_empty()).map(lossy)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// A numstat count; `None` for `-` (binary) or anything unreadable.
fn parse_count(b: &[u8]) -> Option<u32> {
    std::str::from_utf8(b).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const Z: &str = "0000000000000000000000000000000000000000";
    const B1: &str = "1111111111111111111111111111111111111111";
    const B2: &str = "2222222222222222222222222222222222222222";

    fn raw(status: &str, paths: &[&str]) -> String {
        let mut s = format!(":100644 100644 {B1} {B2} {status}\0");
        for p in paths {
            s.push_str(p);
            s.push('\0');
        }
        s
    }

    fn change(
        path: &str,
        old: Option<&str>,
        status: FileStatus,
        add: Option<u32>,
        del: Option<u32>,
    ) -> FileChange {
        FileChange {
            path: path.into(),
            old_path: old.map(Into::into),
            status,
            additions: add,
            deletions: del,
        }
    }

    #[test]
    fn parses_raw_and_numstat() {
        let out = [
            raw("A", &["a.txt"]),
            raw("M", &["b.txt"]),
            raw("R100", &["c.txt", "dir with space/ünï ✨.txt"]),
            raw("D", &["gone.txt"]),
            raw("T", &["link"]),
            raw("C75", &["src.txt", "copy.txt"]),
            raw("A", &["image.bin"]),
            "2\t0\ta.txt\0".into(),
            "1\t1\tb.txt\0".into(),
            "0\t0\t\0c.txt\0dir with space/ünï ✨.txt\0".into(),
            "0\t3\tgone.txt\0".into(),
            "1\t1\tlink\0".into(),
            "4\t0\t\0src.txt\0copy.txt\0".into(),
            "-\t-\timage.bin\0".into(),
        ]
        .concat();
        assert_eq!(
            parse_files(out.as_bytes()),
            [
                change("a.txt", None, FileStatus::A, Some(2), Some(0)),
                change("b.txt", None, FileStatus::M, Some(1), Some(1)),
                change(
                    "dir with space/ünï ✨.txt",
                    Some("c.txt"),
                    FileStatus::R,
                    Some(0),
                    Some(0)
                ),
                change("gone.txt", None, FileStatus::D, Some(0), Some(3)),
                change("link", None, FileStatus::T, Some(1), Some(1)),
                change("copy.txt", Some("src.txt"), FileStatus::C, Some(4), Some(0)),
                change("image.bin", None, FileStatus::A, None, None),
            ]
        );
    }

    #[test]
    fn paths_that_look_like_headers_or_numstat() {
        let out = [
            raw("A", &[":100644 weird"]),
            raw("A", &["1\t2\tlooks like numstat"]),
            "1\t0\t:100644 weird\0".into(),
            "1\t0\t1\t2\tlooks like numstat\0".into(),
        ]
        .concat();
        assert_eq!(
            parse_files(out.as_bytes()),
            [
                change(":100644 weird", None, FileStatus::A, Some(1), Some(0)),
                change(
                    "1\t2\tlooks like numstat",
                    None,
                    FileStatus::A,
                    Some(1),
                    Some(0)
                ),
            ]
        );
    }

    #[test]
    fn files_survive_hostile_input() {
        assert!(parse_files(b"").is_empty());
        assert!(parse_files(b"\0\0\0").is_empty());
        assert!(parse_files(b"\xFF\xFE\0garbage\0").is_empty());
        // Truncated: header without paths, rename with only one path.
        assert!(parse_files(format!(":100644 100644 {B1} {B2} M\0").as_bytes()).is_empty());
        assert!(parse_files(format!(":100644 100644 {B1} {B2} R100\0old\0").as_bytes()).is_empty());
        // Unknown status letters are skipped, the rest kept.
        let out = [raw("X", &["what"]), raw("U", &["merge"]), raw("M", &["ok"])].concat();
        assert_eq!(
            parse_files(out.as_bytes()),
            [change("ok", None, FileStatus::M, None, None)]
        );
        // Numstat without a matching raw entry, or with bad numbers.
        let out = [
            raw("M", &["ok"]),
            "x\ty\tok\0".into(),
            "5\t5\tother\0".into(),
        ]
        .concat();
        assert_eq!(
            parse_files(out.as_bytes()),
            [change("ok", None, FileStatus::M, None, None)]
        );
        // Header that is only a colon; non-UTF-8 path.
        assert!(parse_files(b":\0").is_empty());
        let mut bytes = format!(":100644 100644 {B1} {Z} D\0").into_bytes();
        bytes.extend_from_slice(b"bad\xFF\0");
        assert_eq!(parse_files(&bytes)[0].path, "bad\u{FFFD}");
    }

    #[test]
    fn parses_meta() {
        let out = format!(
            "{B1}\0{B2} {Z}\0Ada\0ada@x\01700000000\0Bob\0bob@x\01700000060\0\0subject\n\nbody ünï\n\n"
        );
        let m = parse_meta(out.as_bytes()).unwrap();
        assert_eq!(m.hash, B1);
        assert_eq!(m.parents, [B2, Z]);
        assert_eq!(
            (m.author.as_str(), m.author_email.as_str()),
            ("Ada", "ada@x")
        );
        assert_eq!(
            (m.committer.as_str(), m.committer_email.as_str()),
            ("Bob", "bob@x")
        );
        assert_eq!(
            (m.author_time, m.commit_time),
            (1_700_000_000, 1_700_000_060)
        );
        assert_eq!(m.message, "subject\n\nbody ünï");
        assert!(!m.shallow_boundary);
        // Without parents and marked grafted: a shallow clone's boundary.
        let cut = format!("{B1}\0\0A\0a\01\0C\0c\02\0grafted\0msg");
        assert!(parse_meta(cut.as_bytes()).unwrap().shallow_boundary);
    }

    #[test]
    fn meta_survives_hostile_input() {
        assert!(parse_meta(b"").is_none());
        assert!(parse_meta(b"\0\0\0").is_none());
        assert!(parse_meta(format!("{B1}\0\0Ada\0").as_bytes()).is_none());
        let mut bytes = format!("{B1}\0\0A").into_bytes();
        bytes.extend_from_slice(b"\xFF\0e\0x\0C\0e\0y\0\0msg \xFF");
        let m = parse_meta(&bytes).unwrap();
        assert_eq!(m.author, "A\u{FFFD}");
        assert_eq!((m.author_time, m.commit_time), (0, 0));
        assert!(m.parents.is_empty());
        assert_eq!(m.message, "msg \u{FFFD}");
    }

    #[test]
    fn hash_validation() {
        assert!(is_commit_hash(B1));
        assert!(is_commit_hash(&"a".repeat(64)));
        assert!(is_commit_hash(&"ABCDEF12".repeat(5)));
        for bad in [
            "",
            "abc",
            "abcd",
            "ABCDEF12",
            &B1[..39],
            &format!("{B1}0"),
            &"a".repeat(63),
            "HEAD",
            "-p",
            "--output=x",
            "abcd ef",
            "ghij",
            &"a".repeat(65),
        ] {
            assert!(!is_commit_hash(bad), "{bad:?}");
        }
    }
}
