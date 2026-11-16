//! The Windows `gg` launcher (`src-tauri/resources/gg.cmd`) as installed, run through cmd
//! against fixture folders. The app is never started: most cases end before the launcher's
//! `start` (version, help, a missing folder, a folder outside any repository, an app that is
//! no longer where it was installed from), which is enough to see how cmd reads paths with
//! spaces, parentheses, `&` and `!` in them; the cases that get as far as `start` start a
//! recording stub instead (`GG_APP_BIN`). Windows only (CI).
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use gg_core::cli::windows_launcher_script;
use gg_core::testutil::{TestRepo, empty_git_config};

/// A folder holding the launcher, filled in for an app at `app`.
fn installed(dir: &Path, app: &str) -> PathBuf {
    let script = dir.join("gg.cmd");
    fs::write(&script, windows_launcher_script("9.8.7", app).unwrap()).unwrap();
    script
}

fn run(script: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(script)
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", empty_git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GG_APP_BIN")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

#[test]
fn version_and_help() {
    let dir = tempfile::tempdir().unwrap();
    let gg = installed(dir.path(), r"C:\nowhere\gg-desktop.exe");
    let out = run(&gg, dir.path(), &["--version"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout).trim(), "gg 9.8.7");
    let out = run(&gg, dir.path(), &["-h"]);
    assert!(out.status.success());
    assert!(text(&out.stdout).starts_with("usage: gg [path]"));
}

#[test]
fn paths_with_spaces_parentheses_ampersands_and_bangs_are_read_whole() {
    let dir = tempfile::tempdir().unwrap();
    let gg = installed(dir.path(), r"C:\nowhere\gg-desktop.exe");
    for name in [
        "my folder",
        "Program Files (x86)",
        "a & b",
        "wow!",
        "50% off",
    ] {
        let plain = dir.path().join("plain").join(name);
        fs::create_dir_all(&plain).unwrap();
        let out = run(&gg, dir.path(), &[plain.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(1), "{name}: {out:?}");
        let said = text(&out.stderr);
        assert!(
            said.contains(&format!(
                "{} is not inside a git repository",
                plain.display()
            )),
            "{name}: {said}"
        );
        let missing = dir.path().join("missing").join(name);
        let out = run(&gg, dir.path(), &[missing.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(1), "{name}: {out:?}");
        assert!(text(&out.stderr).contains("no such folder"), "{name}");
    }
}

#[test]
fn a_repository_whose_app_moved_says_to_install_again() {
    let repo = TestRepo::new();
    repo.commit("one");
    let dir = tempfile::tempdir().unwrap();
    let gg = installed(dir.path(), r"C:\gone (x86) & away!\gg-desktop.exe");
    let out = run(&gg, repo.path(), &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let said = text(&out.stderr);
    assert!(
        said.contains(r"the gg app is no longer at C:\gone (x86) & away!\gg-desktop.exe"),
        "{said}"
    );
}

/// A repository's own `git.cmd` in the folder gg runs in is never what the launcher runs.
#[test]
fn a_git_in_the_current_folder_is_never_run() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    fs::create_dir_all(&plain).unwrap();
    let marker = dir.path().join("ran");
    fs::write(
        plain.join("git.cmd"),
        format!("@echo off\r\necho ran> \"{}\"\r\n", marker.display()),
    )
    .unwrap();
    let gg = installed(dir.path(), r"C:\nowhere\gg-desktop.exe");
    let out = run(&gg, &plain, &[]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!marker.exists(), "the folder's git.cmd ran");
}

/// `script` run in `cwd` with `args`, starting the stub `stub` instead of the app.
fn run_with_stub(script: &Path, cwd: &Path, args: &[&str], stub: &Path) -> Output {
    Command::new(script)
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", empty_git_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GG_APP_BIN", stub)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap()
}

/// A stub in `dir` that writes the folder it is given to `record` (in `dir`): what the app
/// would have been asked to open. `start` runs a batch file in a cmd that stays open (`/K`),
/// holding the launcher's output pipes: the stub ends that cmd itself.
fn recording_stub(dir: &Path) -> (PathBuf, PathBuf) {
    let stub = dir.join("stub.cmd");
    let record = dir.join("record.txt");
    fs::write(
        &stub,
        "@echo off\r\n>\"%~dp0record.txt\" echo(%~1\r\nexit\r\n",
    )
    .unwrap();
    (stub, record)
}

/// What the stub recorded, once it has (`start` returns before the stub has run).
fn recorded(record: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = fs::read_to_string(record)
            && text.ends_with('\n')
        {
            return text.trim_end().to_owned();
        }
        assert!(Instant::now() < deadline, "the stub was not started");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// In a repository (a folder of it, or given as an argument), the app is started with that
/// folder in full.
#[test]
fn a_folder_of_a_repository_is_given_to_the_app() {
    let repo = TestRepo::new();
    repo.commit("one");
    let sub = repo.path().join("my folder");
    fs::create_dir_all(&sub).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let gg = installed(dir.path(), r"C:\nowhere\gg-desktop.exe");
    let (stub, record) = recording_stub(dir.path());
    for (cwd, args) in [
        (sub.as_path(), vec![]),
        (dir.path(), vec![sub.to_str().unwrap()]),
    ] {
        let _ = fs::remove_file(&record);
        let out = run_with_stub(&gg, cwd, &args, &stub);
        assert!(out.status.success(), "{out:?}");
        let opened = recorded(&record);
        assert_eq!(
            dunce::canonicalize(&opened).unwrap(),
            dunce::canonicalize(&sub).unwrap(),
            "{opened}"
        );
    }
}

/// A repository at a drive's root (`subst` makes one of a fixture folder): the app is given
/// `X:\.`, never `X:\` (whose `\` would escape the closing quote).
#[test]
fn a_repository_at_a_drive_root_is_given_as_the_root() {
    let repo = TestRepo::new();
    repo.commit("one");
    let free = ('M'..='Z')
        .rev()
        .map(|l| format!("{l}:"))
        .find(|d| !Path::new(&format!("{d}\\")).exists())
        .expect("a free drive letter");
    let subst = |args: &[&str]| Command::new("subst").args(args).output().unwrap();
    let made = subst(&[&free, repo.path().to_str().unwrap()]);
    assert!(made.status.success(), "{made:?}");
    struct Unsubst(String);
    impl Drop for Unsubst {
        fn drop(&mut self) {
            let _ = Command::new("subst").args([&self.0, "/d"]).output();
        }
    }
    let _undo = Unsubst(free.clone());
    let dir = tempfile::tempdir().unwrap();
    let gg = installed(dir.path(), r"C:\nowhere\gg-desktop.exe");
    let (stub, record) = recording_stub(dir.path());
    let root = format!("{free}\\");
    let out = run_with_stub(&gg, dir.path(), &[&root], &stub);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(recorded(&record), format!("{free}\\."));
}
