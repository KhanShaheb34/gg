//! Opening a worktree's files (or the worktree folder) for the user, to resolve conflicts:
//! with the editor command from the settings, else with the platform's opener, never through
//! a shell, and never handing a file of the repository to the system's default application.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{CoreError, ErrorCode, Result};
use crate::repo::RepoManager;

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
        let worktree = self.get(repo_id)?.worktree.clone();
        let root = dunce::canonicalize(&worktree).unwrap_or_else(|_| worktree.clone());
        let file = path.map(|p| inside_worktree(&root, p)).transpose()?;
        let target = file.as_deref().map(|path| Target {
            path,
            regular: path.metadata().is_ok_and(|m| m.is_file()),
        });
        let Plan { argv, cwd, note } = launch_plan(std::env::consts::OS, command, &root, target)?;
        #[cfg(any(test, feature = "testutil"))]
        if let Some(launches) = crate::lock_ignoring_poison(&self.editor_launches).as_mut() {
            launches.push(
                argv.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
            );
            return Ok(note);
        }
        let record = crate::lock_ignoring_poison(&self.editor_record).clone();
        match record {
            Some(file) => record_launch(&file, &argv)?,
            None => launch(&argv, &cwd, self.login_path())?,
        }
        Ok(note)
    }

    /// From now on, editor launches are appended to `file` (one JSON array of the program
    /// and its arguments per line) instead of run; `None` runs them again. For the dev
    /// server, so that the UI's tests never start an editor.
    pub fn record_editor_launches_to(&self, file: Option<PathBuf>) {
        *crate::lock_ignoring_poison(&self.editor_record) = file;
    }

    /// From now on, editor launches are recorded instead of run (tests only; see
    /// [`RepoManager::editor_launches`]).
    #[cfg(any(test, feature = "testutil"))]
    pub fn record_editor_launches(&self) {
        crate::lock_ignoring_poison(&self.editor_launches).get_or_insert_with(Vec::new);
    }

    /// The argument vectors of the editor launches recorded so far (tests only).
    #[cfg(any(test, feature = "testutil"))]
    pub fn editor_launches(&self) -> Vec<Vec<String>> {
        crate::lock_ignoring_poison(&self.editor_launches)
            .clone()
            .unwrap_or_default()
    }
}

/// Appends `argv` to `file` as a line of JSON (see [`RepoManager::record_editor_launches_to`]).
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
            CoreError::new(ErrorCode::Io, "Could not record the editor launch")
                .with_detail(e.to_string())
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

/// Said when a file was asked for, no editor command is set, and the worktree folder was
/// opened instead.
const FOLDER_INSTEAD: &str = "No editor command is set, so the worktree folder was opened \
                              instead of the file: the system's default application for a \
                              file from a repository could run it. Set an editor command in \
                              Settings to open files.";

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
    /// The worktree for an editor. For the platform's opener a folder outside any
    /// repository, so that a program of that name in the worktree is never the one run
    /// (a relative entry in `PATH` resolves against it).
    cwd: PathBuf,
    note: Option<String>,
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
/// The opener is `/usr/bin/open` on macOS and `explorer.exe` in the Windows folder;
/// `xdg-open` elsewhere, found through `PATH`. It gets the path as a plain argument; no
/// shell (`cmd.exe`) is ever involved.
fn launch_plan(os: &str, command: Option<&str>, root: &Path, file: Option<Target>) -> Result<Plan> {
    let words = command.map(split_command).transpose()?.unwrap_or_default();
    if !words.is_empty() {
        let mut argv: Vec<OsString> = words.into_iter().map(OsString::from).collect();
        argv.push(file.map_or(root, |f| f.path).as_os_str().to_owned());
        return Ok(Plan {
            argv,
            cwd: root.to_owned(),
            note: None,
        });
    }
    // The worktree itself is the folder, not a file.
    let file = file.filter(|f| f.path != root);
    let (opener, cwd): (OsString, PathBuf) = match os {
        "macos" => ("/usr/bin/open".into(), PathBuf::from("/")),
        "windows" => {
            let windows = windows_dir();
            (windows.join("explorer.exe").into_os_string(), windows)
        }
        _ => ("xdg-open".into(), PathBuf::from("/")),
    };
    let (argv, note) = match file {
        Some(file) if os == "macos" && file.regular => (
            vec![opener, "-t".into(), file.path.as_os_str().to_owned()],
            None,
        ),
        Some(_) => (
            vec![opener, root.as_os_str().to_owned()],
            Some(FOLDER_INSTEAD.to_owned()),
        ),
        None => (vec![opener, root.as_os_str().to_owned()], None),
    };
    Ok(Plan { argv, cwd, note })
}

/// The Windows folder (`%SystemRoot%`, `C:\Windows` if unset).
fn windows_dir() -> PathBuf {
    std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
}

/// `command` split into words on whitespace; text in single or double quotes is one word
/// (or part of one), taken literally. Nothing else is special: no escapes, no variables.
fn split_command(command: &str) -> Result<Vec<String>> {
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
            "The editor command has a quote that is not closed. Check it in Settings.",
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
fn resolve_program(program: &OsString, path: Option<&OsString>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let name = Path::new(program);
    if name.components().count() != 1 || name.is_absolute() {
        return Some(name.to_owned());
    }
    let path = path.cloned().or_else(|| std::env::var_os("PATH"))?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// Starts `argv` in `cwd` with no input or output, in a process group of its own (so it
/// outlives a terminal's Ctrl-C), and does not wait for it; a thread reaps it when it ends.
/// `path` is the `PATH` to find the program with (the login shell's), if known; only its
/// absolute entries (see `resolve_program`).
fn launch(argv: &[OsString], cwd: &Path, path: Option<OsString>) -> Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Err(CoreError::new(ErrorCode::BadRequest, "No editor to run."));
    };
    #[cfg(unix)]
    let resolved = resolve_program(program, path.as_ref()).ok_or_else(|| {
        CoreError::new(
            ErrorCode::Io,
            format!(
                "The program \u{201c}{}\u{201d} was not found, so nothing was opened. Set \
                 the editor command in Settings.",
                program.to_string_lossy()
            ),
        )
    })?;
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
        cmd.process_group(0);
    }
    let name = program.to_string_lossy();
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CoreError::new(
            ErrorCode::Io,
            format!(
                "The program \u{201c}{name}\u{201d} was not found, so nothing was opened. Set \
                 the editor command in Settings."
            ),
        )
        .with_detail(e.to_string())),
        Err(e) => Err(CoreError::new(
            ErrorCode::Io,
            format!("Could not start \u{201c}{name}\u{201d} to open the file."),
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
        assert_eq!(
            resolve_program(&OsString::from("plain"), Some(&absolute)),
            None
        );
    }

    #[test]
    fn commands_split_on_whitespace_with_simple_quotes() {
        let split = |c: &str| split_command(c).unwrap();
        assert_eq!(split("code -w"), ["code", "-w"]);
        assert_eq!(split("  subl\t--wait  "), ["subl", "--wait"]);
        assert_eq!(
            split(r#""/Applications/My Editor.app/x" 'a "b"' c"d e"f"#),
            ["/Applications/My Editor.app/x", "a \"b\"", "cd ef"]
        );
        assert_eq!(split("x '' y"), ["x", "", "y"]);
        assert_eq!(split("a\\ b $HOME"), ["a\\", "b", "$HOME"]);
        assert!(split("").is_empty());
        assert_eq!(
            split_command("vim \"x").unwrap_err().code,
            ErrorCode::BadRequest
        );
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

    /// The argv (as strings) and whether there is a note.
    fn plan_on(os: &str, command: Option<&str>, file: Option<&str>) -> (Vec<String>, bool) {
        let target = file.map(|f| Target {
            path: Path::new(f),
            regular: true,
        });
        let Plan { argv, note, .. } = launch_plan(os, command, Path::new("/r"), target).unwrap();
        let argv = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        (argv, note.is_some())
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
            for file in [
                None,
                Some(Target {
                    path: Path::new("/r/a.txt"),
                    regular: true,
                }),
            ] {
                let plan = launch_plan(os, None, root, file).unwrap();
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
}
