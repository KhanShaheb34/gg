//! The version is set once, in the workspace's Cargo.toml: the app (Tauri reads Cargo's
//! version when its config names none), the About section and the `gg` command all take it
//! from there; package.json is kept equal to it here.

use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn json(rel: &str) -> serde_json::Value {
    let path = root().join(rel);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn package_json_has_the_cargo_version() {
    assert_eq!(
        json("package.json")["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "package.json's version differs from the workspace Cargo.toml's"
    );
}

#[test]
fn the_tauri_configs_take_the_cargo_version() {
    for rel in [
        "src-tauri/tauri.conf.json",
        "src-tauri/tauri.linux.conf.json",
        "src-tauri/tauri.windows.conf.json",
    ] {
        assert!(
            json(rel).get("version").is_none(),
            "{rel} names a version of its own; the workspace Cargo.toml's is the one"
        );
    }
}

/// About and `gg --version` say gg-core's version: every crate takes the workspace's, none
/// a version of its own.
#[test]
fn every_crate_takes_the_workspace_version() {
    for rel in [
        "crates/gg-core/Cargo.toml",
        "crates/gg-devserver/Cargo.toml",
        "crates/gg-release/Cargo.toml",
        "src-tauri/Cargo.toml",
    ] {
        let path = root().join(rel);
        let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let package = text
            .split("\n[")
            .find(|section| section.starts_with("[package]") || section.starts_with("package]"))
            .unwrap_or_else(|| panic!("{rel} has no [package]"));
        let versions: Vec<&str> = package
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("version"))
            .collect();
        assert_eq!(
            versions,
            ["version.workspace = true"],
            "{rel}: the version must be the workspace's"
        );
    }
}

/// The shipped identifier is com.shakirul.gg, in normal and release builds alike: only the
/// perf/walk build (scripts/perf.sh) is made under its own,
/// com.shakirul.gg.perf, through a `--config` overlay on the command line. No config file
/// and no release or CI build names another, and the `gg` launcher finds the shipped one.
#[test]
fn the_shipped_identifier_is_com_shakirul_gg() {
    const SHIPPED: &str = "com.shakirul.gg";
    const PERF: &str = "com.shakirul.gg.perf";
    assert_eq!(
        json("src-tauri/tauri.conf.json")["identifier"].as_str(),
        Some(SHIPPED)
    );
    for rel in [
        "src-tauri/tauri.linux.conf.json",
        "src-tauri/tauri.windows.conf.json",
    ] {
        assert!(
            json(rel).get("identifier").is_none(),
            "{rel} names an identifier of its own"
        );
    }
    let read = |rel: &str| {
        let path = root().join(rel);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };
    for rel in [
        "scripts/release-macos.sh",
        ".github/workflows/release.yml",
        ".github/workflows/ci.yml",
        "package.json",
    ] {
        let text = read(rel);
        assert!(!text.contains(PERF), "{rel} builds the perf identifier");
        assert!(
            !text.contains("\"identifier\""),
            "{rel} overrides the identifier"
        );
    }
    assert!(
        read("src-tauri/resources/gg").contains(&format!("GG_BUNDLE_ID='{SHIPPED}'\n")),
        "the launcher finds the shipped app"
    );
    // The perf build's identifier is its own, given on the command line only.
    let perf = read("scripts/perf.sh");
    assert!(perf.contains(&format!("bundle_id={PERF}\n")));
    assert!(perf.contains("--config \"{\\\"identifier\\\":\\\"$bundle_id\\\"}\""));
}
