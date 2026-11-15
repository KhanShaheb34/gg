//! Starting programs outside the app for the user: the editor (a worktree's files, to resolve
//! conflicts, or a worktree's folder), a terminal in a worktree, the file manager showing a
//! worktree, and the browser for a web page (a pull request).
//!
//! Programs run as argument arrays, never through a shell (no `cmd.exe` is ever given an
//! argument that comes from a repository), in a process group of their own, not waited for.
//! No file of a repository is ever handed to the system's default application, which picks a
//! program by file type and could run it (R19). The user's own commands (editor, terminal)
//! are split into words with simple quotes, and the path is one argument.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::{CoreError, ErrorCode, Result};
use crate::git::worktrees::read_worktrees;
use crate::repo::{RepoManager, canonical, canonical_paths};

/// What `open_external` opens: a worktree in the editor, a terminal in it, the file manager
/// showing it, or a web page (`url`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum External {
    Editor,
    Terminal,
    Reveal,
    Url,
}

/// The programs the user configured in the settings (`None`: the platform's own).
#[derive(Debug, Clone, Copy, Default)]
pub struct Programs<'a> {
    pub editor: Option<&'a str>,
    pub terminal: Option<&'a str>,
}

impl RepoManager {
    /// Opens `path` (relative to the active worktree; the worktree folder itself when
    /// `None`) with `command` (the editor command from the settings: a program and its
    /// arguments, split on whitespace with simple quotes, the path added as the last
    /// argument). Without a command, a folder opens with the platform's opener, and a file
    /// opens as text on macOS; elsewhere the worktree folder opens instead and the returned
    /// note says so (see [`launch_plan`]). The program is started directly, never through
    /// a shell, and not waited for. Returns a note for the user, if any.
    ///
    /// `bad_request` for a path that does not stay inside the worktree (`..`, an absolute
    /// path, a symlink that leads out) or a command with an unclosed quote; `path_missing`
    /// for a file that does not exist; `io` if the program cannot be started.
    pub async fn open_in_editor(
        &self,
        repo_id: u32,
        path: Option<&str>,
        command: Option<&str>,
    ) -> Result<Option<String>> {
        let worktree = self.get(repo_id)?.worktree();
        let root = dunce::canonicalize(&worktree).unwrap_or_else(|_| worktree.clone());
        let file = path.map(|p| inside_worktree(&root, p)).transpose()?;
        let target = file.as_deref().map(|path| Target {
            path,
            regular: path.metadata().is_ok_and(|m| m.is_file()),
        });
        let plan = launch_plan(std::env::consts::OS, command, &root, target)?;
        self.start(plan, Some("editor"))
    }

    /// Opens worktree `path` of repository `repo_id` (any worktree git lists for it, read
    /// afresh; the active one when `None`) in the editor, in a terminal or in the file
    /// manager, or (`url`) the web page `url` in the browser. Returns a note for the user, if
    /// any.
    ///
    /// The editor and the terminal are `programs` (see [`terminal_plan`] for the terminal
    /// found without one). A web page must be an `https://` address; it is parsed and written
    /// out again (no spaces, no control characters, no user name or password in it) before
    /// the system's opener gets it.
    ///
    /// `bad_request` for a path that is no worktree of the repository, a missing or wrong
    /// address, or an argument that does not belong to the target; `path_missing` for a
    /// worktree whose folder is gone; `io` if no program could be started.
    pub async fn open_external(
        &self,
        repo_id: u32,
        target: External,
        path: Option<&Path>,
        url: Option<&str>,
        programs: Programs<'_>,
    ) -> Result<Option<String>> {
        let os = std::env::consts::OS;
        let bad = |message: &str| CoreError::new(ErrorCode::BadRequest, message.to_owned());
        if target == External::Url {
            self.get(repo_id)?;
            if path.is_some() {
                return Err(bad("A web page is opened by its address, not a path."));
            }
            let url = url.ok_or_else(|| bad("No web address to open."))?;
            return self.start(url_plan(os, &web_address(url)?), None);
        }
        if url.is_some() {
            return Err(bad("A worktree is opened by its path, not a web address."));
        }
        let dir = self.worktree_folder(repo_id, path).await?;
        let login_path = self.login_path();
        let (plan, setting) = match target {
            External::Editor => (launch_plan(os, programs.editor, &dir, None)?, "editor"),
            External::Terminal => {
                let find = |name: &str| find_program(OsStr::new(name), login_path.as_ref());
                (
                    terminal_plan(os, programs.terminal, &dir, &find)?,
                    "terminal",
                )
            }
            External::Reveal => return self.start(reveal_plan(os, &dir), None),
            External::Url => unreachable!("handled above"),
        };
        self.start(plan, Some(setting))
    }

    /// The folder of worktree `path` of repository `repo_id` (canonical), if git lists it
    /// now; the active worktree's when `None`.
    async fn worktree_folder(&self, repo_id: u32, path: Option<&Path>) -> Result<PathBuf> {
        let repo = self.get(repo_id)?;
        let Some(path) = path else {
            let active = repo.worktree();
            return Ok(dunce::canonicalize(&active).unwrap_or(active));
        };
        let target = canonical(path);
        let listed = canonical_paths(read_worktrees(&repo.runner, &repo.root).await?);
        let Some(wt) = listed.iter().find(|w| Path::new(&w.path) == target) else {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!("{} is not a worktree of this repository.", path.display()),
            ));
        };
        if wt.bare {
            return Err(CoreError::new(
                ErrorCode::BadRequest,
                format!(
                    "{} is the bare repository itself, which has no folder to open.",
                    path.display()
                ),
            ));
        }
        if wt.prunable || !target.is_dir() {
            return Err(CoreError::new(
                ErrorCode::PathMissing,
                format!(
                    "The worktree {} is gone (its folder is missing).",
                    path.display()
                ),
            ));
        }
        Ok(target)
    }

    /// Runs `plan` (or records it, see [`RepoManager::record_launches_to`]) and returns its
    /// note. `setting`: the setting that names the program, for the message when it cannot
    /// be found ("editor", "terminal"); `None` for the platform's own programs.
    fn start(&self, plan: Plan, setting: Option<&str>) -> Result<Option<String>> {
        let Plan {
            argv,
            cwd,
            note,
            console,
        } = plan;
        #[cfg(any(test, feature = "testutil"))]
        if let Some(launches) = crate::lock_ignoring_poison(&self.launches).as_mut() {
            launches.push(
                argv.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
            );
            return Ok(note);
        }
        let record = crate::lock_ignoring_poison(&self.launch_record).clone();
        match record {
            Some(file) => record_launch(&file, &argv)?,
            None => launch(&argv, &cwd, self.login_path(), console, setting)?,
        }
        Ok(note)
    }

    /// From now on, launches (editor, terminal, file manager, browser) are appended to
    /// `file` (one JSON array of the program and its arguments per line) instead of run;
    /// `None` runs them again. For the dev server, so that the UI's tests never start a
    /// program.
    pub fn record_launches_to(&self, file: Option<PathBuf>) {
        *crate::lock_ignoring_poison(&self.launch_record) = file;
    }

    /// From now on, launches are recorded instead of run (tests only; see
    /// [`RepoManager::launches`]).
    #[cfg(any(test, feature = "testutil"))]
    pub fn record_launches(&self) {
        crate::lock_ignoring_poison(&self.launches).get_or_insert_with(Vec::new);
    }

    /// The argument vectors of the launches recorded so far (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn launches(&self) -> Vec<Vec<String>> {
        crate::lock_ignoring_poison(&self.launches)
            .clone()
            .unwrap_or_default()
    }
}

/// Appends `argv` to `file` as a line of JSON (see [`RepoManager::record_launches_to`]).
fn record_launch(file: &Path, argv: &[OsString]) -> Result<()> {
    use std::io::Write as _;
    let args: Vec<String> = argv
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let line = format!("{}\n", serde_json::Value::from(args));
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| {
            CoreError::new(ErrorCode::Io, "Could not record the launch").with_detail(e.to_string())
        })
}

/// `rel` (a path relative to the worktree `root`, canonical), resolved, if it stays inside
/// it: no `..`, no absolute path, and no symlink leading out.
fn inside_worktree(root: &Path, rel: &str) -> Result<PathBuf> {
    let outside = || {
        CoreError::new(
            ErrorCode::BadRequest,
            format!("\u{201c}{rel}\u{201d} is not a path inside the worktree."),
        )
    };
    let path = Path::new(rel);
    let plain = path
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if rel.is_empty() || !plain {
        return Err(outside());
    }
    let resolved = dunce::canonicalize(root.join(path))
        .map_err(|_| CoreError::new(ErrorCode::PathMissing, format!("File not found: {rel}")))?;
    if !resolved.starts_with(root) {
        return Err(outside());
    }
    Ok(resolved)
}

/// `text` as the `https://` address the browser gets: parsed and written out again, so what
/// is opened is exactly a web address. Refused: any other scheme, spaces or control
/// characters (which a parser would quietly drop), no host, a user name or password.
fn web_address(text: &str) -> Result<String> {
    let refused = |why: &str| CoreError::new(ErrorCode::BadRequest, format!("Not opened: {why}."));
    if text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(refused(
            "the web address has spaces or control characters in it",
        ));
    }
    let url = url::Url::parse(text).map_err(|_| refused("that is not a web address"))?;
    if url.scheme() != "https" {
        return Err(refused("only https:// web addresses are opened"));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(refused("the web address has no host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(refused("the web address has a user name or password in it"));
    }
    Ok(url.to_string())
}

/// Said when a file was asked for, no editor command is set, and the worktree folder was
/// opened instead.
const FOLDER_INSTEAD: &str = "No editor command is set, so the worktree folder was opened \
                              instead of the file: the system's default application for a \
                              file from a repository could run it. Set an editor command in \
                              Settings to open files.";

/// Said when no terminal program is found on Linux and no terminal command is set.
const NO_TERMINAL: &str = "No terminal program was found (x-terminal-emulator, gnome-terminal, \
                           konsole or xterm), so nothing was opened. Set a terminal command in \
                           Settings.";

/// What was asked to be opened: a path inside the worktree, and whether it is a regular file
/// (not a folder, which an `X.app` bundle is).
#[derive(Debug, Clone, Copy)]
struct Target<'a> {
    path: &'a Path,
    regular: bool,
}

/// How to open something: the program and its arguments, the folder it runs in, and a
/// note for the user if something else than asked was opened.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    argv: Vec<OsString>,
    /// The worktree for the user's own program or a terminal (it starts there). For the
    /// platform's opener a folder outside any repository, so that a program of that name in
    /// the worktree is never the one run (a relative entry in `PATH` resolves against it).
    cwd: PathBuf,
    note: Option<String>,
    /// Windows: the program gets a console window of its own (`cmd.exe`).
    console: bool,
}

impl Plan {
    /// The platform's own program `argv`, run from outside any repository.
    fn system(os: &str, argv: Vec<OsString>) -> Self {
        let cwd = if os == "windows" {
            windows_dir()
        } else {
            PathBuf::from("/")
        };
        Self {
            argv,
            cwd,
            note: None,
            console: false,
        }
    }
}

/// The platform's opener: `/usr/bin/open` on macOS, `explorer.exe` in the Windows folder,
/// `xdg-open` elsewhere (found through `PATH`).
fn opener(os: &str) -> OsString {
    match os {
        "macos" => "/usr/bin/open".into(),
        "windows" => windows_dir().join("explorer.exe").into_os_string(),
        _ => "xdg-open".into(),
    }
}

/// The opener's arguments that open folder `dir`. On macOS a folder whose name has an
/// extension may be a bundle (`X.app`), which `open` would start: it is shown in its folder
/// instead (`open -R`), which never starts anything.
fn folder_argv(os: &str, dir: &Path) -> Vec<OsString> {
    if os == "macos" && dir.extension().is_some() {
        return vec![opener(os), "-R".into(), dir.into()];
    }
    vec![opener(os), dir.into()]
}

/// The program and arguments that open `file` (`None`: the worktree folder `root`) on
/// operating system `os` (`std::env::consts::OS`), where they run, and a note for the user
/// if something else than asked was opened.
///
/// With `command` (its words, then the path) the editor gets the file. Without one, no
/// file of the repository ever goes to the system's default application, which picks a
/// program by file type and could run it (a `.command`, `.sh`, `.exe`, `.desktop`): on
/// macOS a regular file opens as text (`open -t`); otherwise (a folder such as an `X.app`
/// bundle, which `open` would start, or any file elsewhere) the worktree folder opens
/// instead, with the note. Asked for the worktree itself (`.`), it opens without one.
///
/// The opener gets the path as a plain argument; no shell (`cmd.exe`) is ever involved.
fn launch_plan(os: &str, command: Option<&str>, root: &Path, file: Option<Target>) -> Result<Plan> {
    let words = command
        .map(|c| split_command(c, "editor"))
        .transpose()?
        .unwrap_or_default();
    if !words.is_empty() {
        let mut argv: Vec<OsString> = words.into_iter().map(OsString::from).collect();
        argv.push(file.map_or(root, |f| f.path).as_os_str().to_owned());
        return Ok(Plan {
            argv,
            cwd: root.to_owned(),
            note: None,
            console: false,
        });
    }
    // The worktree itself is the folder, not a file.
    let file = file.filter(|f| f.path != root);
    let (argv, note) = match file {
        Some(file) if os == "macos" && file.regular => (
            vec![opener(os), "-t".into(), file.path.as_os_str().to_owned()],
            None,
        ),
        Some(_) => (folder_argv(os, root), Some(FOLDER_INSTEAD.to_owned())),
        None => (folder_argv(os, root), None),
    };
    Ok(Plan {
        note,
        ..Plan::system(os, argv)
    })
}

/// The program and arguments that open a terminal in folder `dir` on `os`.
///
/// With `command` (the terminal command from the settings), its words with `{path}` replaced
/// by the folder, or the folder added as the last argument when no word has `{path}`; it
/// starts in the folder. Without one: macOS `open -a Terminal <dir>`; Windows `wt.exe -d
/// <dir>` (Windows Terminal) when `find` finds it and the folder has no `;` (which it takes
/// as the start of another command), else `cmd.exe /K` started in the folder in a console of
/// its own, given nothing from the repository; elsewhere the first of `x-terminal-emulator`
/// (started in the folder), `gnome-terminal --working-directory=<dir>`, `konsole --workdir
/// <dir>` and `xterm` (started in the folder) that `find` finds, else `io` with a plain
/// message. `find` looks a program up by name (only in absolute `PATH` entries).
fn terminal_plan(
    os: &str,
    command: Option<&str>,
    dir: &Path,
    find: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Plan> {
    let words = command
        .map(|c| split_command(c, "terminal"))
        .transpose()?
        .unwrap_or_default();
    let in_dir = |argv: Vec<OsString>| Plan {
        argv,
        cwd: dir.to_owned(),
        note: None,
        console: false,
    };
    if !words.is_empty() {
        let placed = words.iter().any(|w| w.contains("{path}"));
        let mut argv: Vec<OsString> = words
            .iter()
            .map(|w| with_path(w, dir.as_os_str()))
            .collect();
        if !placed {
            argv.push(dir.into());
        }
        return Ok(in_dir(argv));
    }
    match os {
        "macos" => Ok(Plan::system(
            os,
            vec![opener(os), "-a".into(), "Terminal".into(), dir.into()],
        )),
        "windows" => {
            let wt = find("wt.exe").filter(|_| !dir.as_os_str().to_string_lossy().contains(';'));
            Ok(match wt {
                Some(wt) => Plan::system(os, vec![wt.into(), "-d".into(), dir.into()]),
                None => Plan {
                    argv: vec![
                        windows_dir().join("System32").join("cmd.exe").into(),
                        "/K".into(),
                    ],
                    console: true,
                    ..in_dir(Vec::new())
                },
            })
        }
        _ => {
            let mut working_directory = OsString::from("--working-directory=");
            working_directory.push(dir);
            let candidates: [(&str, Vec<OsString>); 4] = [
                ("x-terminal-emulator", Vec::new()),
                ("gnome-terminal", vec![working_directory]),
                ("konsole", vec!["--workdir".into(), dir.into()]),
                ("xterm", Vec::new()),
            ];
            for (name, args) in candidates {
                if let Some(program) = find(name) {
                    let mut argv = vec![program.into_os_string()];
                    argv.extend(args);
                    return Ok(in_dir(argv));
                }
            }
            Err(CoreError::new(ErrorCode::Io, NO_TERMINAL))
        }
    }
}

/// `word` with every `{path}` in it replaced by `path`.
fn with_path(word: &str, path: &OsStr) -> OsString {
    let mut out = OsString::new();
    let mut parts = word.split("{path}");
    if let Some(first) = parts.next() {
        out.push(first);
    }
    for part in parts {
        out.push(path);
        out.push(part);
    }
    out
}

/// The program and arguments that show folder `dir` in the file manager on `os`: selected
/// in its folder (macOS `open -R`, Windows `explorer.exe /select,`), or its parent folder
/// opened (`xdg-open`).
fn reveal_plan(os: &str, dir: &Path) -> Plan {
    let argv = match os {
        "macos" => vec![opener(os), "-R".into(), dir.into()],
        "windows" => vec![opener(os), "/select,".into(), dir.into()],
        _ => vec![opener(os), dir.parent().unwrap_or(dir).into()],
    };
    Plan::system(os, argv)
}

/// The program and arguments that open web address `url` (checked, see [`web_address`]) in
/// the browser on `os`: `open`, `xdg-open`, or on Windows `rundll32 url.dll,
/// FileProtocolHandler` (never `cmd /c start`).
fn url_plan(os: &str, url: &str) -> Plan {
    let argv = match os {
        "windows" => vec![
            windows_dir().join("System32").join("rundll32.exe").into(),
            "url.dll,FileProtocolHandler".into(),
            url.into(),
        ],
        _ => vec![opener(os), url.into()],
    };
    Plan::system(os, argv)
}

/// The Windows folder (`%SystemRoot%`, `C:\Windows` if unset).
fn windows_dir() -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

/// `command` split into words on whitespace; text in single or double quotes is one word
/// (or part of one), taken literally. Nothing else is special: no escapes, no variables.
/// `setting`: which command it is ("editor", "terminal"), for the message.
fn split_command(command: &str, setting: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote = None;
    for c in command.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return Err(CoreError::new(
            ErrorCode::BadRequest,
            format!("The {setting} command has a quote that is not closed. Check it in Settings."),
        ));
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// The program a bare name (`code`, no folder in it) stands for: the first executable file
/// of that name in an absolute entry of `path` (the given `PATH`, else the app's). A relative
/// entry (`.`, `bin`, an empty one) is skipped: the editor runs in the worktree, where it
/// would find a program of the repository's (branch review Minor 7). A name with a folder in
/// it is the user's own choice, taken as it is. `None` if no entry has it.
#[cfg(unix)]
fn resolve_program(program: &OsStr, path: Option<&OsString>) -> Option<PathBuf> {
    let name = Path::new(program);
    if name.components().count() != 1 || name.is_absolute() {
        return Some(name.to_owned());
    }
    find_program(program, path)
}

/// The first program called `name` in an absolute entry of `path` (the given `PATH`, else
/// the app's): an executable file on Unix; on Windows anything there (Windows Terminal's
/// `wt.exe` is an alias the file system cannot read as a file).
fn find_program(name: &OsStr, path: Option<&OsString>) -> Option<PathBuf> {
    let path = path.cloned().or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| runnable(candidate))
}

#[cfg(unix)]
fn runnable(candidate: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    candidate
        .metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn runnable(candidate: &Path) -> bool {
    candidate.symlink_metadata().is_ok()
}

/// Starts `argv` in `cwd` with no input or output, in a process group of its own (so it
/// outlives a terminal's Ctrl-C), and does not wait for it; a thread reaps it when it ends.
/// `path` is the `PATH` to find the program with (the login shell's), if known; only its
/// absolute entries (see `resolve_program`). `console`: on Windows, a console window of its
/// own. `setting`: the setting that names the program, for the message when it is missing.
fn launch(
    argv: &[OsString],
    cwd: &Path,
    path: Option<OsString>,
    console: bool,
    setting: Option<&str>,
) -> Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Err(CoreError::new(ErrorCode::BadRequest, "Nothing to run."));
    };
    let name = program.to_string_lossy().into_owned();
    let missing = || {
        let fix = setting.map_or(String::new(), |s| {
            format!(" Set the {s} command in Settings.")
        });
        CoreError::new(
            ErrorCode::Io,
            format!(
                "The program \u{201c}{name}\u{201d} was not found, so nothing was opened.{fix}"
            ),
        )
    };
    #[cfg(unix)]
    let resolved = resolve_program(program, path.as_ref()).ok_or_else(missing)?;
    #[cfg(not(unix))]
    let resolved = program;
    let mut cmd = Command::new(resolved);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let _ = console;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// A console window of its own (`cmd.exe`), else a new process group.
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(if console {
            CREATE_NEW_CONSOLE
        } else {
            CREATE_NEW_PROCESS_GROUP
        });
    }
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(missing().with_detail(e.to_string()))
        }
        Err(e) => Err(CoreError::new(
            ErrorCode::Io,
            format!("Could not start \u{201c}{name}\u{201d}."),
        )
        .with_detail(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bare program name is never found through a relative `PATH` entry, which would
    /// resolve inside the worktree the editor runs in (branch review Minor 7).
    #[cfg(unix)]
    #[test]
    fn a_program_is_never_found_through_a_relative_path_entry() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = bin.join("my-editor");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let name = OsString::from("my-editor");

        let relative = OsString::from("bin:.:");
        assert_eq!(resolve_program(&name, Some(&relative)), None);
        let absolute = std::env::join_paths([Path::new("bin"), bin.as_path()]).unwrap();
        assert_eq!(resolve_program(&name, Some(&absolute)), Some(tool.clone()));
        // A path the user gave is theirs.
        let given = tool.clone().into_os_string();
        assert_eq!(resolve_program(&given, Some(&relative)), Some(tool));
        // Not executable: not it.
        std::fs::write(bin.join("plain"), "x").unwrap();
        assert_eq!(resolve_program(OsStr::new("plain"), Some(&absolute)), None);
    }

    #[test]
    fn commands_split_on_whitespace_with_simple_quotes() {
        let split = |c: &str| split_command(c, "editor").unwrap();
        assert_eq!(split("code -w"), ["code", "-w"]);
        assert_eq!(split("  subl\t--wait  "), ["subl", "--wait"]);
        assert_eq!(
            split(r#""/Applications/My Editor.app/x" 'a "b"' c"d e"f"#),
            ["/Applications/My Editor.app/x", "a \"b\"", "cd ef"]
        );
        assert_eq!(split("x '' y"), ["x", "", "y"]);
        assert_eq!(split("a\\ b $HOME"), ["a\\", "b", "$HOME"]);
        assert!(split("").is_empty());
        let err = split_command("vim \"x", "terminal").unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest);
        assert!(err.message.contains("terminal command"), "{err:?}");
    }

    /// On macOS without an editor command, only a regular file goes to `open -t`: a folder
    /// inside the worktree (an `X.app` bundle, which `open` would start) opens the worktree
    /// folder instead, with the note; the worktree itself (`.`) opens without one.
    #[test]
    fn folders_never_go_to_open_as_text() {
        let Plan { argv, note, .. } = launch_plan(
            "macos",
            None,
            Path::new("/r"),
            Some(Target {
                path: Path::new("/r/X.app"),
                regular: false,
            }),
        )
        .unwrap();
        assert_eq!(argv, ["/usr/bin/open", "/r"]);
        assert!(note.is_some());
        let Plan { argv, note, .. } = launch_plan(
            "macos",
            None,
            Path::new("/r"),
            Some(Target {
                path: Path::new("/r"),
                regular: false,
            }),
        )
        .unwrap();
        assert_eq!(argv, ["/usr/bin/open", "/r"]);
        assert!(note.is_none());
    }

    /// A worktree folder whose name has an extension (`fix.app`, made for a branch of that
    /// name) could be a bundle `open` would start: on macOS it is shown in its folder instead.
    #[test]
    fn a_worktree_folder_that_could_be_a_bundle_is_shown_not_opened() {
        let root = Path::new("/w/fix.app");
        let Plan { argv, note, .. } = launch_plan("macos", None, root, None).unwrap();
        assert_eq!(argv, ["/usr/bin/open", "-R", "/w/fix.app"]);
        assert!(note.is_none());
        let file = Target {
            path: Path::new("/w/fix.app/run.sh"),
            regular: false,
        };
        let Plan { argv, .. } = launch_plan("macos", None, root, Some(file)).unwrap();
        assert_eq!(argv, ["/usr/bin/open", "-R", "/w/fix.app"]);
        // Elsewhere a folder opens in the file manager whatever its name.
        let Plan { argv, .. } = launch_plan("linux", None, root, None).unwrap();
        assert_eq!(argv, ["xdg-open", "/w/fix.app"]);
    }

    /// The argv (as strings) and whether there is a note.
    fn plan_on(os: &str, command: Option<&str>, file: Option<&str>) -> (Vec<String>, bool) {
        let target = file.map(|f| Target {
            path: Path::new(f),
            regular: true,
        });
        let Plan { argv, note, .. } = launch_plan(os, command, Path::new("/r"), target).unwrap();
        (strings(&argv), note.is_some())
    }

    fn strings(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn explorer() -> String {
        windows_dir()
            .join("explorer.exe")
            .to_string_lossy()
            .into_owned()
    }

    /// The platform's opener never runs in the worktree, where a program of its name could
    /// be found through a relative `PATH` entry; an editor command does (it may take paths
    /// relative to it).
    #[test]
    fn openers_run_from_outside_the_worktree() {
        let root = Path::new("/r");
        for os in ["macos", "linux", "windows"] {
            let mut plans = vec![reveal_plan(os, root), url_plan(os, "https://example.com/")];
            for file in [
                None,
                Some(Target {
                    path: Path::new("/r/a.txt"),
                    regular: true,
                }),
            ] {
                plans.push(launch_plan(os, None, root, file).unwrap());
            }
            for plan in plans {
                assert_ne!(plan.cwd, root, "{os}");
                assert!(plan.cwd.is_absolute() || os == "windows", "{os}: {plan:?}");
                assert!(!plan.cwd.starts_with(root), "{os}");
                let program = Path::new(&plan.argv[0]);
                assert!(program.is_absolute() || os != "macos", "{plan:?}");
            }
            let plan = launch_plan(os, Some("code"), root, None).unwrap();
            assert_eq!(plan.cwd, root);
        }
    }

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Without an editor command no file of the repository goes to the system's default
    /// application, which could run it (a `.command`, `.sh`, `.exe`, `.desktop`): macOS
    /// opens it as text, elsewhere the worktree folder is opened instead, with a note. No
    /// shell (`cmd.exe`) anywhere.
    #[test]
    fn without_an_editor_command_files_never_go_to_the_default_application() {
        let file = Some("/r/x.command");
        assert_eq!(
            plan_on("macos", None, file),
            (words(&["/usr/bin/open", "-t", "/r/x.command"]), false)
        );
        assert_eq!(
            plan_on("linux", None, file),
            (words(&["xdg-open", "/r"]), true)
        );
        assert_eq!(
            plan_on("windows", None, file),
            (words(&[&explorer(), "/r"]), true)
        );
        // A blank command is no command.
        assert_eq!(
            plan_on("linux", Some("  "), file),
            (words(&["xdg-open", "/r"]), true)
        );
        // The worktree folder itself opens as before.
        assert_eq!(
            plan_on("macos", None, None),
            (words(&["/usr/bin/open", "/r"]), false)
        );
        assert_eq!(
            plan_on("linux", None, None),
            (words(&["xdg-open", "/r"]), false)
        );
        assert_eq!(
            plan_on("windows", None, None),
            (words(&[&explorer(), "/r"]), false)
        );
        // With a command, the file goes to it, on every platform.
        for os in ["macos", "linux", "windows"] {
            assert_eq!(
                plan_on(os, Some("code -w"), file),
                (words(&["code", "-w", "/r/x.command"]), false)
            );
            assert_eq!(
                plan_on(os, Some("code"), None),
                (words(&["code", "/r"]), false)
            );
        }
    }

    /// A lookup that finds exactly `names`, in `/usr/bin`.
    fn finding(names: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| {
            names
                .contains(&name)
                .then(|| Path::new("/usr/bin").join(name))
        }
    }

    fn terminal(
        os: &str,
        command: Option<&str>,
        dir: &str,
        found: &'static [&'static str],
    ) -> Result<Plan> {
        terminal_plan(os, command, Path::new(dir), &finding(found))
    }

    /// Without a terminal command: Terminal on macOS; on Linux the first terminal found of
    /// x-terminal-emulator, gnome-terminal, konsole and xterm, each started in the folder,
    /// else a plain message; on Windows Windows Terminal, else `cmd.exe /K` started in the
    /// folder with nothing from the repository in its arguments.
    #[test]
    fn a_terminal_without_a_setting_is_the_platforms() {
        let mac = terminal("macos", None, "/w/a b", &[]).unwrap();
        assert_eq!(
            strings(&mac.argv),
            ["/usr/bin/open", "-a", "Terminal", "/w/a b"]
        );
        assert_eq!(mac.cwd, Path::new("/"));

        let all = &["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"];
        let plan = terminal("linux", None, "/w/a b", all).unwrap();
        assert_eq!(strings(&plan.argv), ["/usr/bin/x-terminal-emulator"]);
        assert_eq!(plan.cwd, Path::new("/w/a b"));
        let plan = terminal(
            "linux",
            None,
            "/w/a b",
            &["gnome-terminal", "konsole", "xterm"],
        )
        .unwrap();
        assert_eq!(
            strings(&plan.argv),
            ["/usr/bin/gnome-terminal", "--working-directory=/w/a b"]
        );
        let plan = terminal("linux", None, "/w/a b", &["konsole", "xterm"]).unwrap();
        assert_eq!(
            strings(&plan.argv),
            ["/usr/bin/konsole", "--workdir", "/w/a b"]
        );
        let plan = terminal("linux", None, "/w/a b", &["xterm"]).unwrap();
        assert_eq!(strings(&plan.argv), ["/usr/bin/xterm"]);
        assert_eq!(plan.cwd, Path::new("/w/a b"));
        let err = terminal("linux", None, "/w", &[]).unwrap_err();
        assert_eq!(err.code, ErrorCode::Io);
        assert!(err.message.contains("Settings"), "{err:?}");

        let plan = terminal("windows", None, r"C:\w\a b", &["wt.exe"]).unwrap();
        assert_eq!(
            strings(&plan.argv),
            [
                Path::new("/usr/bin")
                    .join("wt.exe")
                    .to_string_lossy()
                    .as_ref(),
                "-d",
                r"C:\w\a b"
            ]
        );
        assert!(!plan.console);
        let cmd = windows_dir().join("System32").join("cmd.exe");
        for (dir, found) in [(r"C:\w\a b", &[][..]), (r"C:\w\x;calc", &["wt.exe"][..])] {
            let plan = terminal_plan("windows", None, Path::new(dir), &|name| {
                found.contains(&name).then(|| PathBuf::from(name))
            })
            .unwrap();
            assert_eq!(plan.argv, [cmd.as_os_str(), OsStr::new("/K")], "{dir}");
            assert_eq!(plan.cwd, Path::new(dir));
            assert!(plan.console);
        }
    }

    /// A terminal command is split as the editor command is, never run through a shell; the
    /// folder replaces `{path}` (inside a word too), else it is the last argument; it starts
    /// in the folder.
    #[test]
    fn a_terminal_command_gets_the_folder_as_one_argument() {
        for os in ["macos", "linux", "windows"] {
            let plan = terminal(
                os,
                Some("alacritty --working-directory {path}"),
                "/w/a b",
                &[],
            )
            .unwrap();
            assert_eq!(
                strings(&plan.argv),
                ["alacritty", "--working-directory", "/w/a b"]
            );
            assert_eq!(plan.cwd, Path::new("/w/a b"));
            let plan =
                terminal(os, Some("wezterm start --cwd={path} ; rm"), "/w/a b", &[]).unwrap();
            assert_eq!(
                strings(&plan.argv),
                ["wezterm", "start", "--cwd=/w/a b", ";", "rm"]
            );
            let plan = terminal(os, Some("'my term' -x"), "/w/$(x)", &[]).unwrap();
            assert_eq!(strings(&plan.argv), ["my term", "-x", "/w/$(x)"]);
            assert!(!plan.console);
            // Blank: as if unset.
            assert!(terminal(os, Some("  "), "/w", &["xterm", "wt.exe"]).is_ok());
            let err = terminal(os, Some("term 'x"), "/w", &[]).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadRequest);
        }
    }

    /// Reveal shows the folder selected in its parent (macOS, Windows), or opens the parent.
    #[test]
    fn reveal_shows_the_folder_in_the_file_manager() {
        let dir = Path::new("/w/a b");
        assert_eq!(
            strings(&reveal_plan("macos", dir).argv),
            ["/usr/bin/open", "-R", "/w/a b"]
        );
        assert_eq!(
            strings(&reveal_plan("windows", dir).argv),
            [explorer().as_str(), "/select,", "/w/a b"]
        );
        assert_eq!(strings(&reveal_plan("linux", dir).argv), ["xdg-open", "/w"]);
    }

    /// Only `https://` addresses are opened, as written out again by the parser; never
    /// through `cmd /c start` on Windows.
    #[test]
    fn only_https_addresses_go_to_the_browser() {
        assert_eq!(
            web_address("https://github.com/o/r/pull/12").unwrap(),
            "https://github.com/o/r/pull/12"
        );
        assert_eq!(
            web_address("HTTPS://GitHub.com/o/r/compare/main...x?expand=1").unwrap(),
            "https://github.com/o/r/compare/main...x?expand=1"
        );
        // Characters a shell or a command line would act on reach the browser encoded.
        assert_eq!(
            web_address("https://h/a\"b<c>|d^e&f").unwrap(),
            "https://h/a%22b%3Cc%3E|d^e&f"
        );
        for refused in [
            "http://github.com/o/r",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ssh://git@github.com/o/r",
            "https://user:secret@github.com/o/r",
            "https://token@github.com/o/r",
            "https://github.com/o r",
            "https://github.com/o\nr",
            " https://github.com/o/r",
            "https://github.com/o/r\u{7}",
            "github.com/o/r",
            "",
        ] {
            let err = web_address(refused).expect_err(refused);
            assert_eq!(err.code, ErrorCode::BadRequest, "{refused}");
        }
        let url = "https://github.com/o/r/pull/12";
        assert_eq!(
            strings(&url_plan("macos", url).argv),
            ["/usr/bin/open", url]
        );
        assert_eq!(strings(&url_plan("linux", url).argv), ["xdg-open", url]);
        let windows = url_plan("windows", url);
        let rundll = windows_dir().join("System32").join("rundll32.exe");
        assert_eq!(
            strings(&windows.argv),
            [
                rundll.to_string_lossy().as_ref(),
                "url.dll,FileProtocolHandler",
                url
            ]
        );
        assert!(!windows.console);
    }
}
