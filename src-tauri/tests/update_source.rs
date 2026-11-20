//! The shipped app checks one update source, GitHub's latest release, signed with gg's key,
//! and nothing can point it elsewhere: no setting, argument or environment variable is read
//! for it outside `src/update_hook.rs`, which only the `perf` feature compiles (and which
//! only the perf build's identifier, `com.shakirul.gg.perf`, obeys). The config files and
//! the build scripts add nothing to `plugins.updater`, and the page is granted none of the
//! updater's commands. (`lib.rs`'s own test checks the config as compiled into the app.)

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = root().join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn json(rel: &str) -> serde_json::Value {
    serde_json::from_str(&read(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn the_config_names_github_and_gg_s_key() {
    let updater = &json("src-tauri/tauri.conf.json")["plugins"]["updater"];
    assert_eq!(
        updater["endpoints"],
        serde_json::json!([
            "https://github.com/KhanShaheb34/gg/releases/latest/download/latest.json"
        ])
    );
    assert_eq!(updater["pubkey"], gg_desktop_lib::UPDATE_PUBKEY);
    for rel in [
        "src-tauri/tauri.linux.conf.json",
        "src-tauri/tauri.windows.conf.json",
    ] {
        assert!(
            json(rel).get("plugins").is_none(),
            "{rel} changes the plugins' config"
        );
    }
}

/// No build of the app is given a config overlay that touches the updater, and none turns
/// off its checks.
#[test]
fn no_build_changes_the_update_source() {
    for rel in [
        "scripts/release-macos.sh",
        "scripts/perf.sh",
        "scripts/walk.sh",
        ".github/workflows/release.yml",
        ".github/workflows/ci.yml",
        "package.json",
    ] {
        let text = read(rel);
        for word in [
            "\"plugins\"",
            "\"updater\"",
            "endpoints",
            "pubkey",
            "dangerous",
            "allowDowngrades",
            "TAURI_CONFIG",
        ] {
            assert!(!text.contains(word), "{rel} has {word}");
        }
    }
}

/// What could point the updater elsewhere at run time is in `update_hook.rs` alone, and
/// `lib.rs` uses that module only in performance builds.
#[test]
fn only_the_perf_build_has_a_test_source() {
    let src = root().join("src-tauri/src");
    let hooks = [
        "GG_UPDATE_",
        ".endpoints(",
        ".pubkey(",
        "config_mut",
        "dangerousInsecureTransportProtocol",
        "\"updater\"",
    ];
    for entry in fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "update_hook.rs" {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        // The app's code, not its tests (which read the config to check it).
        let app_code = text.split("#[cfg(test)]\nmod tests").next().unwrap();
        // Comments may name them; code may not.
        for (n, line) in app_code.lines().enumerate() {
            let code = line.split("//").next().unwrap();
            for hook in hooks {
                assert!(
                    !code.contains(hook),
                    "src-tauri/src/{name}:{} has {hook}: only update_hook.rs may",
                    n + 1
                );
            }
        }
    }
    let lib = read("src-tauri/src/lib.rs");
    let lines: Vec<&str> = lib.lines().collect();
    let mut uses = 0;
    for (n, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap();
        if code.contains("update_hook") {
            uses += 1;
            assert_eq!(
                lines[n - 1].trim(),
                "#[cfg(feature = \"perf\")]",
                "lib.rs:{} uses update_hook outside a perf build",
                n + 1
            );
        }
    }
    assert!(
        uses >= 3,
        "the module, the environment and the source: {uses}"
    );
    // The hook obeys the perf build's identifier only.
    let hook = read("src-tauri/src/update_hook.rs");
    assert!(hook.contains("pub const PERF_IDENTIFIER: &str = \"com.shakirul.gg.perf\";"));
    assert!(hook.contains("identifier != PERF_IDENTIFIER"));
}

/// The page is granted none of the updater's commands: the core drives it.
#[test]
fn the_page_cannot_use_the_updater() {
    for entry in fs::read_dir(root().join("src-tauri/capabilities")).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("updater:"),
            "{} grants an updater permission",
            path.display()
        );
    }
    let package = json("package.json");
    assert!(
        package["dependencies"]
            .get("@tauri-apps/plugin-updater")
            .is_none()
    );
}
