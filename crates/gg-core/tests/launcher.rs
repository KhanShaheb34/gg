//! The `gg` launcher script (`src-tauri/resources/gg`) as installed, run with `sh` against
//! fixture repositories. The app is never started: `GG_APP_BIN` replaces it with a stub that
//! records its arguments, and where the launcher's own way of starting the app is checked,
//! `open` and `uname` are stand-ins on `PATH`.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use gg_core::cli::launcher_script;
use gg_core::testutil::{TestRepo, empty_git_config};

/// A folder holding the installed launcher and a recording stub for the app.
struct Launcher {
    dir: tempfile::TempDir,
}

impl Launcher {
    fn new(app: Option<&Path>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("gg");
        fs::write(&script, launcher_script("9.8.7", app)).unwrap();
        executable(&script);
        // Records each argument on a line of its own, then stays a while (like the app).
        let stub = dir.path().join("app-stub");
        fs::write(
            &stub,
            "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$GG_STUB_OUT.tmp\"\n\
             mv \"$GG_STUB_OUT.tmp\" \"$GG_STUB_OUT\"\nsleep 5\n",
        )
        .unwrap();
        executable(&stub);
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// Runs `sh gg <args>` in `cwd` with the app replaced by the stub.
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut cmd = self.command(cwd, args);
        cmd.env("GG_APP_BIN", self.path("app-stub"));
        cmd.output().unwrap()
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg(self.path("gg"))
            .args(args)
            .current_dir(cwd)
            .env("GG_STUB_OUT", self.path("launched"))
            .env("GIT_CONFIG_GLOBAL", empty_git_config())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GG_APP_BIN")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE");
        cmd
    }

    /// The arguments the app was started with, waiting up to 5 s for it to start.
    fn launched(&self) -> Vec<String> {
        let file = self.path("launched");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !file.exists() {
            assert!(Instant::now() < deadline, "the app was not started");
            std::thread::sleep(Duration::from_millis(20));
        }
        let text = fs::read_to_string(&file).unwrap();
        fs::remove_file(&file).unwrap();
        text.lines().map(str::to_owned).collect()
    }

    /// The app was not started (give a launch that went wrong a moment to show).
    fn not_launched(&self) {
        std::thread::sleep(Duration::from_millis(200));
        assert!(!self.path("launched").exists(), "the app was started");
    }
}

fn executable(file: &Path) {
    fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
}

fn real(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn repo_with_commit(name: &str) -> TestRepo {
    let repo = TestRepo::new_in_subdir(name);
    repo.write("src/lib/a.txt", "a");
    repo.commit_all("first");
    repo
}

#[test]
fn opens_the_toplevel_of_a_subfolder_and_returns_at_once() {
    let repo = repo_with_commit("repo");
    let gg = Launcher::new(None);
    let started = Instant::now();
    let out = gg.run(&repo.path().join("src/lib"), &[]);
    // The stub stays 5 s: the launcher did not wait for it.
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);
    // An explicit path, and a file standing for its folder.
    let out = gg.run(
        Path::new("/"),
        &[&repo.path().join("src").to_string_lossy()],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);
    let file = repo.path().join("src/lib/a.txt");
    let out = gg.run(Path::new("/"), &[&file.to_string_lossy()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);
}

#[test]
fn a_linked_worktree_opens_as_itself() {
    let repo = repo_with_commit("repo");
    let worktree = repo.add_worktree("feature");
    fs::create_dir_all(worktree.join("deep")).unwrap();
    let gg = Launcher::new(None);
    let out = gg.run(&worktree.join("deep"), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(&worktree)]);
}

#[test]
fn a_bare_repository_or_a_plain_folder_is_refused() {
    let gg = Launcher::new(None);
    let bare = TestRepo::new_in_subdir("bare.git");
    bare.git(&["config", "core.bare", "true"]);
    let plain = tempfile::tempdir().unwrap();
    for dir in [bare.path().to_path_buf(), plain.path().to_path_buf()] {
        let arg = dir.to_string_lossy().into_owned();
        let out = gg.run(Path::new("/"), &[&arg]);
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(
            stderr(&out),
            format!("gg: {arg} is not inside a git repository\n")
        );
    }
    // Inside a repository's .git folder there is no worktree either.
    let repo = repo_with_commit("repo");
    let out = gg.run(&repo.path().join(".git"), &[]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "gg: . is not inside a git repository\n");
    // A path that is not there.
    let out = gg.run(Path::new("/"), &["/no/such/folder"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "gg: /no/such/folder: no such folder\n");
    gg.not_launched();
}

#[test]
fn paths_with_spaces_relative_paths_and_leading_dashes() {
    let gg = Launcher::new(None);
    let spaced = repo_with_commit("my repo ünï");
    let out = gg.run(Path::new("/"), &[&spaced.path().to_string_lossy()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(spaced.path())]);

    // Relative to where gg runs.
    let repo = repo_with_commit("repo");
    let parent = repo.path().parent().unwrap();
    let out = gg.run(parent, &["repo/src"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);

    // A folder whose name starts with -: after --, else an unknown option.
    let dashed = repo_with_commit("-dashed");
    let parent = dashed.path().parent().unwrap();
    let out = gg.run(parent, &["--", "-dashed"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(dashed.path())]);
    let out = gg.run(parent, &["-dashed"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("gg -- -dashed"), "{}", stderr(&out));
    // `-` alone is a folder named -, not the previous folder.
    fs::create_dir(repo.path().join("-")).unwrap();
    let out = gg.run(repo.path(), &["-"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);
    // One path at most.
    let out = gg.run(parent, &["a", "b"]);
    assert_eq!(out.status.code(), Some(2));
    gg.not_launched();
}

#[test]
fn version_and_help() {
    let gg = Launcher::new(None);
    for flag in ["--version", "-V"] {
        let out = gg.run(Path::new("/"), &[flag]);
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "gg 9.8.7\n");
    }
    for flag in ["--help", "-h"] {
        let out = gg.run(Path::new("/"), &[flag]);
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("usage: gg [path]"));
    }
    gg.not_launched();
}

/// `PATH` with a folder of stand-ins first: `open` and `uname` that record or answer as
/// asked, so the launcher's own way of starting the app runs without starting anything.
fn stand_ins(dir: &Path, open_body: &str, uname: &str) -> String {
    let bin = dir.join("stand-ins");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("open"), format!("#!/bin/sh\n{open_body}\n")).unwrap();
    executable(&bin.join("open"));
    fs::write(bin.join("uname"), format!("#!/bin/sh\necho {uname}\n")).unwrap();
    executable(&bin.join("uname"));
    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap())
}

#[test]
fn on_macos_the_app_is_started_by_its_bundle_identifier() {
    let repo = repo_with_commit("repo");
    let gg = Launcher::new(None);
    let record = gg.path("open-args");
    let path = stand_ins(
        gg.dir.path(),
        &format!("printf '%s\\n' \"$@\" > '{}'", record.display()),
        "Darwin",
    );
    let out = gg
        .command(repo.path(), &[])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let args = fs::read_to_string(&record).unwrap();
    let top = real(repo.path());
    assert_eq!(args, format!("-n\n-b\ncom.shakirul.gg\n--args\n{top}\n"));

    // No app with that identifier: said, exit 1.
    let path = stand_ins(
        gg.dir.path(),
        "echo 'Unable to find application with bundle identifier com.shakirul.gg' >&2; exit 1",
        "Darwin",
    );
    let out = gg
        .command(repo.path(), &[])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).starts_with("gg: the gg app was not found"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn elsewhere_the_app_is_the_program_it_was_installed_from() {
    let repo = repo_with_commit("repo");
    let probe = Launcher::new(None);
    let stub = probe.path("app-stub");
    let gg = Launcher::new(Some(&stub));
    let path = stand_ins(gg.dir.path(), "exit 9", "Linux");
    let out = gg
        .command(repo.path(), &[])
        .env("PATH", &path)
        .env("GG_STUB_OUT", gg.path("launched"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);

    // The app moved since: said, with what to do, exit 1.
    let moved = Launcher::new(Some(Path::new("/no/longer/here/gg-desktop")));
    let path = stand_ins(moved.dir.path(), "exit 9", "Linux");
    let out = moved
        .command(repo.path(), &[])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(
        err.contains("no longer at /no/longer/here/gg-desktop"),
        "{err}"
    );
    assert!(err.contains("Install 'gg' Command again"), "{err}");
    moved.not_launched();
}

/// The launched path as the stub wrote it, whole (a name may end with a newline).
fn launched_raw(gg: &Launcher) -> String {
    let file = gg.path("launched");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !file.exists() {
        assert!(Instant::now() < deadline, "the app was not started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = fs::read_to_string(&file).unwrap();
    fs::remove_file(&file).unwrap();
    text
}

#[test]
fn a_folder_reached_through_a_link_opens_where_it_really_is() {
    let repo = repo_with_commit("repo");
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("shortcut");
    std::os::unix::fs::symlink(repo.path(), &link).unwrap();
    let gg = Launcher::new(None);
    let out = gg.run(&link.join("src"), &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);
    let out = gg.run(Path::new("/"), &[&link.to_string_lossy()]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);
}

#[test]
fn names_ending_in_a_newline_or_holding_a_backslash_are_kept_whole() {
    let gg = Launcher::new(None);
    for name in ["ends in newline\n", r"back\slash \n not one", "two\n\n"] {
        let repo = repo_with_commit(name);
        let out = gg.run(repo.path(), &[]);
        assert!(out.status.success(), "{name:?}: {}", stderr(&out));
        assert_eq!(launched_raw(&gg), format!("{}\n", real(repo.path())), "{name:?}");
        // Given as an argument too, from elsewhere.
        let out = gg.run(Path::new("/"), &[&repo.path().to_string_lossy()]);
        assert!(out.status.success(), "{name:?}: {}", stderr(&out));
        assert_eq!(launched_raw(&gg), format!("{}\n", real(repo.path())), "{name:?}");
    }
    // A message naming such a path prints it as it is (printf, not echo).
    let plain = tempfile::tempdir().unwrap();
    let odd = plain.path().join(r"a\nb");
    fs::create_dir(&odd).unwrap();
    let out = gg.run(Path::new("/"), &[&odd.to_string_lossy()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        format!("gg: {} is not inside a git repository\n", odd.display())
    );
}

#[test]
fn a_git_dir_set_in_the_environment_does_not_decide_the_repository() {
    let repo = repo_with_commit("repo");
    let other = repo_with_commit("other");
    let gg = Launcher::new(None);
    let out = gg
        .command(repo.path(), &[])
        .env("GG_APP_BIN", gg.path("app-stub"))
        .env("GIT_DIR", other.path().join(".git"))
        .env("GIT_WORK_TREE", other.path())
        .env("GIT_INDEX_FILE", other.path().join(".git/index"))
        .env("GIT_COMMON_DIR", other.path().join(".git"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(gg.launched(), [real(repo.path())]);
}

/// git's own refusal (other than "not a repository") is shown as git says it.
#[test]
fn git_explains_a_refusal_of_its_own() {
    let repo = repo_with_commit("repo");
    // Like a repository owned by another user (which cannot be made here): one git refuses
    // to read, for a reason of its own.
    repo.git(&["config", "core.repositoryformatversion", "99"]);
    let gg = Launcher::new(None);
    let out = gg.run(repo.path(), &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(!err.contains("is not inside a git repository"), "{err}");
    assert!(err.contains("repo version"), "{err}");
    gg.not_launched();
}
