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
//! Only plain files are read or written (through a symbolic link, such as a dotfiles
//! repository's, to the file it leads to): a FIFO or a device in a start-up file's place is
//! never waited on. The file is never rewritten. The line is appended to it, so nothing in it
//! can be overwritten, and the file keeps its inode, permissions, owner, ACLs and links.
//! Taking the line out shortens the file in place, and only when gg's lines are all that
//! follow what was there before: with the user's own lines after it, gg leaves the file as it
//! is and says which line to delete. (Taking a line out of the middle would mean writing the
//! file again, in place or as a new file renamed over it; either can lose what the user has
//! there if something else writes the file meanwhile or the write is cut short, or lose its
//! permissions. A line left for the user to delete loses nothing.) Every byte is kept as it
//! is: the file need not be UTF-8, and its line ends (CRLF) are followed. A file that ended
//! without a line break gets one before the line, and the line says so ([`NOTE_NO_EOL`]):
//! taking it out takes that line break out too.
//!
//! What no program can rule out without the cooperation of others writing the same file:
//! one that appends to it in the instant between gg's last look at its length and gg
//! shortening it. gg looks again immediately before, and never shortens a file whose length
//! is not the one it read.

use std::fs;
use std::io::{self, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};

use crate::error::{CoreError, ErrorCode, Result};

/// One change to a start-up file at a time: two of gg's own (the dialog and Settings at
/// once) never both find the line missing and both add it.
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    /// `unfunction`): quiet when there is nothing to turn off, and never a failure (`|| :`),
    /// so a start-up file that sets `errexit` (`set -e`) goes on past it when the alias has
    /// gone. It expands no variable (`set -u` is no matter either).
    pub fn command(self) -> &'static str {
        match self {
            Conflict::Alias => "unalias gg 2>/dev/null || :",
            Conflict::Function => "unset -f gg 2>/dev/null || :",
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
/// zsh: `.zshrc` in `ZDOTDIR` as it is when zsh reads `.zshrc` (see `zdotdir`, which the
/// caller learns from the shell itself), else in the home folder. Every interactive zsh reads
/// it, after `.zshenv` and `.zprofile`, which is where an alias such as Oh My Zsh's is made.
/// It may not exist yet; it is then made (only when the user asks, and the shell has just
/// said it has a `gg`), in a folder that exists.
///
/// bash: on macOS, terminals start login shells (Terminal, iTerm), which read the first of
/// `~/.bash_profile`, `~/.bash_login` and `~/.profile` that exists and can be read, and
/// `~/.bashrc` only if that one sources it: so the line goes at the end of that first file,
/// after anything it sources. On Linux, terminals start interactive shells that are not login
/// shells, which read `~/.bashrc` only (a login shell's `~/.profile` or `~/.bash_profile`
/// usually sources it): so the line goes at the end of `~/.bashrc`. The file must exist: a
/// new `~/.bash_profile` would stop bash reading the `~/.profile` the user has, and bash's
/// aliases are made in a file that exists, so gg never makes one (`None`). Only a plain file
/// counts. `macos` says which.
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
            .find(|f| readable_file(f)),
        Shell::Bash => Some(home.join(".bashrc")).filter(|f| readable_file(f)),
    }
}

/// Whether `file` is a plain file (through links) the user may read, as bash asks of its
/// login files.
fn readable_file(file: &Path) -> bool {
    if !fs::metadata(file).is_ok_and(|m| m.is_file()) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(file.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
        unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        true
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

/// Opens `file` (through links) with `options` only if it is a plain file, never waiting on
/// what is not one: it must be one before it is opened (a device is never opened), it is
/// opened without blocking (a FIFO put there meanwhile does not wait for a writer), and the
/// file opened must be one too. Anything else: an `InvalidInput` error.
fn open_plain(file: &Path, options: &mut fs::OpenOptions) -> io::Result<fs::File> {
    let not_plain = || io::Error::new(io::ErrorKind::InvalidInput, "not a plain file");
    if !fs::metadata(file)?.is_file() {
        return Err(not_plain());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let handle = options.open(file)?;
    if !handle.metadata()?.is_file() {
        return Err(not_plain());
    }
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // Blocking again, as plain files are read and written.
        let fd = handle.as_raw_fd();
        // SAFETY: `fd` is an open descriptor owned by `handle` for the duration of the calls.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags != -1 {
                libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
            }
        }
    }
    Ok(handle)
}

/// What plain file `file` holds (see [`open_plain`]).
fn read_plain(file: &Path) -> io::Result<Vec<u8>> {
    let mut content = Vec::new();
    open_plain(file, fs::OpenOptions::new().read(true))?.read_to_end(&mut content)?;
    Ok(content)
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

/// Whether file `file` holds a line gg wrote. A file that is not a plain file, or cannot be
/// read, holds none.
pub fn file_holds_line(file: &Path) -> bool {
    read_plain(file).is_ok_and(|c| holds_line(&c))
}

/// Whether file `file` holds gg's line for `conflict` (as [`file_holds_line`]).
pub fn file_holds_line_for(file: &Path, conflict: Conflict) -> bool {
    read_plain(file).is_ok_and(|c| holds_line_for(&c, conflict))
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
    add_with(file, conflict, create, home, || {})
}

/// [`add`], running `meanwhile` between reading the file and appending to it (tests: another
/// program writing the file then).
fn add_with(
    file: &Path,
    conflict: Conflict,
    create: bool,
    home: Option<&Path>,
    meanwhile: impl FnOnce(),
) -> Result<Added> {
    let _one_at_a_time = crate::lock_ignoring_poison(&WRITING);
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
            if let Err(e) = made.write_all(text.as_bytes()) {
                drop(made);
                // The file gg has just made, holding nothing of the user's.
                let _ = fs::remove_file(file);
                return Err(could_not(&e));
            }
            let _ = made.sync_all();
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
    let mut handle =
        open_plain(file, fs::OpenOptions::new().read(true).append(true)).map_err(|e| {
            if e.kind() == io::ErrorKind::InvalidInput {
                refused(format!("{name} is not a file; gg leaves it alone."))
            } else {
                could_not(&e)
            }
        })?;
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
        // After a final bare CR, a whole CRLF: the line break taken out with the line is then
        // the one gg added, and the CR stays.
        let before = if content.ends_with(b"\r") {
            "\r\n"
        } else {
            eol
        };
        format!("{before}{}{eol}", line_no_eol(true))
    };
    meanwhile();
    let before = content.len() as u64;
    // Nothing was added since it was read (an append would then land after it): if it was,
    // nothing is written, and nothing of the file is touched.
    match handle.metadata() {
        Ok(m) if m.len() == before => {}
        Ok(_) => {
            return Err(could_not(&io::Error::other(
                "the file changed while gg was adding the line; nothing was written",
            )));
        }
        Err(e) => return Err(could_not(&e)),
    }
    let bytes = text.as_bytes();
    let mut written = 0;
    while written < bytes.len() {
        match handle.write(&bytes[written..]) {
            Ok(0) => {
                return Err(undo_part(&mut handle, before, &bytes[..written], &name));
            }
            Ok(n) => written += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => {
                return Err(undo_part(&mut handle, before, &bytes[..written], &name));
            }
        }
    }
    // The line is in the file; a failure to flush it to the disk does not undo that.
    let _ = handle.sync_all();
    Ok(Added::Now)
}

/// After a write that stopped part way: takes `part` (what gg wrote of the line) off the end
/// again, only if the file is exactly what it was (`before` bytes) followed by `part` alone,
/// so nothing anyone else added since is cut. Returns the error to report, which says where a
/// part of the line was left when it could not be taken off.
fn undo_part(handle: &mut fs::File, before: u64, part: &[u8], name: &str) -> CoreError {
    let could_not = |detail: &str| {
        CoreError::new(ErrorCode::Io, format!("Could not add a line to {name}")).with_detail(detail)
    };
    if part.is_empty() {
        return could_not("the file could not be written; nothing was written");
    }
    let ours = (|| -> io::Result<bool> {
        if handle.metadata()?.len() != before + part.len() as u64 {
            return Ok(false);
        }
        let mut tail = vec![0; part.len()];
        handle.seek(io::SeekFrom::Start(before))?;
        handle.read_exact(&mut tail)?;
        Ok(tail == part)
    })();
    if ours.unwrap_or(false) && handle.set_len(before).is_ok() {
        return could_not(
            "the file could not be written; what gg had written of the line was taken off again",
        );
    }
    could_not(&format!(
        "the file could not be written; part of gg's line may be at the end of {name}, after what was there: delete it"
    ))
}

/// What [`remove`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Removed {
    /// gg's lines taken out (none when there were none).
    pub lines: usize,
    /// A line with gg's mark that was changed since is still there, left alone.
    pub changed_left: bool,
}

/// Takes gg's lines out of `file` (see the module's notes): the file is shortened in place,
/// every byte before them staying as it is. A missing file, or one that is not a plain file,
/// holds none. Refused, with the file as it was, when the user's own lines follow gg's (it
/// says which line to delete), or the file cannot be read or written (a read-only file stays
/// as it is), or changed while gg was taking the line out.
pub fn remove(file: &Path, home: Option<&Path>) -> Result<Removed> {
    remove_with(file, home, || {})
}

/// [`remove`], running `meanwhile` after the file is read and before it is checked again and
/// shortened (tests: another program writing the file then).
fn remove_with(file: &Path, home: Option<&Path>, meanwhile: impl FnOnce()) -> Result<Removed> {
    let _one_at_a_time = crate::lock_ignoring_poison(&WRITING);
    let name = shown(file, home);
    let could_not = |e: &io::Error| io_error(format!("Could not take gg's line out of {name}"), e);
    let content = match read_plain(file) {
        Ok(c) => c,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
            ) =>
        {
            return Ok(Removed::default());
        }
        Err(e) => return Err(could_not(&e)),
    };
    let spans = spans(&content);
    let mut cut = vec![false; content.len()];
    let mut removed = Removed::default();
    let mut ours_at = Vec::new();
    // From the end: whether every line after this one is gg's (and goes too).
    let mut only_ours_after = true;
    for (i, span) in spans.iter().enumerate().rev() {
        match kind(&content[span.start..span.text_end]) {
            Kind::Ours(_, no_eol) => {
                removed.lines += 1;
                ours_at.push(i + 1);
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
    let keep = cut.iter().position(|&c| c).unwrap_or(content.len());
    if cut[keep..].iter().any(|&c| !c) {
        // The user's own lines after gg's: the file stays as it is (see the module's notes).
        ours_at.reverse();
        let lines = ours_at
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" and ");
        let which = if ours_at.len() == 1 { "line" } else { "lines" };
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!(
                "{name} has lines of yours after gg's, so gg leaves the file as it is: delete {which} {lines} (ending “# added by gg: …”) yourself."
            ),
        ));
    }
    // Opened for writing, as a check that the user lets it be written: a read-only file stays
    // as it is.
    let mut handle = open_plain(file, fs::OpenOptions::new().read(true).write(true))
        .map_err(|e| could_not(&e))?;
    meanwhile();
    let check = (|| -> io::Result<()> {
        let mut now = Vec::new();
        handle.rewind()?;
        handle.read_to_end(&mut now)?;
        // Its length again, the last thing before it is shortened.
        if now != content || handle.metadata()?.len() != content.len() as u64 {
            return Err(io::Error::other(
                "the file changed while gg was taking the line out; it was left as it is",
            ));
        }
        Ok(())
    })();
    check.map_err(|e| could_not(&e))?;
    handle.set_len(keep as u64).map_err(|e| could_not(&e))?;
    // The line is out; a failure to flush that to the disk does not put it back.
    let _ = handle.sync_all();
    Ok(removed)
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
            "unalias gg 2>/dev/null || :  # added by gg: lets the gg command run; gg Settings > Command line takes it out"
        );
        assert_eq!(
            line(Conflict::Function, false),
            "unset -f gg 2>/dev/null || :  # added by gg: lets the gg command run; gg Settings > Command line takes it out"
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
        // A final bare CR stays: the line break gg adds after it is a whole CRLF, and that is
        // what comes out with the line.
        let before = b"a=1\r";
        let after = format!("a=1\r\r\n{}\n", line(Conflict::Alias, true));
        round_trip(before, after.as_bytes());
        let before = b"a=1\r\nb=2\r";
        let after = format!("a=1\r\nb=2\r\r\n{}\r\n", line(Conflict::Alias, true));
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

    /// The user's own lines after gg's: the file is left exactly as it is, and the refusal
    /// says which line to delete.
    #[test]
    fn with_lines_of_the_users_after_it_the_file_is_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, OMZ).unwrap();
        set_mode(&rc, 0o600);
        add(&rc, Conflict::Alias, false, Some(dir.path())).unwrap();
        let mut more = fs::OpenOptions::new().append(true).open(&rc).unwrap();
        more.write_all(b"export PATH=\"$HOME/.cargo/bin:$PATH\"\n")
            .unwrap();
        drop(more);
        let with_line = fs::read(&rc).unwrap();
        let err = remove(&rc, Some(dir.path())).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(
            err.message,
            "~/.zshrc has lines of yours after gg's, so gg leaves the file as it is: delete line 4 (ending “# added by gg: …”) yourself."
        );
        assert_eq!(fs::read(&rc).unwrap(), with_line);
        assert_eq!(mode(&rc), 0o600);
        // Still gg's line, found and shown as such.
        assert!(file_holds_line(&rc));
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [".zshrc"], "nothing made beside it");
        // Two of gg's lines, the user's between them: both named.
        fs::write(&rc, b"a=1").unwrap();
        add(&rc, Conflict::Alias, false, None).unwrap();
        let mut more = fs::OpenOptions::new().append(true).open(&rc).unwrap();
        more.write_all(b"b=2\n").unwrap();
        drop(more);
        add(&rc, Conflict::Function, false, None).unwrap();
        let err = remove(&rc, None).unwrap_err();
        assert!(
            err.message.contains("delete lines 2 and 4 "),
            "{}",
            err.message
        );
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

        assert_eq!(mode(&real), 0o640);
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

        // With the line at its end: still left as it is.
        set_mode(&rc, 0o644);
        add(&rc, Conflict::Alias, false, None).unwrap();
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
        // One bash cannot read is passed over, as bash passes over it; one that is not a plain
        // file does not count.
        if !is_root() {
            set_mode(&home.join(".bash_profile"), 0o000);
            assert_eq!(
                rc_file(Shell::Bash, home, None, true),
                Some(home.join(".bash_login"))
            );
            set_mode(&home.join(".bash_profile"), 0o644);
        }
        fs::remove_file(home.join(".bash_login")).unwrap();
        fs::remove_file(home.join(".bash_profile")).unwrap();
        fs::create_dir(home.join(".bash_profile")).unwrap();
        assert_eq!(
            rc_file(Shell::Bash, home, None, true),
            Some(home.join(".profile"))
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

    /// Runs `script` in `/bin/sh` with the start-up file `rc` as `$1` (never spliced into the
    /// script); what it printed.
    fn sh_with(script: &str, rc: &Path) -> String {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .arg(rc)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Prints `has` or `none` as `gg` is a command of the shell after `$1` is read.
    const PROBE: &str =
        r#". "$1"; if command -V gg >/dev/null 2>&1; then echo has; else echo none; fi"#;

    /// What the line does, in a POSIX shell (zsh and bash read it the same way): the alias, or
    /// the function, made before it is there before the line is added and gone after; the
    /// comment, and a CR before the line break, do nothing. The shell reaches the end of the
    /// file either way.
    #[test]
    fn the_line_turns_off_the_shells_gg() {
        let dir = tempfile::tempdir().unwrap();
        // A folder whose name a script would misread, passed as an argument.
        let dir = dir.path().join("it's here");
        fs::create_dir(&dir).unwrap();
        let rc = dir.join("rc");
        for (setup, conflict) in [
            ("alias gg='echo alias'\n", Conflict::Alias),
            ("x=1\r\nalias gg='echo alias'\r\n", Conflict::Alias),
            ("gg() { echo function; }\n", Conflict::Function),
            ("gg() { echo function; }", Conflict::Function),
        ] {
            fs::write(&rc, setup).unwrap();
            assert_eq!(sh_with(PROBE, &rc), "has\n", "{setup:?}");
            add(&rc, conflict, false, None).unwrap();
            assert_eq!(sh_with(PROBE, &rc), "none\n", "{setup:?}");
        }
    }

    /// With `errexit` (`set -e`) on, a line with nothing left to turn off does not end the
    /// shell reading the file: what comes after it still runs.
    #[test]
    fn the_line_never_stops_a_shell_that_exits_on_errors() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join("rc");
        for conflict in [Conflict::Alias, Conflict::Function] {
            fs::write(&rc, "set -eu\n").unwrap();
            add(&rc, conflict, false, None).unwrap();
            let said = sh_with(r#". "$1"; echo reached"#, &rc);
            assert_eq!(said, "reached\n", "{conflict:?}");
        }
    }

    /// A FIFO (or anything not a plain file) where a start-up file would be is never waited
    /// on: it holds no line, nothing is added to it, nothing taken out.
    #[test]
    fn a_fifo_in_its_place_is_never_waited_on() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join(".bashrc");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: `c` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let (tx, rx) = std::sync::mpsc::channel();
        let path = fifo.clone();
        std::thread::spawn(move || {
            let held = file_holds_line(&path) || file_holds_line_for(&path, Conflict::Alias);
            let added = add(&path, Conflict::Alias, false, None);
            let removed = remove(&path, None);
            let _ = tx.send((held, added, removed));
        });
        let (held, added, removed) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("waited on the FIFO");
        assert!(!held);
        let err = added.unwrap_err();
        assert!(err.message.contains("is not a file"), "{}", err.message);
        assert_eq!(removed.unwrap(), Removed::default());
        assert!(!readable_file(&fifo));
    }

    /// Another program appending to the file while gg adds its line: gg writes nothing, and
    /// what it appended stays.
    #[test]
    fn a_file_written_meanwhile_is_not_added_to_nor_cut() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, OMZ).unwrap();
        let err = add_with(&rc, Conflict::Alias, false, None, || {
            let mut other = fs::OpenOptions::new().append(true).open(&rc).unwrap();
            other.write_all(b"export THEIRS=1\n").unwrap();
        })
        .unwrap_err();
        assert!(
            err.detail
                .as_deref()
                .unwrap_or("")
                .contains("nothing was written"),
            "{err:?}"
        );
        let mut expected = OMZ.to_vec();
        expected.extend_from_slice(b"export THEIRS=1\n");
        assert_eq!(fs::read(&rc).unwrap(), expected);
    }

    /// Another program appending to the file while gg takes its line out: the file is not
    /// shortened, and what it appended stays.
    #[test]
    fn a_file_written_meanwhile_is_not_shortened() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        fs::write(&rc, OMZ).unwrap();
        add(&rc, Conflict::Alias, false, None).unwrap();
        let with_line = fs::read(&rc).unwrap();
        let err = remove_with(&rc, None, || {
            let mut other = fs::OpenOptions::new().append(true).open(&rc).unwrap();
            other.write_all(b"export THEIRS=1\n").unwrap();
        })
        .unwrap_err();
        assert!(
            err.detail
                .as_deref()
                .unwrap_or("")
                .contains("left as it is"),
            "{err:?}"
        );
        let mut expected = with_line;
        expected.extend_from_slice(b"export THEIRS=1\n");
        assert_eq!(fs::read(&rc).unwrap(), expected);
    }
}
