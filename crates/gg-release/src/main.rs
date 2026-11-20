//! `gg-release`: the release's updater files, checked before the draft release is made.
//!
//! ```text
//! gg-release verify --config <tauri.conf.json> --version <v> <file>...
//!     Each <file>'s signature (<file>.sig) verifies with the config's public key and was
//!     made for version <v>.
//! gg-release manifest --version <v> --tag <tag> --pub-date <rfc3339> --notes <text>
//!                     --dir <dir> --out <latest.json>
//!     Writes the updater manifest from the .sig files in <dir>.
//! gg-release check-manifest --config <tauri.conf.json> --version <v> --tag <tag>
//!                           --dir <dir> <latest.json> <uploaded file>...
//!     The manifest names <v>, only <tag>'s assets among the uploaded ones, and signatures
//!     that verify for the files in <dir>.
//! ```
//!
//! Exits 1 saying what is wrong (one line each), 2 on a usage error.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gg_release::{check_manifest, manifest, pubkey_from_config, verify};

const USAGE: &str = "usage: gg-release verify --config <tauri.conf.json> --version <v> <file>...
       gg-release manifest --version <v> --tag <tag> --pub-date <rfc3339> --notes <text> --dir <dir> --out <file>
       gg-release check-manifest --config <tauri.conf.json> --version <v> --tag <tag> --dir <dir> <latest.json> <uploaded>...";

/// `--name value` options and the other arguments, in order.
fn split(args: &[String]) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let mut options = HashMap::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if let Some(name) = arg.strip_prefix("--") {
            let value = it.next().ok_or_else(|| format!("--{name} needs a value"))?;
            options.insert(name.to_owned(), value.clone());
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((options, rest))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_text(path: &Path) -> Result<String, String> {
    String::from_utf8(read(path)?).map_err(|_| format!("{} is not text", path.display()))
}

fn run(args: &[String]) -> Result<Vec<String>, String> {
    let Some((command, args)) = args.split_first() else {
        return Err("no command".into());
    };
    let (options, rest) = split(args)?;
    let opt = |name: &str| {
        options
            .get(name)
            .cloned()
            .ok_or_else(|| format!("--{name} is required"))
    };
    let pubkey = || pubkey_from_config(&read_text(Path::new(&opt("config")?))?);
    match command.as_str() {
        "verify" => {
            let (key, version) = (pubkey()?, opt("version")?);
            if rest.is_empty() {
                return Err("no file to verify".into());
            }
            let mut problems = Vec::new();
            for file in &rest {
                let path = PathBuf::from(file);
                let sig = read_text(&PathBuf::from(format!("{file}.sig")));
                let checked = read(&path)
                    .and_then(|data| sig.and_then(|sig| verify(&key, &data, &sig, &version)));
                match checked {
                    Ok(()) => println!("verified {file} (version {version})"),
                    Err(e) => problems.push(format!("{file}: {e}")),
                }
            }
            Ok(problems)
        }
        "manifest" => {
            let dir = PathBuf::from(opt("dir")?);
            let m = manifest(
                &opt("version")?,
                &opt("tag")?,
                &opt("notes")?,
                &opt("pub-date")?,
                |file| read_text(&dir.join(format!("{file}.sig"))),
            )?;
            let text = serde_json::to_string_pretty(&m).map_err(|e| e.to_string())? + "\n";
            let out = opt("out")?;
            std::fs::write(&out, text).map_err(|e| format!("{out}: {e}"))?;
            println!("wrote {out}");
            Ok(Vec::new())
        }
        "check-manifest" => {
            let (key, version, tag) = (pubkey()?, opt("version")?, opt("tag")?);
            let dir = PathBuf::from(opt("dir")?);
            let Some((file, uploaded)) = rest.split_first() else {
                return Err("no manifest".into());
            };
            let m: serde_json::Value = serde_json::from_str(&read_text(Path::new(file))?)
                .map_err(|e| format!("{file} is not JSON: {e}"))?;
            let problems =
                check_manifest(&m, &version, &tag, &key, uploaded, |f| read(&dir.join(f)));
            if problems.is_empty() {
                println!(
                    "{file} names {version} and {tag}'s uploaded assets, each signature verified"
                );
            }
            Ok(problems)
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(problems) if problems.is_empty() => ExitCode::SUCCESS,
        Ok(problems) => {
            for p in problems {
                eprintln!("gg-release: {p}");
            }
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("gg-release: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
