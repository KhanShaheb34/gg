//! The release workflow's handling of the update signing key: one step of one job sees it,
//! the sign job, which checks nothing out, builds nothing and runs no pnpm, node or cargo;
//! that step runs only the signer built beforehand (without the secrets), by its absolute
//! path with a clean PATH; the build jobs never see the key; no pull request runs the
//! workflow; the draft release uploads each update, its signature and latest.json, which are
//! checked first.

use std::fs;
use std::path::{Path, PathBuf};

fn read(rel: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

const UPDATES: [&str; 3] = [
    "gg-macos-universal.app.tar.gz",
    "gg-linux-x86_64.AppImage",
    "gg-windows-x64-setup.exe",
];

/// The text of job `name` of release.yml: its `  name:` line and the lines after it, up to
/// the next job's.
fn job(release: &str, name: &str) -> String {
    let is_job = |l: &str| {
        l.len() > 3
            && l.starts_with("  ")
            && !l.starts_with("   ")
            && l.ends_with(':')
            && l.trim_end_matches(':')
                .trim_start()
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'-')
    };
    let mut lines = release.lines().skip_while(|l| *l != format!("  {name}:"));
    let first = lines.next().unwrap_or_else(|| panic!("no job {name}"));
    let mut text = format!("{first}\n");
    for line in lines.take_while(|l| !is_job(l)) {
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// The steps of a job's text, each from its `- name:` (or `- uses:`) line on.
fn steps(job: &str) -> Vec<String> {
    let mut steps: Vec<String> = Vec::new();
    for line in job.lines() {
        if line.trim_start().starts_with("- name:") || line.trim_start().starts_with("- uses:") {
            steps.push(String::new());
        }
        if let Some(step) = steps.last_mut() {
            step.push_str(line);
            step.push('\n');
        }
    }
    steps
}

#[test]
fn one_step_of_the_sign_job_sees_the_key_and_runs_only_the_signer() {
    let release = read(".github/workflows/release.yml");
    // Every use of a secret.
    let uses: Vec<&str> = release.lines().filter(|l| l.contains("secrets.")).collect();
    assert_eq!(
        uses.iter().map(|l| l.trim()).collect::<Vec<_>>(),
        [
            "TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}",
            "TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
        ]
    );
    let sign = job(&release, "sign");
    let signing: Vec<String> = steps(&sign)
        .into_iter()
        .filter(|s| s.contains("secrets."))
        .collect();
    assert_eq!(signing.len(), 1, "one step of the sign job has them");
    let step = &signing[0];
    assert!(
        step.trim_start().starts_with("- name: Sign the updates\n"),
        "{step}"
    );
    // It runs the signer, and only it, by its absolute path, with a clean PATH and no
    // start-up file for its shell.
    let run: Vec<&str> = step
        .lines()
        .filter(|l| l.trim_start().starts_with("run:"))
        .collect();
    assert_eq!(
        run,
        [
            "        run: '\"$GITHUB_WORKSPACE/signer/gg-release\" sign --version \"$VERSION\" updates/gg-macos-universal.app.tar.gz updates/gg-linux-x86_64.AppImage updates/gg-windows-x64-setup.exe'"
        ]
    );
    assert!(step.contains("          PATH: /usr/bin:/bin\n"));
    assert!(step.contains("          BASH_ENV: \"\"\n"));
    // The sign job checks nothing out and builds nothing: before that step only the
    // signer and the updates are fetched, and the signer made runnable.
    for word in [
        "checkout", "pnpm", "node", "cargo", "tauri", "npm", "rustup",
    ] {
        assert!(!sign.contains(word), "the sign job has {word}");
    }
    let names: Vec<String> = steps(&sign)
        .iter()
        .map(|s| s.lines().next().unwrap().trim().to_owned())
        .collect();
    assert_eq!(
        names,
        [
            "- name: Fetch the signer",
            "- name: Fetch the updates",
            "- name: Make the signer runnable",
            "- name: Sign the updates",
            "- name: Keep the signatures",
        ]
    );
    // Only the signatures are kept.
    let kept = steps(&sign).pop().unwrap();
    assert!(kept.contains("name: gg-signatures"));
    let paths: Vec<&str> = kept
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("updates/"))
        .collect();
    assert_eq!(
        paths,
        UPDATES
            .iter()
            .map(|f| format!("updates/{f}.sig"))
            .collect::<Vec<_>>()
    );
    // The signer is built without the secrets, in a job of its own, from the lock file.
    let signer = job(&release, "signer");
    assert!(signer.contains("run: cargo build --locked --release -p gg-release\n"));
    assert!(!signer.contains("secrets."));
    assert!(sign.contains("needs: [version, signer, macos, linux, windows]"));
    // The jobs that build the app never see the key.
    for build in ["macos", "linux", "windows"] {
        assert!(!job(&release, build).contains("secrets."), "{build}");
    }
    // Tags only: no pull request runs the release workflow; CI uses no secret at all.
    assert!(release.contains("on:\n  push:\n    tags: [\"v*\"]\n"));
    assert!(!release.contains("pull_request"));
    assert!(!read(".github/workflows/ci.yml").contains("secrets."));
}

/// The signer reads the key and password from the environment, refuses without them, and
/// never prints them.
#[test]
fn the_signer_fails_without_the_key_and_never_prints_it() {
    let main = read("crates/gg-release/src/main.rs");
    assert!(main.contains("std::env::var(\"TAURI_SIGNING_PRIVATE_KEY\")"));
    assert!(main.contains("std::env::var(\"TAURI_SIGNING_PRIVATE_KEY_PASSWORD\")"));
    assert!(main.contains("TAURI_SIGNING_PRIVATE_KEY is not set: nothing is signed"));
    for (n, line) in main.lines().enumerate() {
        if line.contains("println!") || line.contains("eprintln!") {
            assert!(
                !line.contains("key") && !line.contains("password"),
                "main.rs:{} prints {line}",
                n + 1
            );
        }
    }
    assert!(
        !Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/sign-update.sh")
            .exists()
    );
}

#[test]
fn the_release_uploads_checked_updates_and_latest_json() {
    let release = read(".github/workflows/release.yml");
    let assets_start = release.find("  ASSETS: >-\n").expect("ASSETS");
    let assets: Vec<&str> = release[assets_start..]
        .lines()
        .skip(1)
        .take_while(|l| l.starts_with("    "))
        .flat_map(str::split_whitespace)
        .collect();
    for file in UPDATES {
        assert!(assets.contains(&file), "{file}");
        assert!(
            assets.contains(&format!("{file}.sig").as_str()),
            "{file}.sig"
        );
    }
    assert!(assets.contains(&"latest.json"));
    assert!(assets.contains(&"SHA256SUMS"));
    // Checked with the app's key, and the manifest against what is uploaded, before the
    // draft release is made (which needs the job that checks, which needs the signatures).
    let updates = job(&release, "updates");
    assert!(updates.contains("needs: [version, macos, linux, windows, sign]"));
    assert!(!updates.contains("secrets."));
    assert!(updates.contains("gg-release verify --config src-tauri/tauri.conf.json"));
    assert!(updates.contains("gg-release check-manifest --config src-tauri/tauri.conf.json"));
    assert!(updates.contains("updates/latest.json $ASSETS"));
    let draft = job(&release, "release");
    assert!(draft.contains("needs: [version, macos, linux, windows, sign, updates]"));
    assert!(draft.contains("--generate-notes $ASSETS"));
    // macOS: the update is made from the final signed app (release-macos.sh, after signing).
    let mac = read("scripts/release-macos.sh");
    let signed = mac
        .find("codesign --verify --deep --strict \"$app\"")
        .unwrap();
    let archived = mac
        .find("sh scripts/update-archive.sh \"$app\" \"$out/$update\"")
        .unwrap();
    assert!(signed < archived);
    let archive = read("scripts/update-archive.sh");
    assert!(archive.contains("codesign --verify --deep --strict \"$check/$name\""));
}
