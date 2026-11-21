//! `gg-release`: the release's updater files, checked before the draft release is made.
//!
//! ```text
//! gg-release sign --version <v> <file>...
//!     Signs each <file> into <file>.sig for version <v>, as `tauri signer sign --app-version`
//!     does, with the key in TAURI_SIGNING_PRIVATE_KEY (as `tauri signer generate` wrote it)
//!     and its password in TAURI_SIGNING_PRIVATE_KEY_PASSWORD. Without them it signs nothing.
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

use gg_release::{check_manifest, manifest, pubkey_from_config, sign, verify};

const USAGE: &str = "usage: gg-release sign --version <v> <file>...
       gg-release verify --config <tauri.conf.json> --version <v> <file>...
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
        "sign" => {
            let version = opt("version")?;
            if rest.is_empty() {
                return Err("no file to sign".into());
            }
            // The key and password are read here and never printed.
            let key = std::env::var("TAURI_SIGNING_PRIVATE_KEY").unwrap_or_default();
            if key.trim().is_empty() {
                return Err("TAURI_SIGNING_PRIVATE_KEY is not set: nothing is signed (the release needs the repository secrets TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD)".into());
            }
            let Ok(password) = std::env::var("TAURI_SIGNING_PRIVATE_KEY_PASSWORD") else {
                return Err(
                    "TAURI_SIGNING_PRIVATE_KEY_PASSWORD is not set: nothing is signed".into(),
                );
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs();
            // Every file is signed before any .sig is written: a failure leaves none.
            let mut signed = Vec::new();
            for file in &rest {
                let path = PathBuf::from(file);
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| format!("{file} has no file name"))?
                    .to_owned();
                let sig = sign(&key, &password, &read(&path)?, &name, &version, now)
                    .map_err(|e| format!("{file}: {e}"))?;
                signed.push((format!("{file}.sig"), sig));
            }
            for (out, sig) in signed {
                std::fs::write(&out, sig).map_err(|e| format!("{out}: {e}"))?;
                println!("signed {out} (version {version})");
            }
            Ok(Vec::new())
        }
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
