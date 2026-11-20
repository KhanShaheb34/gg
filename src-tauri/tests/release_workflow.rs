//! The release workflow's handling of the update signing key: only the steps that sign see
//! it, those steps fail without it, no pull request ever runs with it, and the draft release
//! uploads each update, its signature and latest.json, which are checked first.

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

#[test]
fn only_the_signing_steps_see_the_key() {
    let release = read(".github/workflows/release.yml");
    let lines: Vec<&str> = release.lines().collect();
    let mut signing_steps = 0;
    for (n, line) in lines.iter().enumerate() {
        if !line.contains("secrets.") {
            continue;
        }
        assert!(
            line.contains("secrets.TAURI_SIGNING_PRIVATE_KEY"),
            "release.yml:{} uses another secret",
            n + 1
        );
        // The step it is in.
        let step = lines[..n]
            .iter()
            .rev()
            .find(|l| l.trim_start().starts_with("- name:"))
            .unwrap_or_else(|| panic!("release.yml:{} is outside a step", n + 1));
        assert_eq!(
            step.trim(),
            "- name: Sign the update",
            "release.yml:{} gives the key to another step",
            n + 1
        );
        if line.contains("TAURI_SIGNING_PRIVATE_KEY:") {
            signing_steps += 1;
        }
    }
    assert_eq!(signing_steps, 3, "one signing step per platform");
    // Each runs the script that fails without the key.
    assert_eq!(
        release
            .matches("run: scripts/sign-update.sh \"$VERSION\" dist-release/")
            .count(),
        3
    );
    let sign = read("scripts/sign-update.sh");
    assert!(sign.contains("[ -n \"${TAURI_SIGNING_PRIVATE_KEY:-}\" ] ||"));
    assert!(sign.contains("--app-version \"$version\""));
    // Tags only: no pull request runs the release workflow; CI uses no secret at all.
    assert!(release.contains("on:\n  push:\n    tags: [\"v*\"]\n"));
    assert!(!release.contains("pull_request"));
    assert!(!read(".github/workflows/ci.yml").contains("secrets."));
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
    // draft release is made (which needs the job that checks).
    assert!(release.contains("gg-release verify --config src-tauri/tauri.conf.json"));
    assert!(release.contains("gg-release check-manifest --config src-tauri/tauri.conf.json"));
    assert!(release.contains("updates/latest.json $ASSETS"));
    assert!(release.contains("needs: [version, macos, linux, windows, updates]"));
    assert!(release.contains("--generate-notes $ASSETS"));
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
