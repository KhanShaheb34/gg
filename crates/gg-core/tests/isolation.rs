//! In tests, the core's git never reads the developer's global or system git config
//! (`pull.rebase`, `core.hooksPath`, `merge.ff`, ... would change results). One test only:
//! it sets the process environment.

use std::fs;

use gg_core::git::runner::{CmdKind, CommandLog, GitRunner};
use gg_core::testutil::TestRepo;

#[tokio::test]
async fn tests_never_read_the_developers_git_config() {
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join("global");
    fs::write(&global, "[pull]\n\trebase = true\n[gg]\n\tprobe = global\n").unwrap();
    let system = dir.path().join("system");
    fs::write(&system, "[gg]\n\tprobe = system\n").unwrap();
    // SAFETY: this test binary holds this one test, and nothing else in it reads or writes
    // the environment while it runs.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_SYSTEM", &system);
        std::env::remove_var("GIT_CONFIG_NOSYSTEM");
    }
    let repo = TestRepo::new();
    // A git started plainly from this process reads them.
    let plain = std::process::Command::new("git")
        .args(["config", "--get-all", "gg.probe"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let seen = String::from_utf8_lossy(&plain.stdout);
    assert!(seen.contains("global") && seen.contains("system"), "{seen}");

    let runner = GitRunner::detect(CommandLog::new()).await.unwrap();
    for key in ["gg.probe", "pull.rebase"] {
        let out = runner
            .run_unchecked(repo.path(), &["config", "--get-all", key], CmdKind::Read)
            .await
            .unwrap();
        assert_eq!(out.code, 1, "{key} was read: {}", out.text());
    }
    // The repository's own config still applies.
    repo.git(&["config", "gg.probe", "local"]);
    let out = runner
        .run_unchecked(
            repo.path(),
            &["config", "--get-all", "gg.probe"],
            CmdKind::Read,
        )
        .await
        .unwrap();
    assert_eq!(out.text().trim(), "local");
}
