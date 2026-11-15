//! The app is called gg: its development name appears nowhere in the code, the app or the
//! README, except the README's one line crediting the editor extension that inspired it and
//! the old config folder name the store migrates from. The history in `docs/` keeps it.

use std::fs;
use std::path::{Path, PathBuf};

/// What is scanned, relative to the repository root: every text file under a folder.
const SCANNED: &[&str] = &[
    "src",
    "src-tauri",
    "crates",
    "scripts",
    "e2e",
    "public",
    "index.html",
    "package.json",
    "vite.config.ts",
    "Cargo.toml",
    "README.md",
];

/// Folders never scanned: build output and generated files.
const SKIPPED: &[&str] = &["target", "node_modules", "gen"];

/// The old name in lower case, in two parts so that this file does not hold it.
const OLD: [&str; 2] = ["git", "graph"];

/// Whether `line` holds the old name in any case, its parts joined by nothing, a space, an
/// underscore or a hyphen.
fn has_old_name(line: &str) -> bool {
    let lower = line.to_lowercase();
    ["", " ", "_", "-"]
        .iter()
        .any(|sep| lower.contains(&OLD.join(sep)))
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn text_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if SKIPPED.contains(&name) {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            text_files(&entry.unwrap().path(), out);
        }
    } else {
        out.push(path.to_path_buf());
    }
}

#[test]
fn the_old_name_is_gone() {
    let root = root();
    let mut files = Vec::new();
    for scanned in SCANNED {
        let path = root.join(scanned);
        assert!(path.exists(), "{} is missing", path.display());
        text_files(&path, &mut files);
    }
    let mut found = Vec::new();
    let mut credits = 0;
    for file in &files {
        // Binary files (icons) are not text.
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for (n, line) in text.lines().enumerate() {
            if !has_old_name(line) {
                continue;
            }
            // The extension gg is inspired by, credited once.
            if rel == "README.md" && line.contains("VS Code") && credits == 0 {
                credits += 1;
                continue;
            }
            // The config folder the app had under its old identifier, migrated from.
            if rel == "crates/gg-core/src/store.rs"
                && line.starts_with("const OLD_CONFIG_DIR_NAME: &str = ")
            {
                continue;
            }
            found.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }
    assert!(
        found.is_empty(),
        "the old name is still used:\n{}",
        found.join("\n")
    );
    assert_eq!(credits, 1, "README.md credits the VS Code extension once");
}
