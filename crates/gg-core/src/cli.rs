//! The `gg` terminal command: a small launcher script (`src-tauri/resources/gg`) the app
//! writes, when the user asks, into a folder already on their login `PATH`. `gg [path]` then
//! opens the repository holding `path` in the app.
//!
//! The launcher the app installs is a copy, not a link, carrying a marker line ([`MARKER`]):
//! the app only ever replaces or removes a plain file that has it. A link to a launcher (the
//! one in the macOS app bundle, `gg.app/Contents/Resources/gg`, which a Homebrew cask links
//! into its `bin`) is the command too, but it belongs to whoever made it: the app shows it as
//! installed and never replaces, removes or writes through it. The app never needs
//! administrator rights and never edits shell files, the `PATH` or the registry; when the
//! user's shell already has a `gg` (an alias, a function, another program earlier on `PATH`),
//! the app says so ([`shadowed_by`]).
//!
//! On Windows the launcher is `gg.cmd` (`src-tauri/resources/gg.cmd`), written into a folder
//! of the user's own already on their `PATH` (`%LOCALAPPDATA%\Microsoft\WindowsApps`, which
//! Windows puts there for every user, `%USERPROFILE%\bin`, …), and what else is `gg` is found
//! as `where gg` would, by the app itself ([`windows_shadowed_by`]): no shell is asked.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::Core;
use crate::dto::{CliLink, CliState};
use crate::error::{CoreError, ErrorCode, Result};

/// The launcher, with `@GG_VERSION@` and `@GG_APP@` to fill in (see [`launcher_script`]).
const TEMPLATE: &str = include_str!("../../../src-tauri/resources/gg");

/// The Windows launcher, filled in by [`windows_launcher_script`].
const WINDOWS_TEMPLATE: &str = include_str!("../../../src-tauri/resources/gg.cmd");

/// What the marker line says after its comment sign (`#`; `rem` in the Windows launcher),
/// followed by the version that wrote it.
pub const MARKER: &str = "gg launcher, installed by gg ";

/// The launcher's file name.
pub const COMMAND: &str = if cfg!(windows) { "gg.cmd" } else { "gg" };

/// How long the login shell gets to say what `gg` is.
const SHELL_TIMEOUT: Duration = Duration::from_secs(3);

/// Brackets the shell's answer, so profile output around it cannot be taken for it.
#[cfg(unix)]
const WHICH_MARKER: &[u8] = b"__GG_WHICH__";

/// The launcher this platform's `gg` command is (see [`launcher_script`],
/// [`windows_launcher_script`]).
fn platform_launcher(version: &str, app: Option<&Path>) -> Result<String> {
    if !cfg!(windows) {
        return Ok(launcher_script(version, app));
    }
    let app = app.ok_or_else(|| {
        CoreError::new(
            ErrorCode::Io,
            "The gg command could not be written: gg could not tell where its own program is.",
        )
    })?;
    windows_launcher_script(version, &app.to_string_lossy())
}

/// The Windows launcher (`gg.cmd`) as it is written: the marker line naming `version`, and the
/// program to start, `app` (a full path). Its line ends are CRLF (cmd reads labels wrongly in
/// a file of bare LFs). The app's path is written into a `set "GG_APP=…"` line that cmd reads
/// with `%` expansion on and delayed expansion off: every `%` is doubled, and a path that
/// cmd would still read otherwise (a `"`, a line break, no Windows path has them) is refused.
pub fn windows_launcher_script(version: &str, app: &str) -> Result<String> {
    if app.contains(['"', '\r', '\n', '\0']) {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!(
                "The gg command cannot start a program at {app}: the path has a character cmd would misread."
            ),
        ));
    }
    let filled = WINDOWS_TEMPLATE
        .replace("@GG_VERSION@", version)
        .replace("@GG_APP@", &app.replace('%', "%%"));
    Ok(filled.replace("\r\n", "\n").replace('\n', "\r\n"))
}

/// The launcher as it is written: the marker line naming `version`, and the program to start
/// (`app`, used where there is no bundle identifier to find the app by: not macOS).
pub fn launcher_script(version: &str, app: Option<&Path>) -> String {
    let app = app.map_or_else(|| "''".to_owned(), |p| sh_quote(&p.to_string_lossy()));
    TEMPLATE
        .replace("@GG_VERSION@", version)
        .replace("@GG_APP@", &app)
}

/// `s` as one single-quoted POSIX shell word.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Whether `file` is a launcher the app wrote: one of its first lines is the marker line.
pub fn has_marker(file: &Path) -> bool {
    let mut head = [0u8; 512];
    let Ok(n) = fs::File::open(file).and_then(|mut f| f.read(&mut head)) else {
        return false;
    };
    String::from_utf8_lossy(&head[..n])
        .lines()
        .take(4)
        .any(|line| {
            ["# ", "rem "].iter().any(|sign| {
                line.strip_prefix(sign)
                    .is_some_and(|l| l.starts_with(MARKER))
            })
        })
}

/// The absolute folders of `search_path` (a `PATH` value), in order, once each (on Windows,
/// spelt the same but for case or a trailing separator, they are the same folder).
fn path_dirs(search_path: &OsStr) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(search_path) {
        let dir = if cfg!(windows) {
            PathBuf::from(
                dir.to_string_lossy()
                    .trim_end_matches(['\\', '/'])
                    .to_owned(),
            )
        } else {
            dir
        };
        if dir.is_absolute() && !dirs.iter().any(|d| same_dir(d, &dir)) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Whether folders `a` and `b` are spelt the same: on Windows whatever the case (its file
/// systems are case-insensitive) and the separators.
fn same_dir(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        same_windows_dir(&a.to_string_lossy(), &b.to_string_lossy())
    } else {
        a == b
    }
}

/// [`same_dir`] on Windows, as text.
fn same_windows_dir(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    norm(a) == norm(b)
}

/// The folders the launcher can be written to, best first: those of `search_path` (the login
/// `PATH`) that exist and the user can write to. First `~/.local/bin`, `/opt/homebrew/bin`
/// and `/usr/local/bin`, then the others under `home`, in `PATH` order. A folder under `home`
/// must not be inside a repository (a project's `node_modules/.bin`, say). (The system ones
/// may be: Homebrew's prefix is its own repository, which does not track `bin`.) Of the
/// others under `home`, those a tool rewrites are left out: version managers' `shims`, and
/// folders reached through a symbolic link (a version manager's per-shell link, which is
/// gone when that shell ends). A folder elsewhere on `PATH` that happens to be writable is
/// not offered.
pub fn candidates(search_path: &OsStr, home: Option<&Path>) -> Vec<PathBuf> {
    let preferred = if cfg!(windows) {
        windows_preferred(
            home,
            std::env::var_os("LOCALAPPDATA")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .as_deref(),
        )
    } else {
        let mut preferred = Vec::new();
        if let Some(home) = home {
            preferred.push(home.join(".local").join("bin"));
        }
        preferred.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
        preferred
    };
    candidates_with(search_path, home, &preferred)
}

/// On Windows, the folders of the user's own the command goes in first, when they are on the
/// `PATH`: `%LOCALAPPDATA%\Microsoft\WindowsApps` (on every user's `PATH` from the start),
/// then `%USERPROFILE%\bin` and `%USERPROFILE%\.local\bin`. Never a folder outside the
/// user's own (`Program Files`, `Windows`), whatever its permissions say.
fn windows_preferred(home: Option<&Path>, local_app_data: Option<&Path>) -> Vec<PathBuf> {
    let mut preferred = Vec::new();
    if let Some(local) = local_app_data {
        preferred.push(local.join("Microsoft").join("WindowsApps"));
    }
    if let Some(home) = home {
        preferred.push(home.join("bin"));
        preferred.push(home.join(".local").join("bin"));
    }
    preferred
}

fn candidates_with(
    search_path: &OsStr,
    home: Option<&Path>,
    preferred: &[PathBuf],
) -> Vec<PathBuf> {
    let on_path = path_dirs(search_path);
    let under_home = |dir: &Path| {
        home.is_some_and(|h| {
            if cfg!(windows) {
                let (d, h) = (
                    dir.to_string_lossy().to_lowercase(),
                    h.to_string_lossy().to_lowercase(),
                );
                let h = h.trim_end_matches(['\\', '/']);
                d.strip_prefix(h)
                    .is_some_and(|rest| rest.starts_with(['\\', '/']))
            } else {
                dir.starts_with(h)
            }
        })
    };
    // As the PATH spells it (on Windows its case may differ from the preferred one's).
    let listed = |dir: &Path| on_path.iter().find(|d| same_dir(d, dir)).cloned();
    let usable = |dir: &PathBuf| {
        is_writable_dir(dir)
            && !(under_home(dir) && in_repository(dir))
            && (!cfg!(windows) || under_home(dir))
    };
    let mut out: Vec<PathBuf> = preferred
        .iter()
        .filter_map(|d| listed(d))
        .filter(|d| usable(d))
        .collect();
    for dir in &on_path {
        let rewritten = dir.file_name().is_some_and(|n| n == "shims")
            || home.is_some_and(|h| through_a_link(h, dir));
        if under_home(dir) && !out.iter().any(|o| same_dir(o, dir)) && !rewritten && usable(dir) {
            out.push(dir.clone());
        }
    }
    out
}

/// Whether some folder on the way from `home` down to `dir` (under it) is a symbolic link.
fn through_a_link(home: &Path, dir: &Path) -> bool {
    let Ok(rest) = dir.strip_prefix(home) else {
        return false;
    };
    let mut at = home.to_path_buf();
    rest.components().any(|part| {
        at.push(part);
        fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink())
    })
}

/// Whether `dir` is a folder the user may create files in.
fn is_writable_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `c` is a valid NUL-terminated path for the duration of the call.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        fs::metadata(dir).is_ok_and(|m| !m.permissions().readonly())
    }
}

/// Whether `dir` or a folder above it holds a `.git`, where `dir` really is: a folder
/// reached through a symbolic link (a `~/.local/bin` linked into a dotfiles repository) is
/// judged by where the link leads.
fn in_repository(dir: &Path) -> bool {
    let real = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    [dir, real.as_path()]
        .iter()
        .any(|p| p.ancestors().any(|d| d.join(".git").exists()))
}

/// The `gg` command on the `PATH` that is gg's: where it is, and who made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub path: PathBuf,
    /// `None`: a launcher the app wrote; else a link to one, made outside the app.
    pub link: Option<CliLink>,
}

/// The first `gg` on `search_path` that is gg's, if any: a plain file with the marker (a
/// launcher the app wrote), or a link that leads to one (a Homebrew cask's link to the
/// launcher in the app bundle, say), which install and uninstall leave alone. Another `gg`
/// before it is passed over: the shell check names that one.
pub fn installed(search_path: &OsStr) -> Option<Installed> {
    path_dirs(search_path).into_iter().find_map(|d| {
        let path = d.join(COMMAND);
        launcher_at(&path).map(|link| Installed { path, link })
    })
}

/// Whether `file` is gg's launcher: `Some(None)` for a plain file with the marker (one the app
/// wrote), `Some(Some(_))` for a link that leads, through any number of links, to a plain file
/// with the marker; `None` for anything else (no file, a folder, a link that leads nowhere or
/// to something else).
fn launcher_at(file: &Path) -> Option<Option<CliLink>> {
    let meta = fs::symlink_metadata(file).ok()?;
    if meta.is_file() {
        return has_marker(file).then_some(None);
    }
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = dunce::canonicalize(file).ok()?;
    let lands_on_launcher = fs::metadata(&target).is_ok_and(|m| m.is_file()) && has_marker(&target);
    lands_on_launcher.then(|| Some(link_maker(file, &target)))
}

/// Who made `link`, a link to gg's launcher at `target` (where it really is): Homebrew when the
/// link is in a Homebrew prefix's `bin` (the prefix has a `Caskroom`, where casks are kept)
/// and leads into an app bundle (`….app/Contents/Resources/gg`), as a cask's `binary` stanza
/// makes it; anyone else otherwise.
fn link_maker(link: &Path, target: &Path) -> CliLink {
    let in_prefix = link
        .parent()
        .and_then(Path::parent)
        .is_some_and(|prefix| prefix.join("Caskroom").is_dir());
    let in_bundle = target.parent().is_some_and(|resources| {
        resources.ends_with("Contents/Resources")
            && resources
                .parent()
                .and_then(Path::parent)
                .is_some_and(|app| app.extension().is_some_and(|e| e == "app"))
    });
    if in_prefix && in_bundle {
        CliLink::Homebrew
    } else {
        CliLink::Other
    }
}

/// Writes `script` as `gg` (mode 0755; `gg.cmd` on Windows) in `dir` and returns its path. A
/// launcher the app wrote before is replaced; anything else there is left alone and the error
/// says what it is.
pub fn install(dir: &Path, script: &str) -> Result<PathBuf> {
    let target = dir.join(COMMAND);
    let failed = |e: io::Error| {
        CoreError::new(
            ErrorCode::Io,
            format!("Could not write {}", target.display()),
        )
        .with_detail(e.to_string())
    };
    let replacing = match fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_file() && has_marker(&target) => true,
        Ok(meta) => return Err(not_ours(dir, &target, &meta)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(failed(e)),
    };
    // Whole or absent: written beside it under a hidden name, then renamed over it. The temp
    // file is made new (a stale one of ours goes first), never written through a link.
    let tmp = dir.join(format!(".{COMMAND}.{}.tmp", std::process::id()));
    // Set once the launcher is moved into place: only then does "already exists" mean a gg
    // that appeared meanwhile (before, it is about the temp file).
    let mut placing = false;
    // Set once the temp file is ours to remove.
    let mut made = false;
    let mut written = || {
        // Removing a link someone put there removes the link, not what it points at.
        match fs::symlink_metadata(&tmp) {
            Ok(meta) if !meta.is_dir() => fs::remove_file(&tmp)?,
            _ => {}
        }
        let mut file = crate::store::create_new_file(&tmp, 0o755)?;
        made = true;
        file.write_all(script.as_bytes())?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The umask may have taken bits off at creation.
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
        }
        placing = true;
        if replacing {
            fs::rename(&tmp, &target)
        } else {
            // A first install never replaces a gg that appeared meanwhile.
            crate::store::move_no_overwrite(&tmp, &target)
        }
    };
    let written = written();
    if let Err(e) = written {
        if made {
            let _ = fs::remove_file(&tmp);
        }
        if placing
            && e.kind() == io::ErrorKind::AlreadyExists
            && let Ok(meta) = fs::symlink_metadata(&target)
        {
            return Err(not_ours(dir, &target, &meta));
        }
        return Err(failed(e));
    }
    Ok(target)
}

/// The refusal to touch `target` in `dir`, a `gg` the app did not write, saying what it is.
fn not_ours(dir: &Path, target: &Path, meta: &fs::Metadata) -> CoreError {
    // A link to gg's launcher is the command already, kept by whoever made it.
    if let Some(Some(link)) = launcher_at(target) {
        let made_by = match link {
            CliLink::Homebrew => ", made by Homebrew",
            CliLink::Other => "",
        };
        return CoreError::new(
            ErrorCode::BadRequest,
            format!(
                "There is already a gg in {}{made_by}: {}, gg's launcher. gg leaves it alone; choose another folder.",
                dir.display(),
                what_is_there(target, meta)
            ),
        );
    }
    CoreError::new(
        ErrorCode::BadRequest,
        format!(
            "There is already a gg in {} that gg did not write: {}. Remove it, or choose another folder.",
            dir.display(),
            what_is_there(target, meta)
        ),
    )
}

/// Removes every launcher the app wrote in the folders of `search_path` and returns their
/// paths; any other `gg` is left alone.
pub fn uninstall(search_path: &OsStr) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for dir in path_dirs(search_path) {
        let file = dir.join(COMMAND);
        let ours = fs::symlink_metadata(&file).is_ok_and(|m| m.is_file()) && has_marker(&file);
        if !ours {
            continue;
        }
        fs::remove_file(&file).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("Could not remove {}", file.display()),
            )
            .with_detail(e.to_string())
        })?;
        removed.push(file);
    }
    Ok(removed)
}

/// A few words on what file `path` (with metadata `meta`) is.
fn what_is_there(path: &Path, meta: &fs::Metadata) -> String {
    if meta.file_type().is_symlink() {
        return match fs::read_link(path) {
            Ok(to) => format!("a link to {}", to.display()),
            Err(_) => "a link".to_owned(),
        };
    }
    if meta.is_dir() {
        return "a folder".to_owned();
    }
    let mut head = [0u8; 256];
    let n = fs::File::open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    let head = &head[..n];
    if let Some(script) = head.strip_prefix(b"#!") {
        let line = String::from_utf8_lossy(script);
        let line = line.lines().next().unwrap_or("").trim();
        return format!("a script run by {line}");
    }
    const PROGRAMS: [&[u8]; 6] = [
        b"\x7fELF",
        b"\xcf\xfa\xed\xfe",
        b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
        b"\xfe\xed\xfa\xce",
        b"\xca\xfe\xba\xbe",
    ];
    if PROGRAMS.iter().any(|magic| head.starts_with(magic)) {
        return "a program".to_owned();
    }
    let size = meta.len();
    format!("a file of {size} byte{}", if size == 1 { "" } else { "s" })
}

/// What the user's login shell `shell` runs for `gg` when it is not a launcher the app
/// wrote: an alias, a function, a builtin or a program earlier on its `PATH`, in a few
/// words. Asks the shell itself (`$SHELL -ilc 'command -V gg'`, so the aliases and functions
/// of its interactive start-up files count, and its login `PATH`), killed after
/// [`SHELL_TIMEOUT`]. `None` when nothing else is `gg`, or when the shell does not say
/// (fails, takes too long, or answers in a form not understood): a check that cannot be made
/// never stands in the way of installing.
pub fn shadowed_by(shell: &Path) -> Option<String> {
    shadowed_by_within(shell, SHELL_TIMEOUT)
}

fn shadowed_by_within(shell: &Path, timeout: Duration) -> Option<String> {
    #[cfg(unix)]
    {
        let script = "printf '\\n__GG_WHICH__'; command -V gg; printf '__GG_WHICH__'";
        let out = crate::git::runner::shell_output(shell, &["-ilc", script], timeout)?;
        let answer = crate::git::runner::between_markers(&out, WHICH_MARKER)?;
        describe(&String::from_utf8_lossy(answer))
    }
    #[cfg(not(unix))]
    {
        let _ = (shell, timeout);
        None
    }
}

/// On Windows, what else is `gg` for cmd and PowerShell, found as `where gg` would find it
/// but by the app itself: the first file called `gg` with an extension of `pathext`
/// (`PATHEXT`) in a folder of `search_path`, unless it is a launcher the app wrote
/// (`ours`). `None` when there is none, or ours comes first. (A doskey macro or a PowerShell
/// alias cannot be seen from outside the shell.)
fn windows_shadowed_by(
    search_path: &str,
    pathext: Option<&str>,
    exists: &dyn Fn(&str) -> bool,
    ours: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let exts = pathext
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(".COM;.EXE;.BAT;.CMD");
    for dir in crate::platform::windows_path_dirs(search_path) {
        for ext in exts.split(';').map(str::trim).filter(|e| !e.is_empty()) {
            let file = format!(
                "{}\\gg{}",
                dir.trim_end_matches(['\\', '/']),
                ext.to_ascii_lowercase()
            );
            if exists(&file) {
                return (!ours(&file)).then(|| format!("the program {file}"));
            }
        }
    }
    None
}

/// The shell's answer to `command -V gg` (zsh's, bash's or a POSIX sh's), in a few words;
/// `None` when it is not found, is a launcher the app wrote, or the answer is not understood.
#[cfg(unix)]
fn describe(answer: &str) -> Option<String> {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty())?;
    let what = line.strip_prefix("gg is ")?.trim();
    // zsh: "an alias for …"; bash: "aliased to `…'".
    if let Some(value) = what.strip_prefix("aliased to ") {
        let value = value.trim_start_matches(['`', '\'']).trim_end_matches('\'');
        return Some(format!("an alias for {value}"));
    }
    if what.starts_with("an alias") {
        return Some(what.to_owned());
    }
    // bash: "a function"; zsh: "a shell function (from …)".
    if what == "a function" {
        return Some("a shell function".to_owned());
    }
    // A program: "/usr/local/bin/gg", or bash's "hashed (/usr/local/bin/gg)".
    let program = what
        .strip_prefix("hashed (")
        .and_then(|p| p.strip_suffix(')'))
        .unwrap_or(what);
    if Path::new(program).is_absolute() {
        // gg's launcher, or a link to it, is not something else.
        if launcher_at(Path::new(program)).is_some() {
            return None;
        }
        return Some(format!("the program {program}"));
    }
    Some(what.to_owned())
}

/// How [`Core::start_cli`] sets up the `gg` command.
#[derive(Debug, Default, Clone)]
pub struct CliSetup {
    /// The login shell asked what `gg` is; `None`: never asked (`shadowedBy` stays null).
    pub shell: Option<PathBuf>,
    /// The `PATH` the command is looked for on, instead of the login shell's (as for git).
    pub search_path: Option<OsString>,
    /// The home folder, instead of `HOME` (`USERPROFILE` on Windows).
    pub home: Option<PathBuf>,
}

/// Where the `gg` command is looked for and checked: the login `PATH`, the home folder, the
/// login shell (none: no shadow check), and the program a launcher starts (not macOS).
pub(crate) struct CliContext {
    search_path: OsString,
    home: Option<PathBuf>,
    shell: Option<PathBuf>,
    app: Option<PathBuf>,
    /// The last [`shadowed_by`] answer and the number of the check that gave it.
    shadowed_by: Option<String>,
    checked: u64,
    /// Checks started so far.
    checks: u64,
}

impl CliContext {
    /// `search_path` is the login shell's `PATH` (the app's own when it was not read).
    pub(crate) fn new(search_path: Option<OsString>) -> Self {
        let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        Self {
            search_path: search_path
                .or_else(|| std::env::var_os("PATH"))
                .unwrap_or_default(),
            home: std::env::var_os(home_var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            shell: None,
            app: app_program(),
            shadowed_by: None,
            checked: 0,
            checks: 0,
        }
    }
}

/// The program a Linux or Windows launcher starts: the AppImage the app runs from, else the
/// app's own executable. `None` on macOS, where the launcher finds the app by its bundle
/// identifier.
fn app_program() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return None;
    }
    std::env::var_os("APPIMAGE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
}

impl Core {
    /// Sets up the `gg` command (see [`CliSetup`]) and asks the login shell what `gg` is,
    /// on a background thread. Call once at start; the app passes `$SHELL`.
    pub fn start_cli(self: &Arc<Self>, setup: CliSetup) {
        {
            let mut cli = crate::lock_ignoring_poison(&self.cli);
            cli.shell = setup.shell;
            if let Some(path) = setup.search_path {
                cli.search_path = path;
            }
            if let Some(home) = setup.home {
                cli.home = Some(home);
            }
        }
        let core = self.clone();
        std::thread::spawn(move || core.check_shadow());
    }

    /// Asks the login shell what `gg` is and keeps the answer, unless a later check has
    /// answered meanwhile; a new answer is announced as `cli-changed` (with the command's
    /// state), since the UI may have read the state before it came. Blocks for up to
    /// [`SHELL_TIMEOUT`].
    fn check_shadow(&self) {
        let (shell, number, search_path) = {
            let mut cli = crate::lock_ignoring_poison(&self.cli);
            cli.checks += 1;
            (cli.shell.clone(), cli.checks, cli.search_path.clone())
        };
        let answer = if cfg!(windows) {
            // No login shell to ask: the app looks itself.
            windows_shadowed_by(
                &search_path.to_string_lossy(),
                std::env::var("PATHEXT").ok().as_deref(),
                &|f| Path::new(f).is_file(),
                &|f| has_marker(Path::new(f)),
            )
        } else {
            let Some(shell) = shell else {
                return;
            };
            shadowed_by(&shell)
        };
        let changed = {
            let mut cli = crate::lock_ignoring_poison(&self.cli);
            let newest = number > cli.checked;
            let changed = newest && cli.shadowed_by != answer;
            if newest {
                cli.checked = number;
                cli.shadowed_by = answer;
            }
            changed
        };
        if changed && let Ok(state) = serde_json::to_value(self.cli_state()) {
            self.sink.emit("cli-changed", state);
        }
    }

    /// The `gg` command's state as it is now (the shell's answer as last known).
    pub(crate) fn cli_state(&self) -> CliState {
        let cli = crate::lock_ignoring_poison(&self.cli);
        let found = installed(&cli.search_path);
        CliState {
            installed: found
                .as_ref()
                .map(|f| f.path.to_string_lossy().into_owned()),
            link: found.and_then(|f| f.link),
            shadowed_by: cli.shadowed_by.clone(),
            candidates: candidates(&cli.search_path, cli.home.as_deref())
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        }
    }

    /// Writes the launcher into `dir`, which must be one of the candidates, then asks the
    /// shell again. Blocking: call on the blocking pool.
    pub(crate) fn install_cli(&self, dir: &Path) -> Result<CliState> {
        let (allowed, app) = {
            let cli = crate::lock_ignoring_poison(&self.cli);
            let allowed = candidates(&cli.search_path, cli.home.as_deref());
            (allowed, cli.app.clone())
        };
        if !allowed.iter().any(|c| same_dir(c, dir)) {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!(
                    "{} is not a folder the gg command can go in (on your PATH and writable)",
                    dir.display()
                ),
            ));
        }
        let script = platform_launcher(env!("CARGO_PKG_VERSION"), app.as_deref())?;
        install(dir, &script)?;
        self.check_shadow();
        Ok(self.cli_state())
    }

    /// Removes the launchers the app wrote on the `PATH`, then asks the shell again. Returns
    /// the files actually removed (none when there was nothing to remove) and the new state.
    /// Blocking: call on the blocking pool.
    pub(crate) fn uninstall_cli(&self) -> Result<(Vec<PathBuf>, CliState)> {
        let search_path = crate::lock_ignoring_poison(&self.cli).search_path.clone();
        let removed = uninstall(&search_path)?;
        self.check_shadow();
        Ok((removed, self.cli_state()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_carries_the_marker_and_the_app() {
        let script = launcher_script("1.2.3", Some(Path::new("/opt/it's here/gg-desktop")));
        assert!(script.starts_with("#!/bin/sh\n# gg launcher, installed by gg 1.2.3\n"));
        assert!(script.contains("GG_VERSION='1.2.3'"));
        assert!(script.contains(r"GG_APP='/opt/it'\''s here/gg-desktop'"));
        assert!(!script.contains("@GG_"), "every placeholder is filled");
        let none = launcher_script("1.2.3", None);
        assert!(none.contains("GG_APP=''\n"));
    }

    /// The Windows launcher cannot be run here; this checks statically that a path holding
    /// `&`, `(`, `)` or `!` can never be taken for batch syntax: every `%VAR%` of a path is
    /// inside quotes, an `echo` names a path only as `!VAR!` (delayed expansion, which does
    /// not parse the value), and every other `!VAR!` is a quoted argument.
    #[test]
    fn the_windows_launcher_never_expands_a_path_outside_quotes() {
        const CMD: &str = include_str!("../../../src-tauri/resources/gg.cmd");
        const NAMES: [&str; 3] = ["TARGET", "GG_APP", "GG_APP_BIN"];
        let quoted = |line: &str, at: usize| line[..at].matches('"').count() % 2 == 1;
        let mut delayed = false;
        let mut echoed = 0;
        for (n, line) in CMD.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("rem") || code.starts_with(':') {
                continue;
            }
            delayed |= code.starts_with("setlocal EnableDelayedExpansion");
            let echo = code.starts_with("echo");
            for name in NAMES {
                for pattern in [format!("%{name}%"), format!("%{name}:")] {
                    for (at, _) in line.match_indices(&pattern) {
                        assert!(!echo, "line {}: an echo expands %{name}%: {line}", n + 1);
                        assert!(quoted(line, at), "line {}: unquoted: {line}", n + 1);
                    }
                }
                for (at, _) in line.match_indices(&format!("!{name}!")) {
                    assert!(delayed, "line {}: !{name}! before it is enabled", n + 1);
                    if echo {
                        echoed += 1;
                    } else {
                        assert!(quoted(line, at), "line {}: unquoted: {line}", n + 1);
                    }
                }
            }
            // No block holds a value: `(` opens only a `for` set.
            if code.ends_with('(') || code.contains(") else") {
                panic!("line {}: a ( ) block: {line}", n + 1);
            }
        }
        assert!(echoed >= 3, "the messages name the paths");
        // No program is looked for in the current folder (a repository's git.cmd).
        assert!(CMD.contains(r#"set "NoDefaultCurrentDirectoryInExePath=1""#));
        assert!(!CMD.contains("where git"));
        assert!(CMD.contains("setlocal DisableDelayedExpansion"));
        // No output is captured: `for /f` runs its command through another cmd, which
        // parses the path again.
        assert!(!CMD.to_lowercase().contains("for /f"));
        assert!(!CMD.contains("%%T"));
        assert!(
            CMD.contains(
                r#"git -C "!TARGET!" rev-parse --show-toplevel >nul 2>nul || goto notrepo"#
            )
        );
        // A root's trailing backslash would escape the argument's closing quote.
        assert!(CMD.contains(r#"if "!TARGET:~-1!"=="\" set "TARGET=!TARGET!.""#));
        assert!(CMD.contains(r#"start "" "!GG_APP!" "!TARGET!""#));
    }

    #[test]
    fn the_windows_launcher_is_filled_in_for_cmd() {
        let app = r"C:\Users\a%b%\AppData\Local\gg (x86) & co!\gg-desktop.exe";
        let script = windows_launcher_script("1.2.3", app).unwrap();
        assert!(script.starts_with("@echo off\r\nrem gg launcher, installed by gg 1.2.3\r\n"));
        // Every % doubled: cmd reads the line with % expansion on.
        assert!(script.contains(
            "set \"GG_APP=C:\\Users\\a%%b%%\\AppData\\Local\\gg (x86) & co!\\gg-desktop.exe\"\r\n"
        ));
        assert!(!script.contains("@GG_"), "every placeholder is filled");
        // CRLF throughout, no bare LF.
        assert!(!script.replace("\r\n", "").contains('\n'));
        assert!(script.contains("NoDefaultCurrentDirectoryInExePath=1"));
        for bad in ["C:\\a\"b.exe", "C:\\a\nb.exe"] {
            assert_eq!(
                windows_launcher_script("1", bad).unwrap_err().code,
                ErrorCode::BadRequest
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gg.cmd");
        fs::write(&file, &script).unwrap();
        assert!(has_marker(&file));
    }

    #[test]
    fn windows_offers_the_users_own_folders_first() {
        let home = Path::new("/Users/ada");
        let local = Path::new("/Users/ada/AppData/Local");
        assert_eq!(
            windows_preferred(Some(home), Some(local)),
            [
                local.join("Microsoft").join("WindowsApps"),
                home.join("bin"),
                home.join(".local").join("bin"),
            ]
        );
        assert!(windows_preferred(None, None).is_empty());
        assert!(same_windows_dir(r"C:\Users\Ada\bin\", "c:/users/ada/BIN"));
        assert!(!same_windows_dir(r"C:\Users\Ada\bin", r"C:\Users\Ada\bin2"));
    }

    #[test]
    fn windows_finds_what_else_is_gg_as_where_does() {
        let disk = |files: &'static [&'static str]| {
            move |f: &str| files.iter().any(|x| x.eq_ignore_ascii_case(f))
        };
        let ours = |f: &str| f.eq_ignore_ascii_case(r"C:\Users\ada\bin\gg.cmd");
        let path = r"C:\Tools;C:\Users\ada\bin";
        // Another gg earlier on PATH.
        assert_eq!(
            windows_shadowed_by(
                path,
                None,
                &disk(&[r"C:\Tools\gg.exe", r"C:\Users\ada\bin\gg.cmd"]),
                &ours
            ),
            Some(r"the program C:\Tools\gg.exe".to_owned())
        );
        // PATHEXT order within a folder: gg.bat before gg.cmd.
        assert_eq!(
            windows_shadowed_by(
                path,
                Some(".BAT;.CMD"),
                &disk(&[r"C:\Tools\gg.cmd", r"C:\Tools\gg.bat"]),
                &ours
            ),
            Some(r"the program C:\Tools\gg.bat".to_owned())
        );
        // Ours first, or nothing at all: nothing else is gg.
        assert_eq!(
            windows_shadowed_by(path, None, &disk(&[r"C:\Users\ada\bin\gg.cmd"]), &ours),
            None
        );
        assert_eq!(windows_shadowed_by(path, None, &disk(&[]), &ours), None);
        // A relative PATH entry is never looked in.
        assert_eq!(
            windows_shadowed_by(
                r".;tools",
                None,
                &disk(&[r".\gg.exe", r"tools\gg.exe"]),
                &ours
            ),
            None
        );
    }

    #[test]
    fn the_marker_is_recognised_in_the_first_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("gg");
        fs::write(&file, launcher_script("0.1.0", None)).unwrap();
        assert!(has_marker(&file));
        fs::write(
            &file,
            "@echo off\r\nrem gg launcher, installed by gg 0.1.0\r\n",
        )
        .unwrap();
        assert!(has_marker(&file));
        fs::write(&file, "#!/bin/sh\necho hi\n").unwrap();
        assert!(!has_marker(&file));
        fs::write(&file, "1\n2\n3\n4\n# gg launcher, installed by gg 0.1.0\n").unwrap();
        assert!(!has_marker(&file));
        assert!(!has_marker(&dir.path().join("missing")));
    }

    // Unix only: the answer of a POSIX shell, which only Unix asks (Windows looks itself).
    #[cfg(unix)]
    #[test]
    fn describes_what_the_shell_says() {
        let zsh_alias = "gg is an alias for git log --graph";
        assert_eq!(describe(zsh_alias).unwrap(), "an alias for git log --graph");
        let bash_alias = "gg is aliased to `git log --graph'";
        assert_eq!(
            describe(bash_alias).unwrap(),
            "an alias for git log --graph"
        );
        assert_eq!(
            describe("gg is a function\ngg () \n{\n  :\n}").unwrap(),
            "a shell function"
        );
        let zsh_fn = "gg is a shell function from /x/.zshrc";
        assert_eq!(
            describe(zsh_fn).unwrap(),
            zsh_fn.strip_prefix("gg is ").unwrap()
        );
        assert_eq!(
            describe("\ngg is /opt/x/gg\n").unwrap(),
            "the program /opt/x/gg"
        );
        assert_eq!(
            describe("gg is hashed (/opt/x/gg)").unwrap(),
            "the program /opt/x/gg"
        );
        assert_eq!(describe("gg not found"), None);
        assert_eq!(describe(""), None);
    }

    /// `dirs` joined as a `PATH` value.
    #[cfg(unix)]
    fn path_of(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[cfg(unix)]
    fn mkdir(path: &Path) -> PathBuf {
        fs::create_dir_all(path).unwrap();
        path.to_path_buf()
    }

    #[cfg(unix)]
    fn read_only(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o555)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn candidates_are_writable_folders_on_the_path_outside_repositories() {
        let root = tempfile::tempdir().unwrap();
        let home = mkdir(&root.path().join("home"));
        let local = mkdir(&home.join(".local/bin"));
        let bin = mkdir(&home.join("bin"));
        let tools = mkdir(&home.join("tools/bin"));
        let ro = mkdir(&home.join("ro"));
        read_only(&ro);
        let repo = mkdir(&home.join("project"));
        mkdir(&repo.join(".git"));
        let in_repo = mkdir(&repo.join("node_modules/.bin"));
        let missing = home.join("missing");
        let shims = mkdir(&home.join(".pyenv/shims"));
        let real_node = mkdir(&root.path().join("node-22/bin"));
        let link_parent = mkdir(&home.join(".local/state/fnm"));
        std::os::unix::fs::symlink(real_node.parent().unwrap(), link_parent.join("123_456"))
            .unwrap();
        let linked = link_parent.join("123_456/bin");
        let elsewhere = mkdir(&root.path().join("elsewhere"));
        let not_on_path = mkdir(&home.join("not-on-path"));
        let search = path_of(&[
            &elsewhere,
            &tools,
            &in_repo,
            &ro,
            &missing,
            &bin,
            Path::new("relative/bin"),
            &local,
            &tools,
            &shims,
            &linked,
        ]);
        let _ = not_on_path;
        // ~/.local/bin first, then the others under home in PATH order, once each; not a
        // version manager's shims, nor a folder reached through a link.
        assert!(linked.is_dir());
        assert_eq!(
            candidates(&search, Some(&home)),
            [local.clone(), tools.clone(), bin.clone()]
        );
        // Without a home folder, only the usual folders count.
        assert!(candidates(&search, None).is_empty());
        // ~/.local/bin only when it is on PATH.
        assert_eq!(candidates(&path_of(&[&bin]), Some(&home)), [bin]);
    }

    /// A folder is judged by where it really is: a `~/.local/bin` that is a link into a
    /// dotfiles repository (inside home or out of it) is not offered.
    #[cfg(unix)]
    #[test]
    fn a_folder_linked_into_a_repository_is_not_offered() {
        let root = tempfile::tempdir().unwrap();
        for dotfiles in ["home/dotfiles", "dotfiles"] {
            let home = mkdir(&root.path().join("home"));
            let repo = mkdir(&root.path().join(dotfiles));
            mkdir(&repo.join(".git"));
            let target = mkdir(&repo.join("bin"));
            mkdir(&home.join(".local"));
            let local = home.join(".local/bin");
            let _ = fs::remove_file(&local);
            std::os::unix::fs::symlink(&target, &local).unwrap();
            let bin = mkdir(&home.join("bin"));
            assert_eq!(
                candidates(&path_of(&[&local, &bin]), Some(&home)),
                [bin],
                "{dotfiles}"
            );
        }
    }

    // Unix only: the usual folders here are outside home, which Windows never offers (its
    // own are tested in `windows_offers_the_users_own_folders_first`).
    #[cfg(unix)]
    #[test]
    fn the_usual_folders_come_first_in_their_order() {
        let root = tempfile::tempdir().unwrap();
        let home = mkdir(&root.path().join("home"));
        let local = mkdir(&home.join(".local/bin"));
        let mine = mkdir(&home.join("bin"));
        // Stand-ins for /opt/homebrew/bin and /usr/local/bin, outside home. Homebrew's prefix
        // is a repository of its own: its bin is offered all the same.
        let brew = mkdir(&root.path().join("homebrew/bin"));
        mkdir(&root.path().join("homebrew/.git"));
        let usr = mkdir(&root.path().join("usr/local/bin"));
        let preferred = [local.clone(), brew.clone(), usr.clone()];
        let search = path_of(&[&mine, &usr, &brew, &local]);
        assert_eq!(
            candidates_with(&search, Some(&home), &preferred),
            [local, brew, usr, mine]
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_writes_an_executable_launcher_and_is_idempotent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = launcher_script("0.1.0", None);
        let path = install(dir.path(), &script).unwrap();
        assert_eq!(path, dir.path().join("gg"));
        assert_eq!(fs::read_to_string(&path).unwrap(), script);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        // Again (a newer version, say): replaced, nothing else left there.
        let newer = launcher_script("0.2.0", None);
        assert_eq!(install(dir.path(), &newer).unwrap(), path);
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["gg"]);
        assert_eq!(installed(&path_of(&[dir.path()])), Some(written(&path)));
    }

    /// The temp file is made new: a stale one of ours is replaced, and a link planted under
    /// its name is never written through.
    #[cfg(unix)]
    #[test]
    fn install_never_writes_through_a_planted_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        fs::write(&victim, "keep me").unwrap();
        let tmp = dir.path().join(format!(".gg.{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
        let script = launcher_script("0.1.0", None);
        let path = install(dir.path(), &script).unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "keep me");
        assert_eq!(fs::read_to_string(&path).unwrap(), script);
        assert!(fs::symlink_metadata(&tmp).is_err(), "no temp file left");
        // A stale temp file of ours.
        fs::write(&tmp, "half a script").unwrap();
        install(dir.path(), &script).unwrap();
        assert!(fs::symlink_metadata(&tmp).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), script);
        // A folder in the temp file's place: a plain failure, never "a gg gg did not write"
        // (the gg there is ours).
        fs::create_dir(&tmp).unwrap();
        let err = install(dir.path(), &script).unwrap_err();
        assert_eq!(err.code, ErrorCode::Io, "{}", err.message);
        assert!(!err.message.contains("did not write"), "{}", err.message);
        assert!(tmp.is_dir(), "left as it was");
        assert_eq!(fs::read_to_string(&path).unwrap(), script);
    }

    #[cfg(unix)]
    #[test]
    fn install_never_replaces_a_gg_it_did_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let script = launcher_script("0.1.0", None);
        let gg = dir.path().join("gg");
        let refused = |what: &str| {
            let err = install(dir.path(), &script).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadRequest);
            assert!(err.message.contains("did not write"), "{}", err.message);
            assert!(err.message.contains(what), "{} lacks {what}", err.message);
        };
        fs::write(&gg, "#!/usr/bin/env node\nconsole.log(1)\n").unwrap();
        refused("a script run by /usr/bin/env node");
        assert!(fs::read_to_string(&gg).unwrap().contains("node"));
        fs::write(&gg, b"\xcf\xfa\xed\xfe\x07\x00").unwrap();
        refused("a program");
        fs::write(&gg, "x").unwrap();
        refused("a file of 1 byte");
        fs::remove_file(&gg).unwrap();
        std::os::unix::fs::symlink("/somewhere/else/gg", &gg).unwrap();
        refused("a link to /somewhere/else/gg");
        fs::remove_file(&gg).unwrap();
        fs::create_dir(&gg).unwrap();
        refused("a folder");
        assert_eq!(installed(&path_of(&[dir.path()])), None);
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_removes_only_launchers_it_wrote() {
        let root = tempfile::tempdir().unwrap();
        let ours = mkdir(&root.path().join("ours"));
        let again = mkdir(&root.path().join("again"));
        let theirs = mkdir(&root.path().join("theirs"));
        let script = launcher_script("0.1.0", None);
        install(&ours, &script).unwrap();
        install(&again, &script).unwrap();
        fs::write(theirs.join("gg"), "#!/bin/sh\necho theirs\n").unwrap();
        let search = path_of(&[&theirs, &ours, &again]);
        assert_eq!(installed(&search), Some(written(&ours.join("gg"))));
        assert_eq!(
            uninstall(&search).unwrap(),
            [ours.join("gg"), again.join("gg")]
        );
        assert!(!ours.join("gg").exists() && !again.join("gg").exists());
        assert!(theirs.join("gg").exists());
        assert_eq!(installed(&search), None);
        // Nothing left to remove: fine.
        assert!(uninstall(&search).unwrap().is_empty());
        // A link to a launcher is not one the app wrote there: it is the command, but it is
        // never removed.
        install(&ours, &script).unwrap();
        let linked = mkdir(&root.path().join("linked"));
        std::os::unix::fs::symlink(ours.join("gg"), linked.join("gg")).unwrap();
        let search = path_of(&[&linked]);
        assert_eq!(
            installed(&search),
            Some(Installed {
                path: linked.join("gg"),
                link: Some(CliLink::Other),
            })
        );
        assert!(uninstall(&search).unwrap().is_empty());
        assert!(ours.join("gg").exists());
        assert!(is_link(&linked.join("gg")));
    }

    /// The launcher the app wrote, as [`installed`] finds it.
    #[cfg(unix)]
    fn written(path: &Path) -> Installed {
        Installed {
            path: path.to_path_buf(),
            link: None,
        }
    }

    #[cfg(unix)]
    fn is_link(path: &Path) -> bool {
        fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
    }

    /// A stand-in Homebrew install under `root`: a prefix (`brew`, with its `Caskroom`) whose
    /// `bin/gg` links to the launcher inside an app bundle in `root/Applications`, as a cask's
    /// `binary "#{appdir}/gg.app/Contents/Resources/gg"` makes it. Returns (the prefix's
    /// `bin`, the launcher in the bundle).
    #[cfg(unix)]
    fn homebrew_link(root: &Path) -> (PathBuf, PathBuf) {
        let resources = mkdir(&root.join("Applications/gg.app/Contents/Resources"));
        let launcher = resources.join("gg");
        fs::write(&launcher, launcher_script("0.3.1", None)).unwrap();
        set_executable(&launcher);
        let bin = mkdir(&root.join("brew/bin"));
        mkdir(&root.join("brew/Caskroom/gg/0.3.1"));
        std::os::unix::fs::symlink(&launcher, bin.join("gg")).unwrap();
        (bin, launcher)
    }

    #[cfg(unix)]
    #[test]
    fn a_link_homebrew_made_to_the_bundled_launcher_is_the_command() {
        let root = tempfile::tempdir().unwrap();
        let (bin, _) = homebrew_link(root.path());
        let homebrew = Installed {
            path: bin.join("gg"),
            link: Some(CliLink::Homebrew),
        };
        assert_eq!(installed(&path_of(&[&bin])), Some(homebrew.clone()));
        // A relative link, and a folder of the prefix reached through a link, count too.
        let relative = mkdir(&root.path().join("rel/brew/bin"));
        mkdir(&root.path().join("rel/brew/Caskroom"));
        std::os::unix::fs::symlink(
            "../../../Applications/gg.app/Contents/Resources/gg",
            relative.join("gg"),
        )
        .unwrap();
        assert_eq!(
            installed(&path_of(&[&relative])).unwrap().link,
            Some(CliLink::Homebrew)
        );
        // The first gg of the app's on PATH: an unknown gg before it is passed over (the
        // shell check names it), a launcher the app wrote after it is not the command.
        let other = mkdir(&root.path().join("other"));
        fs::write(other.join("gg"), "#!/bin/sh\necho other\n").unwrap();
        let mine = mkdir(&root.path().join("mine"));
        install(&mine, &launcher_script("0.3.1", None)).unwrap();
        assert_eq!(installed(&path_of(&[&other, &bin, &mine])), Some(homebrew));
        assert_eq!(
            installed(&path_of(&[&mine, &bin])),
            Some(written(&mine.join("gg")))
        );
    }

    /// Who made a link is told only when both signs agree: it lives in a Homebrew prefix's
    /// `bin` (a `Caskroom` beside it) and leads into an app bundle. Any other link to the
    /// launcher is just a link to it.
    #[cfg(unix)]
    #[test]
    fn a_link_to_the_launcher_made_by_someone_else_is_just_a_link() {
        let root = tempfile::tempdir().unwrap();
        let (_, launcher) = homebrew_link(root.path());
        // Into the bundle, from a folder that is not a Homebrew prefix's.
        let bin = mkdir(&root.path().join("home/bin"));
        std::os::unix::fs::symlink(&launcher, bin.join("gg")).unwrap();
        assert_eq!(
            installed(&path_of(&[&bin])),
            Some(Installed {
                path: bin.join("gg"),
                link: Some(CliLink::Other),
            })
        );
        // From a Homebrew prefix, to a launcher the app wrote (not in a bundle).
        let local = mkdir(&root.path().join("home/.local/bin"));
        install(&local, &launcher_script("0.3.1", None)).unwrap();
        let prefix = mkdir(&root.path().join("prefix2/bin"));
        mkdir(&root.path().join("prefix2/Caskroom"));
        std::os::unix::fs::symlink(local.join("gg"), prefix.join("gg")).unwrap();
        assert_eq!(
            installed(&path_of(&[&prefix])).unwrap().link,
            Some(CliLink::Other)
        );
    }

    /// A link that leads nowhere, to a file without the marker, or to a folder is not the
    /// app's, exactly like any other gg.
    #[cfg(unix)]
    #[test]
    fn a_link_to_anything_else_is_not_the_command() {
        let root = tempfile::tempdir().unwrap();
        let (bin, launcher) = homebrew_link(root.path());
        let search = path_of(&[&bin]);
        fs::write(&launcher, "#!/bin/sh\necho someone else\n").unwrap();
        assert_eq!(installed(&search), None);
        fs::remove_file(&launcher).unwrap();
        assert_eq!(installed(&search), None, "dangling");
        fs::create_dir(&launcher).unwrap();
        assert_eq!(installed(&search), None, "a folder");
        // A link to a link to the launcher: followed to the end.
        fs::remove_dir(&launcher).unwrap();
        let real = root.path().join("real-gg");
        fs::write(&real, launcher_script("0.3.1", None)).unwrap();
        std::os::unix::fs::symlink(&real, &launcher).unwrap();
        assert_eq!(installed(&search).unwrap().link, Some(CliLink::Other));
    }

    /// The app never replaces or removes a link to its launcher, never writes through it,
    /// and says what it is when asked to install there.
    #[cfg(unix)]
    #[test]
    fn install_and_uninstall_leave_a_link_to_the_launcher_alone() {
        let root = tempfile::tempdir().unwrap();
        let (bin, launcher) = homebrew_link(root.path());
        let bundled = fs::read(&launcher).unwrap();
        let link = bin.join("gg");
        let err = install(&bin, &launcher_script("9.9.9", None)).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert_eq!(
            err.message,
            format!(
                "There is already a gg in {}, made by Homebrew: a link to {}, gg's launcher. gg leaves it alone; choose another folder.",
                bin.display(),
                launcher.display()
            )
        );
        assert!(is_link(&link));
        assert_eq!(fs::read_link(&link).unwrap(), launcher);
        assert_eq!(fs::read(&launcher).unwrap(), bundled);
        // Removing the app's launchers leaves the link and the bundle's file; one the app
        // wrote elsewhere on PATH still goes.
        let mine = mkdir(&root.path().join("mine"));
        install(&mine, &launcher_script("0.3.1", None)).unwrap();
        let search = path_of(&[&bin, &mine]);
        assert_eq!(uninstall(&search).unwrap(), [mine.join("gg")]);
        assert!(is_link(&link));
        assert_eq!(fs::read_link(&link).unwrap(), launcher);
        assert_eq!(fs::read(&launcher).unwrap(), bundled);
        assert_eq!(installed(&search).unwrap().link, Some(CliLink::Homebrew));
    }

    /// The bundled launcher is made by `scripts/fill-launcher.sh` at release time; it must be
    /// byte for byte what [`launcher_script`] makes, executable, and the script must refuse a
    /// version that is not plain.
    #[cfg(unix)]
    #[test]
    fn the_release_fill_matches_launcher_script() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let fill = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fill-launcher.sh");
        let dir = tempfile::tempdir().unwrap();
        let run = |version: &str, out: &Path| {
            Command::new("sh")
                .arg(&fill)
                .arg(version)
                .arg(out)
                .output()
                .unwrap()
        };
        for version in [env!("CARGO_PKG_VERSION"), "1.2.3-beta.1+build.7"] {
            let out = dir.path().join(format!("gg-{version}"));
            let done = run(version, &out);
            assert!(
                done.status.success(),
                "{}",
                String::from_utf8_lossy(&done.stderr)
            );
            assert_eq!(
                fs::read_to_string(&out).unwrap(),
                launcher_script(version, None),
                "{version}"
            );
            let mode = fs::metadata(&out).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755);
            assert!(has_marker(&out));
        }
        for bad in ["", "1.0/x", "1.0&", "1 0", "1.0'", "1.0\\1"] {
            let out = dir.path().join("refused");
            let done = run(bad, &out);
            assert!(!done.status.success(), "{bad:?} was accepted");
            assert!(fs::symlink_metadata(&out).is_err(), "{bad:?} left a file");
        }
    }

    /// A stand-in login shell: a POSIX sh script that runs `setup` and then the command it
    /// is given (`shell -ilc <command>`), never the user's own shell or its files.
    #[cfg(unix)]
    fn fake_shell(dir: &Path, setup: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let shell = dir.join("fake-shell");
        fs::write(&shell, format!("#!/bin/sh\n{setup}\neval \"$2\"\n")).unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
        shell
    }

    #[cfg(unix)]
    #[test]
    fn shadowed_by_names_an_alias_a_function_or_an_earlier_program() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "PATH=/usr/bin:/bin\necho 'profile noise'\nalias gg='git log --graph'",
        );
        assert_eq!(shadowed_by(&shell).unwrap(), "an alias for git log --graph");

        let shell = fake_shell(dir.path(), "PATH=/usr/bin:/bin\ngg() { :; }");
        assert!(
            shadowed_by(&shell).unwrap().contains("function"),
            "{:?}",
            shadowed_by(&shell)
        );

        let other = mkdir(&dir.path().join("other"));
        fs::write(other.join("gg"), "#!/bin/sh\n").unwrap();
        set_executable(&other.join("gg"));
        let setup = format!("PATH='{}':/usr/bin:/bin", other.display());
        let shell = fake_shell(dir.path(), &setup);
        assert_eq!(
            shadowed_by(&shell).unwrap(),
            format!("the program {}", other.join("gg").display())
        );

        // The launcher the app wrote is not something else.
        fs::remove_file(other.join("gg")).unwrap();
        install(&other, &launcher_script("0.1.0", None)).unwrap();
        assert_eq!(shadowed_by(&shell), None);

        // Nor is a link to the launcher (Homebrew's); a link that leads nowhere, or to
        // something else, is another gg.
        let (bin, launcher) = homebrew_link(&dir.path().join("brew-root"));
        let shell = fake_shell(
            dir.path(),
            &format!("PATH='{}':/usr/bin:/bin", bin.display()),
        );
        assert_eq!(shadowed_by(&shell), None);
        fs::write(&launcher, "#!/bin/sh\necho someone else\n").unwrap();
        assert_eq!(
            shadowed_by(&shell).unwrap(),
            format!("the program {}", bin.join("gg").display())
        );
        fs::remove_file(&launcher).unwrap();
        assert_eq!(
            describe(&format!("gg is {}", bin.join("gg").display())).unwrap(),
            format!("the program {}", bin.join("gg").display())
        );

        // Nothing called gg.
        let shell = fake_shell(dir.path(), "PATH=/usr/bin:/bin");
        assert_eq!(shadowed_by(&shell), None);
    }

    #[cfg(unix)]
    fn set_executable(file: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn shadowed_by_gives_up_on_a_slow_or_broken_shell() {
        let dir = tempfile::tempdir().unwrap();
        let slow = fake_shell(dir.path(), "sleep 10");
        let started = std::time::Instant::now();
        assert_eq!(shadowed_by_within(&slow, Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
        let failing = fake_shell(dir.path(), "exit 3");
        assert_eq!(shadowed_by(&failing), None);
        assert_eq!(shadowed_by(&dir.path().join("no-such-shell")), None);
    }
}
