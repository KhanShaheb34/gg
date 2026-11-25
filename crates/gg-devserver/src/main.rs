//! `gg-devserver --port 1421 --config-dir <dir> [--open <path>] [--allow-origin <origin>]...`:
//! serves gg-core on 127.0.0.1 for the browser build of the UI. Browsers may call it from
//! `http://127.0.0.1:1420` and `http://localhost:1420` (the vite dev server) and from each
//! `--allow-origin`. Prints `listening on http://127.0.0.1:<port>` once ready (`--port 0`
//! picks a free port). Env `GG_PERF_LOG` names a file perf marks are appended to. With
//! `--exit-on-stdin-eof` it quits when its stdin closes (a test runner that started it died).
//! With `--record-launches <file>` it never starts a program (editor, terminal, file manager,
//! browser): each launch is appended to the file as a line of JSON instead (for the UI's
//! tests). The `gg` command is looked for on `--cli-path <PATH>` instead of the login shell's
//! `PATH`, and `--cli-shell <program>` is asked what `gg` is instead of `$SHELL` (the UI's
//! tests pass both: temp folders and a stand-in shell, never the user's own). With
//! `--tool-path <PATH>`, `gh` and `ssh` are found and run on that `PATH` instead of the login
//! shell's (the UI's tests pass a folder holding a fake `gh` and `ssh`, never the real ones).
//! With `--fake-updates <dir>`, updating gg is a stand-in that answers from files in `<dir>`
//! (see `fake_updates.rs`); without it, updating is unavailable. Nothing is downloaded.
//! `--pr-details-every-ms <ms>` sets the least time between two loads of a pull request's
//! checks and comments (10 s otherwise), for the UI's tests of its polling.

use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use gg_core::Core;
use gg_devserver::fake_updates::FakeUpdates;
use gg_devserver::{DEFAULT_ORIGINS, Events, serve};
use tokio::net::TcpListener;

const USAGE: &str = "usage: gg-devserver [--port <port>] --config-dir <dir> [--open <path>] \
                     [--allow-origin <origin>]... [--exit-on-stdin-eof] \
                     [--record-launches <file>] [--cli-path <PATH>] \
                     [--cli-shell <program>] [--tool-path <PATH>] \
                     [--fake-updates <dir>] [--pr-details-every-ms <ms>]";
const DEFAULT_PORT: u16 = 1421;

#[derive(Debug, PartialEq)]
struct Args {
    port: u16,
    config_dir: PathBuf,
    open: Option<PathBuf>,
    /// Origins allowed besides [`DEFAULT_ORIGINS`].
    allow_origins: Vec<String>,
    /// Quit when stdin reaches its end.
    exit_on_stdin_eof: bool,
    /// Where editor launches are recorded instead of run.
    record_launches: Option<PathBuf>,
    /// The `PATH` the `gg` command is looked for on.
    cli_path: Option<OsString>,
    /// The shell asked what `gg` is.
    cli_shell: Option<PathBuf>,
    /// The `PATH` `gh` and `ssh` are found and run on.
    tool_path: Option<OsString>,
    /// The folder the stand-in for the updater answers from.
    fake_updates: Option<PathBuf>,
    /// Least time between two loads of a pull request's details, instead of 10 s.
    pr_details_every: Option<std::time::Duration>,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut args = args.into_iter();
    let mut port = DEFAULT_PORT;
    let mut config_dir = None;
    let mut open = None;
    let mut allow_origins = Vec::new();
    let mut exit_on_stdin_eof = false;
    let mut record_launches = None;
    let mut cli_path = None;
    let mut cli_shell = None;
    let mut tool_path = None;
    let mut fake_updates = None;
    let mut pr_details_every = None;
    while let Some(flag) = args.next() {
        let flag = flag.to_string_lossy().into_owned();
        let mut value = || args.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--port" => {
                let v = value()?;
                port = v
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("bad port: {}", v.to_string_lossy()))?;
            }
            "--config-dir" => config_dir = Some(PathBuf::from(value()?)),
            "--open" => {
                let path = PathBuf::from(value()?);
                if !path.is_dir() {
                    return Err(format!("--open: not a folder: {}", path.display()));
                }
                open = Some(path);
            }
            "--allow-origin" => {
                let origin = value()?.to_string_lossy().into_owned();
                if !is_origin(&origin) {
                    return Err(format!(
                        "--allow-origin: not an origin (scheme://host[:port]): {origin}"
                    ));
                }
                allow_origins.push(origin);
            }
            "--exit-on-stdin-eof" => exit_on_stdin_eof = true,
            "--record-launches" => record_launches = Some(PathBuf::from(value()?)),
            "--cli-path" => cli_path = Some(value()?),
            "--cli-shell" => cli_shell = Some(PathBuf::from(value()?)),
            "--tool-path" => tool_path = Some(value()?),
            "--fake-updates" => fake_updates = Some(PathBuf::from(value()?)),
            "--pr-details-every-ms" => {
                let v = value()?;
                let ms: u64 = v
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| format!("bad milliseconds: {}", v.to_string_lossy()))?;
                pr_details_every = Some(std::time::Duration::from_millis(ms));
            }
            _ => return Err(format!("unknown argument: {flag}")),
        }
    }
    Ok(Args {
        port,
        config_dir: config_dir.ok_or("--config-dir is required")?,
        open,
        allow_origins,
        exit_on_stdin_eof,
        record_launches,
        cli_path,
        cli_shell,
        tool_path,
        fake_updates,
        pr_details_every,
    })
}

/// Whether `s` is a web origin: `http(s)://host[:port]` with no path, where host is a name
/// or IPv4 address (letters, digits, `.` and `-`, not starting with `-`) or a bracketed IPv6
/// address, and port is 1 to 65535.
fn is_origin(s: &str) -> bool {
    let Some(rest) = s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
    else {
        return false;
    };
    let (host_ok, port) = if let Some(v6) = rest.strip_prefix('[') {
        let Some((addr, after)) = v6.split_once(']') else {
            return false;
        };
        let ok = !addr.is_empty() && addr.bytes().all(|b| b.is_ascii_hexdigit() || b == b':');
        match after.strip_prefix(':') {
            Some(port) => (ok, Some(port)),
            None if after.is_empty() => (ok, None),
            None => return false,
        }
    } else {
        let (host, port) = match rest.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (rest, None),
        };
        let ok = !host.is_empty()
            && !host.starts_with('-')
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
        (ok, port)
    };
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u16>().is_ok_and(|n| n > 0)
    });
    host_ok && port_ok
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args(std::env::args_os().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.exit_on_stdin_eof {
        // Whoever started it holds its stdin open; when that end closes, it is gone.
        std::thread::spawn(|| {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            std::process::exit(0);
        });
    }
    let events = Events::new();
    let core = match Core::new(args.config_dir, args.open, events.sink()).await {
        Ok(core) => core,
        Err(e) => {
            eprintln!("gg-devserver: {e}");
            return ExitCode::FAILURE;
        }
    };
    core.set_perf_log(std::env::var_os("GG_PERF_LOG").map(PathBuf::from));
    core.record_launches_to(args.record_launches);
    if args.tool_path.is_some() {
        core.set_tool_path(args.tool_path);
    }
    if let Some(every) = args.pr_details_every {
        core.set_pr_details_every(every);
    }
    if let Some(dir) = args.fake_updates {
        core.set_update_backend(Arc::new(FakeUpdates::new(dir)));
    }
    core.start_cli(gg_core::cli::CliSetup {
        shell: args
            .cli_shell
            .or_else(|| std::env::var_os("SHELL").map(PathBuf::from)),
        search_path: args.cli_path,
        home: None,
    });
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, args.port)).await {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("gg-devserver: cannot listen on port {}: {e}", args.port);
            return ExitCode::FAILURE;
        }
    };
    match listener.local_addr() {
        Ok(addr) => println!("listening on http://{addr}"),
        Err(e) => eprintln!("gg-devserver: {e}"),
    }
    let origins = DEFAULT_ORIGINS
        .iter()
        .map(|o| (*o).to_owned())
        .chain(args.allow_origins)
        .collect();
    if let Err(e) = serve(listener, core, events, origins).await {
        eprintln!("gg-devserver: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn flags() {
        let dir = std::env::temp_dir();
        let dir_s = dir.to_str().unwrap();
        assert_eq!(
            parse(&["--config-dir", "/c"]).unwrap(),
            Args {
                port: 1421,
                config_dir: "/c".into(),
                open: None,
                allow_origins: vec![],
                exit_on_stdin_eof: false,
                record_launches: None,
                cli_path: None,
                cli_shell: None,
                tool_path: None,
                fake_updates: None,
                pr_details_every: None,
            }
        );
        assert_eq!(
            parse(&[
                "--port",
                "0",
                "--config-dir",
                "/c",
                "--open",
                dir_s,
                "--allow-origin",
                "http://127.0.0.1:5173",
                "--allow-origin",
                "http://[::1]:5173",
                "--record-launches",
                "/e.jsonl",
                "--cli-path",
                "/a:/b",
                "--cli-shell",
                "/fake/sh",
                "--tool-path",
                "/fake/bin",
                "--fake-updates",
                "/fake/updates",
                "--pr-details-every-ms",
                "250",
            ])
            .unwrap(),
            Args {
                port: 0,
                config_dir: "/c".into(),
                open: Some(dir.clone()),
                allow_origins: vec!["http://127.0.0.1:5173".into(), "http://[::1]:5173".into()],
                exit_on_stdin_eof: false,
                record_launches: Some("/e.jsonl".into()),
                cli_path: Some("/a:/b".into()),
                cli_shell: Some("/fake/sh".into()),
                tool_path: Some("/fake/bin".into()),
                fake_updates: Some("/fake/updates".into()),
                pr_details_every: Some(std::time::Duration::from_millis(250)),
            }
        );
        assert!(parse(&["--config-dir", "/c", "--pr-details-every-ms", "soon"]).is_err());
        for bad in [
            "*",
            "null",
            "127.0.0.1:5173",
            "http://",
            "http://x/path",
            "http://a b",
            "http://a:b:c",
            "http://:5173",
            "http://host:",
            "http://host:99999",
            "http://[::1",
            "http://h]:1",
            "http://-host",
        ] {
            let args = ["--config-dir", "/c", "--allow-origin", bad];
            assert!(parse(&args).is_err(), "{bad} accepted");
        }
        assert!(parse(&[]).unwrap_err().contains("--config-dir"));
        assert!(parse(&["--port", "x", "--config-dir", "/c"]).is_err());
        assert!(parse(&["--port"]).unwrap_err().contains("needs a value"));
        assert!(parse(&["--config-dir", "/c", "--open", "/no/such/dir"]).is_err());
        assert!(parse(&["--verbose"]).unwrap_err().contains("unknown"));
    }
}
