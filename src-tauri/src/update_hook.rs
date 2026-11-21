//! Performance and test builds only (`--features perf`; this file is not compiled into a
//! normal or release build): where the perf build looks for updates, for
//! `scripts/update-proof.sh`, which serves a test update on 127.0.0.1.
//!
//! - The perf build (bundle identifier `com.shakirul.gg.perf`) checks for updates only when
//!   it is given a test source: `GG_UPDATE_ENDPOINT` (an `http://127.0.0.1:<port>/...` URL)
//!   and `GG_UPDATE_PUBKEY` (the test key's public half). Without both, or under any other
//!   identifier, it does not update at all, so performance runs never reach the network.
//! - `--perf-env <file>` among the arguments sets the environment from `<file>` (lines of
//!   `NAME=value`, only the names below) before anything reads it: a restart to update goes
//!   through LaunchServices, which starts the app with the arguments it had but none of the
//!   environment, and the restarted copy must use the same config folder and test source.

use serde_json::{Value, json};
use std::ffi::OsString;
use std::path::PathBuf;

/// The perf build's identifier: the only one a test source is used under.
pub const PERF_IDENTIFIER: &str = "com.shakirul.gg.perf";

/// The names `--perf-env` may set.
const ENV_NAMES: &[&str] = &[
    "GG_CONFIG_DIR",
    "GG_OPEN",
    "GG_PERF_LOG",
    "GG_PERF_SCROLL",
    "GG_PERF_LAUNCHES",
    "GG_PERF_WINDOW",
    "GG_UPDATE_ENDPOINT",
    "GG_UPDATE_PUBKEY",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
    "SHELL",
    "PATH",
];

/// The file named by `--perf-env <file>` in `args` (the program first).
fn env_file(args: &[OsString]) -> Option<PathBuf> {
    let at = args.iter().position(|a| a == "--perf-env")?;
    args.get(at + 1).map(PathBuf::from)
}

/// The `NAME=value` lines of `text` whose names may be set.
fn env_lines(text: &str) -> Vec<(&str, &str)> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| ENV_NAMES.contains(name))
        .collect()
}

/// Sets the environment from `--perf-env <file>`, if the arguments have one. Call first
/// thing, before any thread starts.
pub fn load_env() {
    let args: Vec<OsString> = std::env::args_os().collect();
    let Some(file) = env_file(&args) else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&file) else {
        return;
    };
    for (name, value) in env_lines(&text) {
        // SAFETY: called at the start of `run`, before the app starts any thread, so nothing
        // reads the environment meanwhile.
        unsafe { std::env::set_var(name, value) };
    }
}

/// The updater's config for a test source, if `identifier` is the perf build's and the
/// endpoint is plain HTTP on 127.0.0.1 (the test server); `None` otherwise.
fn test_source(identifier: &str, endpoint: Option<&str>, pubkey: Option<&str>) -> Option<Value> {
    let (endpoint, pubkey) = (endpoint?, pubkey?);
    if identifier != PERF_IDENTIFIER || pubkey.trim().is_empty() {
        return None;
    }
    let url = tauri::Url::parse(endpoint).ok()?;
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
        return None;
    }
    Some(json!({
        "pubkey": pubkey.trim(),
        "endpoints": [endpoint],
        "dangerousInsecureTransportProtocol": true,
        "requireSignedVersion": true,
    }))
}

/// Points the updater at the test source given in the environment; whether there is one
/// (without one the perf build does not update).
pub fn apply<R: tauri::Runtime>(context: &mut tauri::Context<R>) -> bool {
    let identifier = context.config().identifier.clone();
    let endpoint = std::env::var("GG_UPDATE_ENDPOINT").ok();
    let pubkey = std::env::var("GG_UPDATE_PUBKEY").ok();
    match test_source(&identifier, endpoint.as_deref(), pubkey.as_deref()) {
        Some(config) => {
            context
                .config_mut()
                .plugins
                .0
                .insert("updater".to_owned(), config);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_source_only_for_the_perf_build_on_127_0_0_1() {
        let ok = test_source(
            PERF_IDENTIFIER,
            Some("http://127.0.0.1:8123/latest.json"),
            Some("KEY"),
        )
        .unwrap();
        assert_eq!(
            ok["endpoints"],
            json!(["http://127.0.0.1:8123/latest.json"])
        );
        assert_eq!(ok["pubkey"], "KEY");
        assert_eq!(ok["requireSignedVersion"], true);
        for (id, endpoint, key) in [
            (
                "com.shakirul.gg",
                Some("http://127.0.0.1:8123/l.json"),
                Some("K"),
            ),
            (
                PERF_IDENTIFIER,
                Some("https://example.com/l.json"),
                Some("K"),
            ),
            (
                PERF_IDENTIFIER,
                Some("http://localhost:8123/l.json"),
                Some("K"),
            ),
            (
                PERF_IDENTIFIER,
                Some("http://127.0.0.2:8123/l.json"),
                Some("K"),
            ),
            (PERF_IDENTIFIER, Some("http://127.0.0.1:8123/l.json"), None),
            (
                PERF_IDENTIFIER,
                Some("http://127.0.0.1:8123/l.json"),
                Some(" "),
            ),
            (PERF_IDENTIFIER, None, Some("K")),
            (PERF_IDENTIFIER, Some("not a url"), Some("K")),
        ] {
            assert_eq!(
                test_source(id, endpoint, key),
                None,
                "{id} {endpoint:?} {key:?}"
            );
        }
    }

    #[test]
    fn the_env_file_sets_only_known_names() {
        let args: Vec<OsString> = ["gg-desktop", "--perf-env", "/tmp/env"]
            .map(OsString::from)
            .into();
        assert_eq!(env_file(&args), Some(PathBuf::from("/tmp/env")));
        assert_eq!(env_file(&args[..1]), None);
        assert_eq!(env_file(&args[..2]), None);
        assert_eq!(
            env_lines("GG_CONFIG_DIR=/c\nHOME=/h\nnot a line\nPATH=/a:/b=c\n"),
            [("GG_CONFIG_DIR", "/c"), ("PATH", "/a:/b=c")]
        );
    }
}
