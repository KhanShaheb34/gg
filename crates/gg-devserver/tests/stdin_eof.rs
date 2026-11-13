//! `--exit-on-stdin-eof`: the server quits when its stdin closes, so a test runner that dies
//! (Ctrl-C) does not leave it running.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn quits_when_stdin_closes() {
    let config = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_gg-devserver"))
        .args(["--port", "0", "--config-dir"])
        .arg(config.path())
        .arg("--exit-on-stdin-eof")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.starts_with("listening on"), "{line:?}");
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            return;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("still running 10 s after its stdin closed");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
