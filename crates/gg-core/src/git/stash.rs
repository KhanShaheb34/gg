//! Reads the stash list: one `git stash list` with an explicit format, NUL-separated.

use std::path::Path;

use crate::error::Result;
use crate::git::runner::{CmdKind, GitRunner};

/// `stash list` format: fields separated by US (0x1f), the subject last (so a US inside it
/// survives); records separated by NUL (`-z`), which a commit message cannot contain.
/// Fields: commit, parents, reflog selector (`stash@{0}`), author name, author email,
/// author time, subject.
const STASH_FORMAT: &str = "--format=%H%x1f%P%x1f%gd%x1f%an%x1f%ae%x1f%at%x1f%s";
const STASH_FIELDS: usize = 7;
const FIELD_SEP: u8 = 0x1f;

/// One stash entry. Its commit (`hash`) has the commit the changes were stashed on as its
/// first parent (`base`), the commit holding the staged changes second (`index_parent`) and,
/// for a stash that includes untracked files, the commit holding those third
/// (`untracked_parent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    pub hash: String,
    /// `stash@{n}`: where it is in the list now (it moves as stashes are added and dropped).
    pub selector: String,
    pub base: String,
    pub index_parent: Option<String>,
    pub untracked_parent: Option<String>,
    pub author: String,
    pub email: String,
    /// Author time, unix seconds (0 if git's value could not be read).
    pub time: i64,
    /// What git stores as the stash's subject: "WIP on main: …", or "On main: <message>".
    pub subject: String,
}

/// The stashes, newest (`stash@{0}`) first; none when there is no stash (or no commit yet).
pub async fn read_stashes(r: &GitRunner, cwd: &Path) -> Result<Vec<Stash>> {
    let out = r
        .run(cwd, &["stash", "list", "-z", STASH_FORMAT], CmdKind::Read)
        .await?;
    Ok(parse_stashes(&out.stdout))
}

/// Parses `stash list -z` output in [`STASH_FORMAT`]. Text is decoded lossily; malformed
/// records are skipped. Never panics.
pub fn parse_stashes(out: &[u8]) -> Vec<Stash> {
    out.split(|&b| b == 0).filter_map(parse_record).collect()
}

fn parse_record(rec: &[u8]) -> Option<Stash> {
    // A record may start with the newline that ends the one before.
    let rec = rec.trim_ascii_start();
    let fields: Vec<&[u8]> = rec.splitn(STASH_FIELDS, |&b| b == FIELD_SEP).collect();
    let [hash, parents, selector, author, email, time, subject] =
        <[&[u8]; STASH_FIELDS]>::try_from(fields).ok()?;
    let hex = |h: &str| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit());
    let hash = lossy(hash);
    let parents = lossy(parents);
    let mut parents = parents.split_ascii_whitespace().map(str::to_owned);
    let base = parents.next()?;
    let selector = lossy(selector);
    if !hex(&hash) || !hex(&base) || !selector.starts_with("stash@{") {
        return None;
    }
    let subject = lossy(subject);
    Some(Stash {
        hash,
        selector,
        base,
        index_parent: parents.next(),
        untracked_parent: parents.next(),
        author: lossy(author),
        email: lossy(email),
        time: lossy(time).trim().parse().unwrap_or(0),
        // `--format` puts a newline after the last record.
        subject: subject.strip_suffix('\n').unwrap_or(&subject).to_owned(),
    })
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const I: &str = "3333333333333333333333333333333333333333";

    fn record(hash: &str, parents: &str, n: u32, subject: &[u8]) -> Vec<u8> {
        let mut r =
            format!("{hash}\x1f{parents}\x1fstash@{{{n}}}\x1fAda\x1fada@x\x1f1700000000\x1f")
                .into_bytes();
        r.extend_from_slice(subject);
        r
    }

    #[test]
    fn parses_records_and_skips_malformed_ones() {
        let mut out = record(W, &format!("{B} {I}"), 0, b"WIP on main: x");
        out.push(0);
        out.extend(b"junk\x1f\x1f\0");
        out.extend(record("zz", B, 1, b"bad hash"));
        out.push(0);
        out.extend(record(W, "", 2, b"no parents"));
        out.push(0);
        out.extend(record(I, B, 3, b"On dev: a\x1fb\n"));
        let stashes = parse_stashes(&out);
        assert_eq!(stashes.len(), 2, "{stashes:?}");
        assert_eq!(stashes[0].index_parent.as_deref(), Some(I));
        assert_eq!(stashes[0].untracked_parent, None);
        assert_eq!(stashes[0].time, 1_700_000_000);
        assert_eq!(stashes[1].selector, "stash@{3}");
        assert_eq!(stashes[1].subject, "On dev: a\x1fb");
    }

    #[test]
    fn survives_hostile_input() {
        assert!(parse_stashes(b"").is_empty());
        assert!(parse_stashes(b"\0\0\n\0").is_empty());
        assert!(parse_stashes(b"\xff\xfe\x1f\x1f\x1f\x1f\x1f\x1f\0").is_empty());
        let mut out = record(W, B, 0, b"\xff");
        out.extend(b"\0");
        assert_eq!(parse_stashes(&out)[0].subject, "\u{FFFD}");
    }
}
