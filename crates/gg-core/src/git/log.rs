//! Reads the commit log: `git log` with a fixed machine-readable format, parsed incrementally.

use std::io;
use std::path::Path;
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::dto::ViewOptions;
use crate::error::{CoreError, ErrorCode, Result};
use crate::git::runner::{CmdKind, GitRunner, git_failed};

/// `git log` format: fields separated by US (0x1f), records terminated by RS (0x1e).
pub const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%at%x1f%ct%x1f%s%x1e";

const FIELD_SEP: u8 = 0x1f;
const RECORD_END: u8 = 0x1e;
/// Bytes read from git's stdout per read; each read yields at most one batch.
const READ_CHUNK: usize = 64 * 1024;

/// One commit as listed by `git log`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    /// Parent hashes, first parent first; empty for a root commit.
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    /// Author time, unix seconds (0 if git's value could not be read).
    pub time: i64,
    /// Committer time, unix seconds (0 if unreadable): what `--date-order` goes by.
    pub commit_time: i64,
    pub subject: String,
}

/// Incremental parser for `git log` output in [`LOG_FORMAT`]. Feed it chunks split at any
/// byte; each complete record is emitted as soon as its terminator arrives. Text is decoded
/// lossily; malformed records are skipped. Never panics.
#[derive(Debug, Default)]
pub struct LogParser {
    /// Bytes of the record in progress.
    buf: Vec<u8>,
    /// Prefix of `buf` already known to contain no record terminator.
    scanned: usize,
}

impl LogParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `bytes` and pushes every record they complete onto `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Commit>) {
        self.buf.extend_from_slice(bytes);
        let mut start = 0;
        let mut from = self.scanned;
        while let Some(off) = self.buf[from..].iter().position(|&b| b == RECORD_END) {
            let end = from + off;
            out.extend(parse_record(&self.buf[start..end]));
            start = end + 1;
            from = start;
        }
        self.buf.drain(..start);
        self.scanned = self.buf.len();
    }

    /// Ends the stream. Bytes after the last terminator belong to a record git did not
    /// finish writing (whose subject may be cut short), so they are dropped, not emitted.
    pub fn finish(self, _out: &mut Vec<Commit>) {}
}

/// Parses one record (without its terminator). `None` if it is malformed.
fn parse_record(rec: &[u8]) -> Option<Commit> {
    // `--format` ends every record with a newline, which lands before the next record.
    let mut fields = rec.trim_ascii_start().splitn(7, |&b| b == FIELD_SEP);
    let hash = fields.next()?;
    let parents = fields.next()?;
    let author = fields.next()?;
    let email = fields.next()?;
    let time = fields.next()?;
    let commit_time = fields.next()?;
    let subject = fields.next()?;
    if hash.is_empty() || !hash.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    Some(Commit {
        hash: lossy(hash),
        parents: lossy(parents)
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect(),
        author: lossy(author),
        email: lossy(email),
        time: lossy(time).trim().parse().unwrap_or(0),
        commit_time: lossy(commit_time).trim().parse().unwrap_or(0),
        subject: lossy(subject),
    })
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Arguments for the `git log` that lists the graph's commits, newest first.
///
/// With `branches: None`: all local branches, plus remote-tracking branches and tags when
/// shown, plus `HEAD`. With a selection: exactly those branches, each resolved as
/// `refs/remotes/<name>` when it is in `remote_branches`, else as `refs/heads/<name>`.
/// Always ends with `--`, so no revision can be taken for a file name.
pub fn log_args(opts: &ViewOptions, remote_branches: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec!["log".into(), LOG_FORMAT.into(), "--date-order".into()];
    match &opts.branches {
        None => {
            args.push("--branches".into());
            if opts.show_remotes {
                args.push("--remotes".into());
            }
            if opts.show_tags {
                args.push("--tags".into());
            }
            args.push("HEAD".into());
        }
        Some(names) => {
            args.extend(names.iter().map(|name| {
                if remote_branches.contains(name) {
                    format!("refs/remotes/{name}")
                } else {
                    format!("refs/heads/{name}")
                }
            }));
        }
    }
    // Everything before `--` is a revision, even where a file has the same name.
    args.push("--".into());
    args
}

/// Runs `git <args>` (normally from [`log_args`]) and calls `on_batch` with the commits
/// parsed from each chunk of output as it arrives (never with an empty batch). Stderr is
/// drained concurrently; the command is logged when git exits.
///
/// A nonzero exit is `GitFailed`, except when `HEAD` does not resolve to a commit and the
/// repository has no commits to show: then the result is `Ok(())` with no batches. If HEAD
/// is unborn but other refs exist (an orphan branch), `HEAD` is dropped from the arguments
/// and the log is read again.
pub async fn read_log(
    r: &GitRunner,
    cwd: &Path,
    args: &[String],
    mut on_batch: impl FnMut(Vec<Commit>),
) -> Result<()> {
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let first = stream_log(r, cwd, &argv, &mut on_batch).await?;
    if first.code == 0 {
        return Ok(());
    }
    let failed = git_failed(&argv, first.code, &first.stderr);
    if first.emitted || head_is_unborn(r, cwd).await? != Some(true) {
        return Err(failed);
    }
    // Only revisions (before any `--`) are candidates; a path named HEAD stays.
    let revs_end = argv.iter().position(|a| *a == "--").unwrap_or(argv.len());
    if argv[..revs_end].contains(&"HEAD") {
        let without_head: Vec<&str> = argv
            .iter()
            .enumerate()
            .filter(|&(i, a)| i >= revs_end || *a != "HEAD")
            .map(|(_, a)| *a)
            .collect();
        let retry = stream_log(r, cwd, &without_head, &mut on_batch).await?;
        if retry.code == 0 {
            return Ok(());
        }
        if retry.emitted {
            return Err(git_failed(&without_head, retry.code, &retry.stderr));
        }
    }
    if has_any_ref(r, cwd).await? {
        Err(failed)
    } else {
        Ok(())
    }
}

/// How a streamed `git log` ended.
struct StreamEnd {
    code: i32,
    stderr: String,
    /// At least one batch was delivered.
    emitted: bool,
}

async fn stream_log<F: FnMut(Vec<Commit>)>(
    r: &GitRunner,
    cwd: &Path,
    args: &[&str],
    on_batch: &mut F,
) -> Result<StreamEnd> {
    let started = Instant::now();
    // Counted as running until it has been waited for.
    let _live = r.live_child();
    let mut child = r.spawn_stream(cwd, args)?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(CoreError::new(ErrorCode::Io, "git output was not piped"));
    };
    let pumped = pump(stdout, stderr, on_batch, || {
        let _ = child.start_kill();
    })
    .await;
    if pumped.stderr.is_err() {
        // stdout may be unread from here on; make sure git does not linger.
        let _ = child.start_kill();
    }
    let status = child.wait().await;
    let code = status.as_ref().ok().and_then(|s| s.code()).unwrap_or(-1);
    let stderr_text = pumped
        .stderr
        .as_deref()
        .map(String::from_utf8_lossy)
        .unwrap_or_default()
        .into_owned();
    r.log_stream_done(cwd, args, started, code, &stderr_text);

    let io_err = |e: io::Error| {
        CoreError::new(ErrorCode::Io, "could not read git output").with_detail(e.to_string())
    };
    let emitted = pumped.stdout.map_err(io_err)?;
    pumped.stderr.map_err(io_err)?;
    status.map_err(io_err)?;
    Ok(StreamEnd {
        code,
        stderr: stderr_text,
        emitted,
    })
}

/// What [`pump`] read.
struct Pumped {
    /// Whether at least one batch was delivered, or the stdout read error.
    stdout: io::Result<bool>,
    /// All of stderr, or the read error.
    stderr: io::Result<Vec<u8>>,
}

/// Reads log output from `stdout`, delivering batches to `on_batch`, while draining
/// `stderr` concurrently. If reading stdout fails, `on_stdout_error` must stop the process:
/// otherwise a git blocked writing to the unread stdout pipe never closes stderr.
async fn pump<O, E, F>(
    mut stdout: O,
    mut stderr: E,
    on_batch: &mut F,
    on_stdout_error: impl FnOnce(),
) -> Pumped
where
    O: AsyncRead + Unpin,
    E: AsyncRead + Unpin,
    F: FnMut(Vec<Commit>),
{
    let read_stdout = async {
        let mut parser = LogParser::new();
        let mut buf = vec![0u8; READ_CHUNK];
        let mut batch = Vec::new();
        let mut emitted = false;
        loop {
            let n = match stdout.read(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    on_stdout_error();
                    return Err(e);
                }
            };
            if n == 0 {
                break;
            }
            parser.feed(&buf[..n], &mut batch);
            if !batch.is_empty() {
                emitted = true;
                on_batch(std::mem::take(&mut batch));
            }
        }
        parser.finish(&mut batch);
        if !batch.is_empty() {
            emitted = true;
            on_batch(batch);
        }
        Ok(emitted)
    };
    let read_stderr = async {
        let mut err = Vec::new();
        stderr.read_to_end(&mut err).await.map(|_| err)
    };
    let (stdout, stderr) = tokio::join!(read_stdout, read_stderr);
    Pumped { stdout, stderr }
}

/// `Some(true)` when HEAD does not resolve to a commit (no commits on the current branch),
/// `Some(false)` when it does, `None` when git could not tell (for example not a repository).
async fn head_is_unborn(r: &GitRunner, cwd: &Path) -> Result<Option<bool>> {
    let out = r
        .run_unchecked(
            cwd,
            &["rev-parse", "-q", "--verify", "HEAD^{commit}"],
            CmdKind::Read,
        )
        .await?;
    Ok(match out.code {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    })
}

/// Whether the repository has any ref at all (an empty repository has none).
async fn has_any_ref(r: &GitRunner, cwd: &Path) -> Result<bool> {
    let out = r
        .run(
            cwd,
            &["for-each-ref", "--count=1", "--format=%(refname)"],
            CmdKind::Read,
        )
        .await?;
    Ok(!out.stdout.trim_ascii().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: &str = "1111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222";
    const H3: &str = "3333333333333333333333333333333333333333";
    const H4: &str = "4444444444444444444444444444444444444444";

    /// One record exactly as `git log` prints it with `LOG_FORMAT` (tformat adds `\n`).
    fn record(hash: &str, parents: &str, subject: &[u8]) -> Vec<u8> {
        let mut r = format!(
            "{hash}\x1f{parents}\x1fAda Lovelace\x1fada@example.com\x1f1700000000\x1f1700000000\x1f"
        )
        .into_bytes();
        r.extend_from_slice(subject);
        r.extend_from_slice(b"\x1e\n");
        r
    }

    fn commit(hash: &str, parents: &[&str], subject: &str) -> Commit {
        Commit {
            hash: hash.into(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: "Ada Lovelace".into(),
            email: "ada@example.com".into(),
            time: 1_700_000_000,
            commit_time: 1_700_000_000,
            subject: subject.into(),
        }
    }

    fn parse_chunks<'a>(chunks: impl IntoIterator<Item = &'a [u8]>) -> Vec<Commit> {
        let mut p = LogParser::new();
        let mut out = Vec::new();
        for c in chunks {
            p.feed(c, &mut out);
        }
        p.finish(&mut out);
        out
    }

    #[test]
    fn parser_handles_split_records() {
        let mut bytes = record(H1, H2, "first: ünï ✨".as_bytes());
        bytes.extend(record(H2, "", b"second"));
        let expected = vec![
            commit(H1, &[H2], "first: ünï ✨"),
            commit(H2, &[], "second"),
        ];
        assert_eq!(parse_chunks([bytes.as_slice()]), expected);
        for split in 0..=bytes.len() {
            let (a, b) = bytes.split_at(split);
            assert_eq!(parse_chunks([a, b]), expected, "split at {split}");
        }
        // One byte at a time.
        assert_eq!(parse_chunks(bytes.chunks(1)), expected);
    }

    #[test]
    fn parser_emits_records_as_soon_as_they_complete() {
        let first = record(H1, "", b"one");
        let second = record(H2, H1, b"two");
        let mut p = LogParser::new();
        let mut out = Vec::new();
        p.feed(&first, &mut out);
        assert_eq!(out, [commit(H1, &[], "one")]);
        p.feed(&second[..10], &mut out);
        assert_eq!(out.len(), 1);
        p.feed(&second[10..], &mut out);
        assert_eq!(out.len(), 2);
        p.finish(&mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn parser_handles_no_parents_and_many_parents() {
        let mut bytes = record(H1, "", b"root");
        bytes.extend(record(H4, &format!("{H1} {H2} {H3}"), b"octopus"));
        assert_eq!(
            parse_chunks([bytes.as_slice()]),
            [
                commit(H1, &[], "root"),
                commit(H4, &[H1, H2, H3], "octopus")
            ]
        );
    }

    #[test]
    fn parser_is_lossy() {
        let bytes = record(H1, "", b"bad \xFF\xFE bytes");
        let out = parse_chunks([bytes.as_slice()]);
        assert_eq!(out.len(), 1);
        assert!(out[0].subject.contains('\u{FFFD}'), "{:?}", out[0].subject);
        assert!(out[0].subject.starts_with("bad ") && out[0].subject.ends_with(" bytes"));
    }

    #[test]
    fn parser_handles_5000_char_subject() {
        let subject = "x".repeat(5000);
        let bytes = record(H1, "", subject.as_bytes());
        for chunk in [7, 100, 4096, bytes.len()] {
            let out = parse_chunks(bytes.chunks(chunk));
            assert_eq!(out, [commit(H1, &[], &subject)], "chunk size {chunk}");
        }
    }

    #[test]
    fn parser_keeps_unit_separators_inside_the_subject() {
        let bytes = record(H1, "", b"a\x1fb");
        assert_eq!(parse_chunks([bytes.as_slice()])[0].subject, "a\x1fb");
    }

    #[test]
    fn parser_survives_hostile_input() {
        assert!(parse_chunks([b"".as_slice()]).is_empty());
        assert!(parse_chunks([b"\n\n".as_slice()]).is_empty());
        assert!(parse_chunks([b"\x1e\x1e\n\x1e".as_slice()]).is_empty());
        assert!(parse_chunks([b"\x1f\x1f\x1f\x1f\x1f\x1e".as_slice()]).is_empty());
        assert!(parse_chunks([b"\xFF\xFE\x00garbage".as_slice()]).is_empty());
        // Hash that is not hex, or too few fields.
        assert!(parse_chunks([b"zz\x1f\x1fa\x1fe\x1f1\x1fs\x1e".as_slice()]).is_empty());
        assert!(parse_chunks([format!("{H1}\x1f\x1fa\x1e").as_bytes()]).is_empty());
        // A bad record between good ones only drops itself.
        let mut bytes = record(H1, "", b"one");
        bytes.extend_from_slice(b"junk\x1e\n");
        bytes.extend(record(H2, "", b"two"));
        assert_eq!(
            parse_chunks([bytes.as_slice()]),
            [commit(H1, &[], "one"), commit(H2, &[], "two")]
        );
        // Unparseable times become 0 rather than dropping the commit.
        let bytes = format!("{H1}\x1f\x1fa\x1fe\x1fnot-a-number\x1fnor-this\x1fs\x1e");
        let parsed = &parse_chunks([bytes.as_bytes()])[0];
        assert_eq!((parsed.time, parsed.commit_time), (0, 0));
    }

    #[test]
    fn parser_finish_drops_truncated_record() {
        let mut bytes = record(H1, "", b"complete");
        let second = record(H2, "", b"cut off");
        bytes.extend_from_slice(&second[..second.len() / 2]);
        assert_eq!(
            parse_chunks([bytes.as_slice()]),
            [commit(H1, &[], "complete")]
        );
        // Without its terminator even a record with every field may have a cut-off subject.
        let unterminated =
            format!("{H1}\x1f\x1fAda Lovelace\x1fada@example.com\x1f1700000000\x1flast");
        assert!(parse_chunks([unterminated.as_bytes()]).is_empty());
    }

    /// A reader whose every read fails.
    struct FailingReader;

    impl tokio::io::AsyncRead for FailingReader {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Err(io::Error::other("broken pipe")))
        }
    }

    #[tokio::test]
    async fn pump_stops_the_process_when_stdout_fails() {
        // stderr stays open until the process is stopped, as with a git blocked on a full
        // stdout pipe. Unless the stdout error triggers the stop, the drain never ends.
        let (stderr_writer, stderr) = tokio::io::duplex(64);
        let mut batches = 0;
        let pumped = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            pump(FailingReader, stderr, &mut |_| batches += 1, move || {
                drop(stderr_writer)
            }),
        )
        .await
        .expect("a stdout error must stop the process, not hang on stderr");
        assert!(pumped.stdout.is_err());
        assert!(pumped.stderr.is_ok());
        assert_eq!(batches, 0);
    }

    #[tokio::test]
    async fn pump_batches_stdout_and_collects_stderr() {
        let bytes = [record(H1, "", b"one"), record(H2, H1, b"two")].concat();
        let mut got = Vec::new();
        let pumped = pump(
            bytes.as_slice(),
            b"warning: x\n".as_slice(),
            &mut |b| got.extend(b),
            || panic!("no error, nothing to stop"),
        )
        .await;
        assert!(pumped.stdout.unwrap(), "emitted");
        assert_eq!(pumped.stderr.unwrap(), b"warning: x\n");
        assert_eq!(got, [commit(H1, &[], "one"), commit(H2, &[H1], "two")]);
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn log_args_all() {
        let opts = ViewOptions {
            branches: None,
            show_remotes: true,
            show_tags: true,
            show_stashes: true,
        };
        assert_eq!(
            log_args(&opts, &[]),
            strings(&[
                "log",
                LOG_FORMAT,
                "--date-order",
                "--branches",
                "--remotes",
                "--tags",
                "HEAD",
                "--"
            ])
        );
    }

    #[test]
    fn log_args_no_remotes_no_tags() {
        let opts = ViewOptions {
            branches: None,
            show_remotes: false,
            show_tags: false,
            show_stashes: true,
        };
        assert_eq!(
            log_args(&opts, &strings(&["origin/main"])),
            strings(&[
                "log",
                LOG_FORMAT,
                "--date-order",
                "--branches",
                "HEAD",
                "--"
            ])
        );
    }

    #[test]
    fn log_args_selected() {
        let opts = ViewOptions {
            branches: Some(strings(&["main", "origin/dev"])),
            show_remotes: true,
            show_tags: true,
            show_stashes: true,
        };
        assert_eq!(
            log_args(&opts, &strings(&["origin/dev"])),
            strings(&[
                "log",
                LOG_FORMAT,
                "--date-order",
                "refs/heads/main",
                "refs/remotes/origin/dev",
                "--"
            ])
        );
    }
}
