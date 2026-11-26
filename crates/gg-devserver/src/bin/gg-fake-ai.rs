//! A recording stand-in for the command-line AI tools (`claude`, `codex`), for tests and the
//! devserver's end-to-end runs only: copied into the tool `PATH` under a tool's name, it never
//! reaches any AI service. It is a program rather than a script so that it runs as itself on
//! Windows too.
//!
//! Its name (`claude`, `codex`) is the file it was started as (`argv[0]`), and its state is
//! the folder `<its folder>/<name>.fake/`:
//!
//! - `version`: what `--version` prints (else a made-up version).
//! - each call is recorded in `calls/<n>/` (n from 1): `argv` (arguments, each ended by NUL),
//!   `stdin`, `cwd`, `env` (`NAME=value` lines), `started`, and `done` once it answered.
//! - the answer is `reply.out` on stdout and `reply.err` on stderr, then exit `reply.code`
//!   (0 when missing), after waiting `reply.sleep` milliseconds. With
//!   `--output-last-message <file>` (Codex), `reply.last` (else `reply.out`) is written there.
//!   With `reply.echo`, what it read on stdin is written to stderr first, as Codex does.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let me = PathBuf::from(args.first().cloned().unwrap_or_default());
    let name = me
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = me
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("{name}.fake"));
    let args = &args[1..];
    let read = |file: &str| fs::read(dir.join(file)).ok();
    let text = |file: &str| read(file).map(|b| String::from_utf8_lossy(&b).into_owned());

    if args == ["--version"] {
        let version = text("version").unwrap_or_else(|| match name.as_str() {
            "codex" => "codex-cli 0.0.1\n".to_owned(),
            _ => "0.0.1 (Claude Code)\n".to_owned(),
        });
        print!("{version}");
        return;
    }

    let calls = dir.join("calls");
    let _ = fs::create_dir_all(&calls);
    let mut n = 1;
    let call = loop {
        let c = calls.join(n.to_string());
        match fs::create_dir(&c) {
            Ok(()) => break c,
            Err(_) if n < 10_000 => n += 1,
            Err(e) => {
                eprintln!("fake: {e}");
                std::process::exit(99);
            }
        }
    };
    let mut argv = Vec::new();
    for a in args {
        argv.extend_from_slice(a.as_bytes());
        argv.push(0);
    }
    let _ = fs::write(call.join("argv"), argv);
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = fs::write(call.join("cwd"), cwd);
    let env: String = std::env::vars().map(|(k, v)| format!("{k}={v}\n")).collect();
    let _ = fs::write(call.join("env"), env);
    let mut stdin = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin);
    let _ = fs::write(call.join("stdin"), &stdin);
    let _ = fs::write(call.join("started"), "");

    if dir.join("reply.echo").exists() {
        let mut err = std::io::stderr();
        let _ = err.write_all(b"OpenAI Codex v0.0.1\n--------\nmodel: fake\n--------\nuser\n");
        let _ = err.write_all(&stdin);
        let _ = err.write_all(b"\n");
    }
    if let Some(ms) = text("reply.sleep").and_then(|s| s.trim().parse::<u64>().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
    if let Some(err) = read("reply.err") {
        let _ = std::io::stderr().write_all(&err);
    }
    let out = read("reply.out").unwrap_or_default();
    if let Some(at) = args
        .iter()
        .position(|a| a == "--output-last-message" || a == "-o")
        && let Some(file) = args.get(at + 1)
    {
        let last = read("reply.last").unwrap_or_else(|| out.clone());
        let _ = fs::write(file, last);
    }
    let _ = std::io::stdout().write_all(&out);
    let _ = std::io::stdout().flush();
    let _ = fs::write(call.join("done"), "");
    let code = text("reply.code")
        .and_then(|c| c.trim().parse::<i32>().ok())
        .unwrap_or(0);
    std::process::exit(code);
}
