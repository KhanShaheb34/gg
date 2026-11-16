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
