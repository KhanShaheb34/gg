//! The Windows `gg` launcher (`src-tauri/resources/gg.cmd`) as installed, run through cmd
//! against fixture folders. The app is never started: these cases all end before the
//! launcher's `start` (version, help, a missing folder, a folder outside any repository,
//! an app that is no longer where it was installed from), which is enough to see how cmd
//! reads paths with spaces, parentheses, `&` and `!` in them. Windows only (CI).
#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
