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
        let (argv, note) = launch_plan(std::env::consts::OS, command, &root, target)?;
        #[cfg(any(test, feature = "testutil"))]
        if let Some(launches) = crate::lock_ignoring_poison(&self.editor_launches).as_mut() {
            launches.push(
                argv.iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect(),
            );
            return Ok(note);
        }
        launch(&argv, &worktree, self.login_path())?;
        Ok(note)
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

/// The program and arguments that open `file` (`None`: the worktree folder `root`) on
/// operating system `os` (`std::env::consts::OS`), and a note for the user if something
/// else than asked was opened.
///
/// With `command` (its words, then the path) the editor gets the file. Without one, no
/// file of the repository ever goes to the system's default application, which picks a
/// program by file type and could run it (a `.command`, `.sh`, `.exe`, `.desktop`): on
/// macOS a regular file opens as text (`open -t`); otherwise (a folder such as an `X.app`
/// bundle, which `open` would start, or any file elsewhere) the worktree folder opens
/// instead, with the note. Asked for the worktree itself (`.`), it opens without one. Folders open with `open`, `xdg-open` or `explorer`, each given the path as a
/// plain argument; no shell (`cmd.exe`) is ever involved.
fn launch_plan(
    os: &str,
    command: Option<&str>,
    root: &Path,
    file: Option<Target>,
) -> Result<(Vec<OsString>, Option<String>)> {
    let words = command.map(split_command).transpose()?.unwrap_or_default();
    if !words.is_empty() {
        let mut argv: Vec<OsString> = words.into_iter().map(OsString::from).collect();
        argv.push(file.map_or(root, |f| f.path).as_os_str().to_owned());
        return Ok((argv, None));
    }
    // The worktree itself is the folder, not a file.
    let file = file.filter(|f| f.path != root);
    let folder_opener = match os {
        "macos" => "open",
        "windows" => "explorer",
        _ => "xdg-open",
    };
    Ok(match file {
        Some(file) if os == "macos" && file.regular => (
            vec!["open".into(), "-t".into(), file.path.as_os_str().to_owned()],
            None,
        ),
        Some(_) => (
            vec![folder_opener.into(), root.as_os_str().to_owned()],
            Some(FOLDER_INSTEAD.to_owned()),
        ),
        None => (
            vec![folder_opener.into(), root.as_os_str().to_owned()],
            None,
        ),
    })
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

/// Starts `argv` in `cwd` with no input or output, in a process group of its own (so it
/// outlives a terminal's Ctrl-C), and does not wait for it; a thread reaps it when it ends.
/// `path` is the `PATH` to find the program with (the login shell's), if known.
fn launch(argv: &[OsString], cwd: &Path, path: Option<OsString>) -> Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Err(CoreError::new(ErrorCode::BadRequest, "No editor to run."));
    };
    let mut cmd = Command::new(program);
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
        let (argv, note) = launch_plan(
            "macos",
            None,
            Path::new("/r"),
            Some(Target {
                path: Path::new("/r/X.app"),
                regular: false,
            }),
        )
        .unwrap();
        assert_eq!(argv, ["open", "/r"]);
        assert!(note.is_some());
        let (argv, note) = launch_plan(
            "macos",
            None,
            Path::new("/r"),
            Some(Target {
                path: Path::new("/r"),
                regular: false,
            }),
        )
        .unwrap();
        assert_eq!(argv, ["open", "/r"]);
        assert!(note.is_none());
    }

    /// The argv (as strings) and whether there is a note.
    fn plan_on(os: &str, command: Option<&str>, file: Option<&str>) -> (Vec<String>, bool) {
        let target = file.map(|f| Target {
            path: Path::new(f),
            regular: true,
        });
        let (argv, note) = launch_plan(os, command, Path::new("/r"), target).unwrap();
        let argv = argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        (argv, note.is_some())
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
            (words(&["open", "-t", "/r/x.command"]), false)
        );
        assert_eq!(
            plan_on("linux", None, file),
            (words(&["xdg-open", "/r"]), true)
        );
        assert_eq!(
            plan_on("windows", None, file),
            (words(&["explorer", "/r"]), true)
        );
        // A blank command is no command.
        assert_eq!(
            plan_on("linux", Some("  "), file),
            (words(&["xdg-open", "/r"]), true)
        );
        // The worktree folder itself opens as before.
        assert_eq!(
            plan_on("macos", None, None),
            (words(&["open", "/r"]), false)
        );
        assert_eq!(
            plan_on("linux", None, None),
            (words(&["xdg-open", "/r"]), false)
        );
        assert_eq!(
            plan_on("windows", None, None),
            (words(&["explorer", "/r"]), false)
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
