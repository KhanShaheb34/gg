//! The one line gg adds to the end of the user's shell start-up file, only when they ask (a
//! box ticked in the install dialog, a button in Settings), to turn off a `gg` alias or
//! shell function that would win over the `gg` command; and takes out again when they remove
//! the command in gg, or ask in Settings.
//!
//! The line ends with a comment ([`NOTE`]) that marks it as gg's, and is recognised only
//! whole: a line that has been changed is the user's and is left alone (gg says so rather
//! than add another). Only zsh and bash, and only an alias or a function: another program
//! called `gg` earlier on the `PATH` is not something a line here should hide.
//!
//! The file is never rewritten to add the line: it is opened for appending, so what is in it
//! cannot be lost, through a symbolic link (a dotfiles repository's) to the file it leads
//! to, which keeps its permissions. Taking the line out of the end of the file shortens it
//! in place; taking it out of the middle (lines added after it since) writes the whole file
//! beside the real one, with its permissions, and renames it over it, so it is whole or as
//! it was. Every byte is kept as it is: the file need not be UTF-8, and its line ends (CRLF)
//! are followed. A file that ended without a line break gets one before the line, and the
//! line says so ([`NOTE_NO_EOL`]): taking it out takes that line break out too.

use std::fs;
use std::io::{self, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode, Result};

/// The login shells whose start-up file gg can add the line to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
}

impl Shell {
    /// The shell `program` is, by its file name (`/bin/zsh`, `/opt/homebrew/bin/bash`);
    /// `None` for any other (fish, sh, …).
    pub fn of(program: &Path) -> Option<Shell> {
        match program.file_name()?.to_str()? {
            "zsh" => Some(Shell::Zsh),
            "bash" => Some(Shell::Bash),
            _ => None,
        }
    }
}

/// What the shell's own `gg` is, among those a line can turn off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Conflict {
    Alias,
    Function,
}

impl Conflict {
    /// The command that turns it off, the same in zsh and bash (zsh's `unset -f` is its
    /// `unfunction`); quiet when there is nothing to turn off.
    pub fn command(self) -> &'static str {
        match self {
            Conflict::Alias => "unalias gg 2>/dev/null",
            Conflict::Function => "unset -f gg 2>/dev/null",
        }
    }
}

/// The comment that marks the line as gg's.
pub const NOTE: &str =
    "# added by gg: lets the gg command run; gg Settings > Command line takes it out";

/// The same, on a line that followed a file without a final line break: gg added that line
/// break too, and takes it out with the line.
pub const NOTE_NO_EOL: &str = "# added by gg: lets the gg command run; gg Settings > Command line takes it out (and the line break before it)";

/// What every marked line holds, changed or not: how one the user changed is still known.
const MARK: &[u8] = b"# added by gg: lets the gg command run";

/// The line, as written, for `conflict` (without its line end).
pub fn line(conflict: Conflict, no_eol: bool) -> String {
    let note = if no_eol { NOTE_NO_EOL } else { NOTE };
    format!("{}  {note}", conflict.command())
}

/// The start-up file the line goes at the end of, for `shell` of a user whose home folder is
/// `home`; `None` when there is none to add it to.
///
/// zsh: `.zshrc` in `ZDOTDIR` (as the shell itself says it, see `zdotdir`), else in the
/// home folder. Every interactive zsh reads it, after `.zshenv` and `.zprofile`, which is
/// where an alias such as Oh My Zsh's is made. It may not exist yet; it is then made (only
/// when the user asks, and the shell has just said it has a `gg`), in a folder that exists.
///
/// bash: on macOS, terminals start login shells (Terminal, iTerm), which read the first of
/// `~/.bash_profile`, `~/.bash_login` and `~/.profile` that exists, and `~/.bashrc` only if
/// that one sources it: so the line goes at the end of that first file, after anything it
/// sources. On Linux, terminals start interactive shells that are not login shells, which
/// read `~/.bashrc` only (a login shell's `~/.profile` or `~/.bash_profile` usually sources
/// it): so the line goes at the end of `~/.bashrc`. The file must exist: a new
/// `~/.bash_profile` would stop bash reading the `~/.profile` the user has, and bash's
/// aliases are made in a file that exists, so gg never makes one (`None`).
/// `macos` says which.
pub fn rc_file(shell: Shell, home: &Path, zdotdir: Option<&Path>, macos: bool) -> Option<PathBuf> {
    match shell {
        Shell::Zsh => {
            let dir = match zdotdir {
                Some(z) if z.is_absolute() => z,
                Some(_) => return None,
                None => home,
            };
            Some(dir.join(".zshrc"))
        }
        Shell::Bash if macos => [".bash_profile", ".bash_login", ".profile"]
            .iter()
            .map(|name| home.join(name))
            .find(|f| f.exists()),
        Shell::Bash => Some(home.join(".bashrc")).filter(|f| f.exists()),
    }
}

/// Every start-up file a line of gg's may be in, whatever the shell is now (the user may have
/// changed it, or `ZDOTDIR`, since): looked through to show the line, and to take it out.
pub fn rc_files(home: &Path, zdotdir: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Some(z) = zdotdir.filter(|z| z.is_absolute()) {
        files.push(z.join(".zshrc"));
    }
    for name in [
        ".zshrc",
        ".bash_profile",
        ".bash_login",
        ".profile",
        ".bashrc",
    ] {
        let file = home.join(name);
        if !files.contains(&file) {
            files.push(file);
        }
    }
    files
}

/// One line of a file: where it starts, where its text ends (before `\r\n` or `\n`), and
/// where the next one starts.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    text_end: usize,
    end: usize,
}

fn spans(content: &[u8]) -> Vec<Span> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < content.len() {
        let (text_end, end) = match content[start..].iter().position(|&b| b == b'\n') {
            Some(i) => {
                let nl = start + i;
                let text_end = if nl > start && content[nl - 1] == b'\r' {
                    nl - 1
                } else {
                    nl
                };
                (text_end, nl + 1)
            }
            None => (content.len(), content.len()),
        };
        out.push(Span {
            start,
            text_end,
            end,
        });
        start = end;
    }
    out
}

/// What a line of the file is to gg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A line gg wrote, as it wrote it: for `Conflict`, and whether it followed a file
    /// without a final line break.
    Ours(Conflict, bool),
    /// A line with gg's mark that is not one gg wrote (changed since).
    Changed,
    Other,
}

fn kind(text: &[u8]) -> Kind {
    for conflict in [Conflict::Alias, Conflict::Function] {
        for no_eol in [false, true] {
            if text == line(conflict, no_eol).as_bytes() {
                return Kind::Ours(conflict, no_eol);
            }
        }
    }
    if text.windows(MARK.len()).any(|w| w == MARK) {
        Kind::Changed
    } else {
        Kind::Other
    }
}

/// Whether `content` holds a line gg wrote (for any conflict).
pub fn holds_line(content: &[u8]) -> bool {
    spans(content)
        .iter()
        .any(|s| matches!(kind(&content[s.start..s.text_end]), Kind::Ours(..)))
}

/// Whether file `file` holds a line gg wrote. A file that cannot be read holds none.
pub fn file_holds_line(file: &Path) -> bool {
    fs::read(file).is_ok_and(|c| holds_line(&c))
}

/// Whether file `file` holds gg's line for `conflict`. A file that cannot be read holds none.
pub fn file_holds_line_for(file: &Path, conflict: Conflict) -> bool {
    fs::read(file).is_ok_and(|c| holds_line_for(&c, conflict))
}

/// Whether `content` holds gg's line for `conflict`.
fn holds_line_for(content: &[u8], conflict: Conflict) -> bool {
    spans(content)
        .iter()
        .any(|s| matches!(kind(&content[s.start..s.text_end]), Kind::Ours(c, _) if c == conflict))
}

/// Whether `content` holds a line with gg's mark that is not one gg wrote.
fn holds_changed_line(content: &[u8]) -> bool {
    spans(content)
        .iter()
        .any(|s| kind(&content[s.start..s.text_end]) == Kind::Changed)
}

/// What [`add`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    /// The line is at the end of the file now.
    Now,
    /// It was in the file already; nothing was written.
    Already,
}

/// `path` as a user reads it: `~/…` under `home`.
pub fn shown(path: &Path, home: Option<&Path>) -> String {
    if let Some(rest) = home.and_then(|h| path.strip_prefix(h).ok()) {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

fn io_error(message: String, e: &io::Error) -> CoreError {
    CoreError::new(ErrorCode::Io, message).with_detail(e.to_string())
}

/// Adds gg's line for `conflict` to the end of `file` (see the module's notes), unless it is
/// there already. A missing file is made only when `create` says so (zsh's `.zshrc`), never
/// in a folder that is missing. Refused, with nothing written, when the file is not a plain
/// file (or a link to one), holds a line with gg's mark that was changed, cannot be read or
/// written, or changed while gg was adding the line. `home` is only for the messages.
pub fn add(file: &Path, conflict: Conflict, create: bool, home: Option<&Path>) -> Result<Added> {
    let name = shown(file, home);
    let could_not = |e: &io::Error| io_error(format!("Could not add a line to {name}"), e);
    let refused = |why: String| CoreError::new(ErrorCode::BadRequest, why);
    let line_no_eol = |no_eol: bool| line(conflict, no_eol);

    match fs::symlink_metadata(file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !create {
                return Err(refused(format!(
                    "{name} does not exist; gg makes no new file for bash."
                )));
            }
            // Made new, never through a link someone put there meanwhile.
            let mut made = crate::store::create_new_file(file, 0o644).map_err(|e| could_not(&e))?;
            let text = format!("{}\n", line_no_eol(false));
            let written = made
                .write_all(text.as_bytes())
                .and_then(|()| made.sync_all());
            if let Err(e) = written {
                drop(made);
                let _ = fs::remove_file(file);
                return Err(could_not(&e));
            }
            return Ok(Added::Now);
        }
        Err(e) => return Err(could_not(&e)),
        Ok(_) => {}
    }
    // Through a link to the file it leads to; it must be a plain file.
    let meta = fs::metadata(file).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            refused(format!(
                "{name} is a link to a file that does not exist; gg leaves it alone."
            ))
        } else {
            could_not(&e)
        }
    })?;
    if !meta.is_file() {
        return Err(refused(format!(
            "{name} is not a file; gg leaves it alone."
        )));
    }
    // Opened for appending only: nothing in the file can be overwritten or cut.
    let mut handle = fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(file)
        .map_err(|e| could_not(&e))?;
    let mut content = Vec::new();
    handle
        .read_to_end(&mut content)
        .map_err(|e| could_not(&e))?;
    if holds_line_for(&content, conflict) {
        return Ok(Added::Already);
    }
    if holds_changed_line(&content) {
        return Err(refused(format!(
            "{name} has a line marked as added by gg that has been changed since; gg leaves it alone and adds no other. Take it out or put it back as it was, then try again."
        )));
    }
    // The file's own line ends: CRLF when its last line break is one.
    let crlf = content
        .iter()
        .rposition(|&b| b == b'\n')
        .is_some_and(|i| i > 0 && content[i - 1] == b'\r');
    let eol = if crlf { "\r\n" } else { "\n" };
    let text = if content.is_empty() || content.ends_with(b"\n") {
        format!("{}{eol}", line_no_eol(false))
    } else {
        format!("{eol}{}{eol}", line_no_eol(true))
    };
    let before = content.len() as u64;
    let appended = (|| {
        // Nothing was added since it was read (an append would then land after it).
        if handle.metadata()?.len() != before {
            return Err(io::Error::other(
                "the file changed while gg was adding the line",
            ));
        }
        handle.write_all(text.as_bytes())?;
        handle.sync_all()
    })();
    if let Err(e) = appended {
        // Not half a line: back to the length it had, which only takes off what was added.
        if handle.metadata().is_ok_and(|m| m.len() > before) {
            let _ = handle.set_len(before);
        }
        return Err(could_not(&e));
    }
    Ok(Added::Now)
}

/// What [`remove`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Removed {
    /// gg's lines taken out (none when there were none).
    pub lines: usize,
    /// A line with gg's mark that was changed since is still there, left alone.
    pub changed_left: bool,
}

/// Takes gg's lines out of `file` (see the module's notes); every other byte stays as it is.
/// A missing file holds none. Refused, with the file as it was, when it cannot be read or
/// written (a read-only file stays as it is, even where its folder would let it be
/// replaced), or changed while gg was taking the line out.
pub fn remove(file: &Path, home: Option<&Path>) -> Result<Removed> {
    let name = shown(file, home);
    let could_not = |e: &io::Error| io_error(format!("Could not take gg's line out of {name}"), e);
    let content = match fs::read(file) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Removed::default()),
        Err(e) => return Err(could_not(&e)),
    };
    let spans = spans(&content);
    let mut cut = vec![false; content.len()];
    let mut removed = Removed::default();
    // From the end: whether every line after this one is gg's (and goes too).
    let mut only_ours_after = true;
    for (i, span) in spans.iter().enumerate().rev() {
        match kind(&content[span.start..span.text_end]) {
            Kind::Ours(_, no_eol) => {
                removed.lines += 1;
                cut[span.start..span.end].fill(true);
                // The line break gg added before it, when nothing came after it since.
                if no_eol && only_ours_after && i > 0 {
                    let prev = spans[i - 1];
                    cut[prev.text_end..prev.end].fill(true);
                }
            }
            Kind::Changed => {
                removed.changed_left = true;
                only_ours_after = false;
            }
            Kind::Other => only_ours_after = false,
        }
    }
    if removed.lines == 0 {
        return Ok(removed);
    }
    let kept: Vec<u8> = content
        .iter()
        .zip(&cut)
        .filter(|(_, cut)| !**cut)
        .map(|(b, _)| *b)
        .collect();
    // Opened for writing first, as a check that the user lets it be written: a read-only file
    // is never replaced, even where its folder would allow it.
    let mut handle = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(file)
        .map_err(|e| could_not(&e))?;
    let unchanged = |handle: &mut fs::File| -> io::Result<()> {
        let mut now = Vec::new();
        handle.rewind()?;
        handle.read_to_end(&mut now)?;
        if now != content {
            return Err(io::Error::other(
                "the file changed while gg was taking the line out",
            ));
        }
        Ok(())
    };
    if content.starts_with(&kept) {
        // Only the end goes: the file is shortened in place, nothing else is written.
        unchanged(&mut handle)
            .and_then(|()| handle.set_len(kept.len() as u64))
            .and_then(|()| handle.sync_all())
            .map_err(|e| could_not(&e))?;
        return Ok(removed);
    }
    replace(file, &kept, &mut handle, unchanged).map_err(|e| could_not(&e))?;
    Ok(removed)
}

/// Writes `content` beside the file `file` leads to (through any links) with its
/// permissions, and renames it over that file once `unchanged` says it still holds what was
/// read: whole or as it was. The link, if any, stays.
fn replace(
    file: &Path,
    content: &[u8],
    handle: &mut fs::File,
    unchanged: impl Fn(&mut fs::File) -> io::Result<()>,
) -> io::Result<()> {
    let real = dunce::canonicalize(file)?;
    let dir = real.parent().ok_or_else(|| io::Error::other("no folder"))?;
    let base = real
        .file_name()
        .ok_or_else(|| io::Error::other("no name"))?;
    let tmp = dir.join(format!(
        ".{}.gg-{}.tmp",
        base.to_string_lossy(),
        std::process::id()
    ));
    let meta = handle.metadata()?;
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let mode = {
        let _ = &meta;
        0o644
    };
    // A stale one of ours (a link planted there is removed, never written through).
    if fs::symlink_metadata(&tmp).is_ok_and(|m| !m.is_dir()) {
        fs::remove_file(&tmp)?;
    }
    let mut made = crate::store::create_new_file(&tmp, mode)?;
    let result = (|| {
        made.write_all(content)?;
        made.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The umask may have taken bits off at creation.
            fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
        }
        unchanged(handle)?;
        fs::rename(&tmp, &real)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const OMZ: &[u8] =
        b"export ZSH=\"$HOME/.oh-my-zsh\"\nplugins=(git)\nsource $ZSH/oh-my-zsh.sh\n";

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    fn set_mode(path: &Path, mode: u32) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Adds the alias line to `before` and takes it out: the file is `after_add`, then
    /// `before` again, byte for byte.
    fn round_trip(before: &[u8], after_add: &[u8]) {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, before).unwrap();
        assert_eq!(add(&rc, Conflict::Alias, false, None).unwrap(), Added::Now);
        assert_eq!(fs::read(&rc).unwrap(), after_add);
        assert!(file_holds_line(&rc));
        let removed = remove(&rc, None).unwrap();
        assert_eq!(
            removed,
            Removed {
                lines: 1,
                changed_left: false
            }
        );
        assert_eq!(fs::read(&rc).unwrap(), before);
        assert!(!file_holds_line(&rc));
    }

    #[test]
    fn the_line_says_what_it_does_and_whose_it_is() {
        assert_eq!(
            line(Conflict::Alias, false),
            "unalias gg 2>/dev/null  # added by gg: lets the gg command run; gg Settings > Command line takes it out"
        );
        assert_eq!(
            line(Conflict::Function, false),
            "unset -f gg 2>/dev/null  # added by gg: lets the gg command run; gg Settings > Command line takes it out"
        );
        assert!(line(Conflict::Alias, true).ends_with("(and the line break before it)"));
        assert!(NOTE.is_ascii() && NOTE_NO_EOL.is_ascii());
    }

    #[test]
    fn the_line_goes_at_the_end_and_comes_out_exactly() {
        let ours = line(Conflict::Alias, false);
        let mut after = OMZ.to_vec();
        after.extend_from_slice(format!("{ours}\n").as_bytes());
        round_trip(OMZ, &after);
    }

    #[test]
    fn a_file_without_a_final_line_break_gets_it_back_without_one() {
        let before = b"alias gg='git gui citool'";
        let after = format!(
            "alias gg='git gui citool'\n{}\n",
            line(Conflict::Alias, true)
        );
        round_trip(before, after.as_bytes());
    }

    #[test]
    fn crlf_line_ends_are_followed() {
        let before = b"alias gg='git gui citool'\r\nexport A=1\r\n";
        let after = format!(
            "alias gg='git gui citool'\r\nexport A=1\r\n{}\r\n",
            line(Conflict::Alias, false)
        );
        round_trip(before, after.as_bytes());
        // And without a final one.
        let before = b"alias gg=x\r\nexport A=1";
        let after = format!(
            "alias gg=x\r\nexport A=1\r\n{}\r\n",
            line(Conflict::Alias, true)
        );
        round_trip(before, after.as_bytes());
    }

    #[test]
    fn bytes_that_are_not_utf8_survive_untouched() {
        let before: &[u8] = b"# caf\xe9 \xff\xfe latin-1\nalias gg='git gui citool'\n\x80\x81\n";
        let mut after = before.to_vec();
        after.extend_from_slice(format!("{}\n", line(Conflict::Alias, false)).as_bytes());
        round_trip(before, &after);
    }

    #[test]
    fn an_empty_file_gets_the_line_and_is_empty_again() {
        round_trip(
            b"",
            format!("{}\n", line(Conflict::Alias, false)).as_bytes(),
        );
    }

    #[test]
    fn the_line_is_never_added_twice() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, OMZ).unwrap();
        assert_eq!(add(&rc, Conflict::Alias, false, None).unwrap(), Added::Now);
        let once = fs::read(&rc).unwrap();
        assert_eq!(
            add(&rc, Conflict::Alias, false, None).unwrap(),
            Added::Already
        );
        assert_eq!(fs::read(&rc).unwrap(), once);
        // A function's line is another line; both come out.
        assert_eq!(
            add(&rc, Conflict::Function, false, None).unwrap(),
            Added::Now
        );
        assert_eq!(remove(&rc, None).unwrap().lines, 2);
        assert_eq!(fs::read(&rc).unwrap(), OMZ);
    }

    /// Both lines after a file without a final line break: both go, and the line break gg
    /// added before the first.
    #[test]
    fn two_lines_after_a_file_without_a_final_line_break() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".bashrc");
        fs::write(&rc, b"alias gg=x").unwrap();
        add(&rc, Conflict::Alias, false, None).unwrap();
        add(&rc, Conflict::Function, false, None).unwrap();
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            format!(
                "alias gg=x\n{}\n{}\n",
                line(Conflict::Alias, true),
                line(Conflict::Function, false)
            )
        );
        assert_eq!(remove(&rc, None).unwrap().lines, 2);
        assert_eq!(fs::read(&rc).unwrap(), b"alias gg=x");
    }

    #[test]
    fn a_line_deleted_by_hand_is_added_again_and_nothing_is_taken_out() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, OMZ).unwrap();
        add(&rc, Conflict::Alias, false, None).unwrap();
        fs::write(&rc, OMZ).unwrap();
        assert_eq!(remove(&rc, None).unwrap(), Removed::default());
        assert_eq!(fs::read(&rc).unwrap(), OMZ);
        assert_eq!(add(&rc, Conflict::Alias, false, None).unwrap(), Added::Now);
        assert!(file_holds_line(&rc));
    }

    #[test]
    fn a_line_changed_by_hand_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        let changed = format!(
            "{}# {}\n",
            String::from_utf8_lossy(OMZ),
            line(Conflict::Alias, false)
        );
        fs::write(&rc, &changed).unwrap();
        assert!(!file_holds_line(&rc));
        // Not taken out, and no other added.
        assert_eq!(
            remove(&rc, None).unwrap(),
            Removed {
                lines: 0,
                changed_left: true
            }
        );
        let err = add(&rc, Conflict::Alias, false, None).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(
            err.message.contains("has been changed since"),
            "{}",
            err.message
        );
        assert_eq!(fs::read_to_string(&rc).unwrap(), changed);
        // Next to one of gg's own: that one goes, the changed one stays.
        let both = format!("{changed}{}\n", line(Conflict::Alias, false));
        fs::write(&rc, &both).unwrap();
        assert_eq!(
            remove(&rc, None).unwrap(),
            Removed {
                lines: 1,
                changed_left: true
            }
        );
        assert_eq!(fs::read_to_string(&rc).unwrap(), changed);
    }

    #[test]
    fn lines_added_after_it_stay_when_it_comes_out() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, OMZ).unwrap();
        set_mode(&rc, 0o600);
        add(&rc, Conflict::Alias, false, None).unwrap();
        let mut more = fs::OpenOptions::new().append(true).open(&rc).unwrap();
        more.write_all(b"export PATH=\"$HOME/.cargo/bin:$PATH\"\n")
            .unwrap();
        drop(more);
        assert_eq!(remove(&rc, None).unwrap().lines, 1);
        let mut expected = OMZ.to_vec();
        expected.extend_from_slice(b"export PATH=\"$HOME/.cargo/bin:$PATH\"\n");
        assert_eq!(fs::read(&rc).unwrap(), expected);
        assert_eq!(mode(&rc), 0o600, "its permissions kept");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [".zshrc"], "no temp file left");
        // After a file without a final line break: only the line goes, the line break gg
        // added stays (something follows it now).
        fs::write(&rc, b"a=1").unwrap();
        add(&rc, Conflict::Alias, false, None).unwrap();
        let mut more = fs::OpenOptions::new().append(true).open(&rc).unwrap();
        more.write_all(b"b=2\n").unwrap();
        drop(more);
        remove(&rc, None).unwrap();
        assert_eq!(fs::read(&rc).unwrap(), b"a=1\nb=2\n");
    }

    /// A dotfiles repository's link: the line goes into the file it leads to, the link stays
    /// a link to it, and the file keeps its permissions, both ways.
    #[test]
    fn a_linked_file_is_written_through_and_the_link_stays() {
        let root = tempfile::tempdir().unwrap();
        let dotfiles = root.path().join("dotfiles");
        let home = root.path().join("home");
        fs::create_dir_all(&dotfiles).unwrap();
        fs::create_dir_all(&home).unwrap();
        let real = dotfiles.join("zshrc");
        fs::write(&real, OMZ).unwrap();
        set_mode(&real, 0o640);
        let rc = home.join(".zshrc");
        std::os::unix::fs::symlink(&real, &rc).unwrap();
        let is_link = |p: &Path| fs::symlink_metadata(p).unwrap().file_type().is_symlink();

        add(&rc, Conflict::Alias, false, Some(&home)).unwrap();
        assert!(is_link(&rc));
        assert!(file_holds_line(&real));
        assert_eq!(mode(&real), 0o640);
        remove(&rc, Some(&home)).unwrap();
        assert!(is_link(&rc));
        assert_eq!(fs::read_link(&rc).unwrap(), real);
        assert_eq!(fs::read(&real).unwrap(), OMZ);

        // Out of the middle too: replaced beside the real file, the link kept.
        add(&rc, Conflict::Alias, false, Some(&home)).unwrap();
        let mut more = fs::OpenOptions::new().append(true).open(&real).unwrap();
        more.write_all(b"x=1\n").unwrap();
        drop(more);
        remove(&rc, Some(&home)).unwrap();
        assert!(is_link(&rc));
        assert_eq!(fs::read_link(&rc).unwrap(), real);
        assert_eq!(mode(&real), 0o640);
        let mut expected = OMZ.to_vec();
        expected.extend_from_slice(b"x=1\n");
        assert_eq!(fs::read(&real).unwrap(), expected);
        assert!(
            fs::read_dir(&home).unwrap().count() == 1,
            "nothing left beside the link"
        );

        // A link that leads nowhere is left alone.
        fs::remove_file(&real).unwrap();
        let err = add(&rc, Conflict::Alias, true, Some(&home)).unwrap_err();
        assert!(
            err.message.contains("a link to a file that does not exist"),
            "{}",
            err.message
        );
        assert!(!real.exists());
        assert!(is_link(&rc));
    }

    #[test]
    fn a_missing_file_is_made_only_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        let err = add(&rc, Conflict::Alias, false, None).unwrap_err();
        assert!(err.message.contains("does not exist"), "{}", err.message);
        assert!(!rc.exists());
        assert_eq!(add(&rc, Conflict::Alias, true, None).unwrap(), Added::Now);
        assert_eq!(
            fs::read_to_string(&rc).unwrap(),
            format!("{}\n", line(Conflict::Alias, false))
        );
        // Taking it out leaves the file, empty: gg does not delete a start-up file.
        remove(&rc, None).unwrap();
        assert_eq!(fs::read(&rc).unwrap(), b"");
        // Never in a folder that is missing.
        let nowhere = dir.path().join("zdotdir/.zshrc");
        assert!(add(&nowhere, Conflict::Alias, true, None).is_err());
        assert!(!dir.path().join("zdotdir").exists());
        // Nothing to take out of a file that is not there.
        assert_eq!(remove(&nowhere, None).unwrap(), Removed::default());
    }

    #[test]
    fn a_folder_in_its_place_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::create_dir(&rc).unwrap();
        let err = add(&rc, Conflict::Alias, true, None).unwrap_err();
        assert!(err.message.contains("is not a file"), "{}", err.message);
    }

    /// Skipped as root, which may write anything.
    fn is_root() -> bool {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn a_read_only_file_is_left_as_it_was() {
        if is_root() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, OMZ).unwrap();
        set_mode(&rc, 0o444);
        let err = add(&rc, Conflict::Alias, false, Some(dir.path())).unwrap_err();
        assert_eq!(err.code, ErrorCode::Io);
        assert_eq!(err.message, "Could not add a line to ~/.zshrc");
        assert!(err.detail.is_some());
        assert_eq!(fs::read(&rc).unwrap(), OMZ);
        assert_eq!(mode(&rc), 0o444);

        // With the line in it, the middle of it (its folder is writable, which would let it
        // be replaced): still left as it is.
        set_mode(&rc, 0o644);
        add(&rc, Conflict::Alias, false, None).unwrap();
        let mut more = fs::OpenOptions::new().append(true).open(&rc).unwrap();
        more.write_all(b"x=1\n").unwrap();
        drop(more);
        let with_line = fs::read(&rc).unwrap();
        set_mode(&rc, 0o444);
        let err = remove(&rc, Some(dir.path())).unwrap_err();
        assert_eq!(err.message, "Could not take gg's line out of ~/.zshrc");
        assert_eq!(fs::read(&rc).unwrap(), with_line);
        assert_eq!(mode(&rc), 0o444);
    }

    #[test]
    fn a_read_only_folder() {
        if is_root() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("home");
        fs::create_dir(&dir).unwrap();
        // No .zshrc, and the folder cannot take one: nothing is made.
        set_mode(&dir, 0o555);
        let rc = dir.join(".zshrc");
        assert_eq!(
            add(&rc, Conflict::Alias, true, None).unwrap_err().code,
            ErrorCode::Io
        );
        assert!(!rc.exists());
        // A writable file in it: the line is appended (nothing new is made in the folder),
        // and taken out of the end in place.
        set_mode(&dir, 0o755);
        fs::write(&rc, OMZ).unwrap();
        set_mode(&dir, 0o555);
        assert_eq!(add(&rc, Conflict::Alias, false, None).unwrap(), Added::Now);
        assert_eq!(remove(&rc, None).unwrap().lines, 1);
        assert_eq!(fs::read(&rc).unwrap(), OMZ);
        // Out of the middle it needs a file beside it: refused, the file as it was.
        add(&rc, Conflict::Alias, false, None).unwrap();
        let mut more = fs::OpenOptions::new().append(true).open(&rc).unwrap();
        more.write_all(b"x=1\n").unwrap();
        drop(more);
        let with_line = fs::read(&rc).unwrap();
        assert_eq!(remove(&rc, None).unwrap_err().code, ErrorCode::Io);
        assert_eq!(fs::read(&rc).unwrap(), with_line);
        set_mode(&dir, 0o755);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [".zshrc"]);
    }

    #[test]
    fn the_shell_is_known_by_its_name() {
        assert_eq!(Shell::of(Path::new("/bin/zsh")), Some(Shell::Zsh));
        assert_eq!(
            Shell::of(Path::new("/opt/homebrew/bin/bash")),
            Some(Shell::Bash)
        );
        assert_eq!(Shell::of(Path::new("/opt/homebrew/bin/fish")), None);
        assert_eq!(Shell::of(Path::new("/bin/sh")), None);
        assert_eq!(Shell::of(Path::new("/bin/zsh-5.9")), None);
    }

    #[test]
    fn the_start_up_file_is_the_one_the_shell_reads_last() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path();
        // zsh: .zshrc in ZDOTDIR, else at home; whether or not it exists yet.
        assert_eq!(
            rc_file(Shell::Zsh, home, None, true),
            Some(home.join(".zshrc"))
        );
        let zdot = home.join(".config/zsh");
        assert_eq!(
            rc_file(Shell::Zsh, home, Some(&zdot), false),
            Some(zdot.join(".zshrc"))
        );
        assert_eq!(
            rc_file(Shell::Zsh, home, Some(Path::new("rel")), true),
            None
        );
        // bash: none that exists, none.
        assert_eq!(rc_file(Shell::Bash, home, None, true), None);
        assert_eq!(rc_file(Shell::Bash, home, None, false), None);
        // macOS: the first login file bash reads; Linux: .bashrc.
        fs::write(home.join(".profile"), "").unwrap();
        fs::write(home.join(".bashrc"), "").unwrap();
        assert_eq!(
            rc_file(Shell::Bash, home, None, true),
            Some(home.join(".profile"))
        );
        fs::write(home.join(".bash_login"), "").unwrap();
        assert_eq!(
            rc_file(Shell::Bash, home, None, true),
            Some(home.join(".bash_login"))
        );
        fs::write(home.join(".bash_profile"), "").unwrap();
        assert_eq!(
            rc_file(Shell::Bash, home, None, true),
            Some(home.join(".bash_profile"))
        );
        assert_eq!(
            rc_file(Shell::Bash, home, None, false),
            Some(home.join(".bashrc"))
        );
        // All of them are looked through for the line, ZDOTDIR's first.
        assert_eq!(
            rc_files(home, Some(&zdot)),
            [
                zdot.join(".zshrc"),
                home.join(".zshrc"),
                home.join(".bash_profile"),
                home.join(".bash_login"),
                home.join(".profile"),
                home.join(".bashrc"),
            ]
        );
    }

    /// What the line does, in a POSIX shell (zsh and bash read it the same way): the alias, or
    /// the function, made before it is gone after it; the comment, and a CR before the line
    /// break, do nothing.
    #[test]
    fn the_line_turns_off_the_shells_gg() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join("rc");
        for (setup, conflict) in [
            ("alias gg='echo alias'\n", Conflict::Alias),
            ("x=1\r\nalias gg='echo alias'\r\n", Conflict::Alias),
            ("gg() { echo function; }\n", Conflict::Function),
            ("gg() { echo function; }", Conflict::Function),
        ] {
            fs::write(&rc, setup).unwrap();
            add(&rc, conflict, false, None).unwrap();
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(format!(
                    ". '{}' 2>&1; command -V gg 2>&1 || echo none",
                    rc.display()
                ))
                .env("PATH", "/usr/bin:/bin")
                .output()
                .unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(
                !said.contains("alias") && !said.contains("function"),
                "{said}"
            );
        }
    }
}
