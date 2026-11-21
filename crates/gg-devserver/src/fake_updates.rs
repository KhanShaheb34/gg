//! A stand-in for the app's updater (`--fake-updates <dir>`), so the UI's tests can walk
//! through every update state without a network or an app bundle. It answers from files in
//! `<dir>`, read at each call, which the test writes:
//!
//! - `check.json`: `{"offer": "0.4.1"}` (a version, or `null`: nothing newer), or
//!   `{"fail": "<kind>", "detail": "..."}` with kind `offline`, `notFound`, `badManifest`,
//!   `noDownload`, `signature`, `install` or `other`. Missing: nothing newer.
//! - `download.json`: `{"fail": ..., "detail": ...}` as above; missing: the download works.
//!   While `download-gate` exists, a download stops halfway and waits for it to go.
//! - `blocker`: why this copy cannot update itself: `translocated`, `diskImage`,
//!   `notWritable <folder>`, `linuxPackage` or `notInstalled`. Missing: it can.
//! - `install.json`: `{"fail": ...}`; missing: the install works.
//! - `version`: the running version; missing: gg-core's.
//!
//! Every restart asked for is appended to `<dir>/restarts` (one line: the version), and
//! nothing restarts.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gg_core::update::{
    Blocker, BoxFuture, FailureKind, Offer, Progress, UpdateBackend, UpdateFailure,
};
use serde::Deserialize;

pub struct FakeUpdates {
    dir: PathBuf,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    #[serde(default)]
    offer: Option<String>,
    #[serde(default)]
    fail: Option<String>,
    #[serde(default)]
    detail: String,
}

impl FakeUpdates {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn answer(&self, name: &str) -> Option<Answer> {
        let text = fs::read_to_string(self.dir.join(name)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn failure(answer: &Answer) -> Option<UpdateFailure> {
        let kind = match answer.fail.as_deref()? {
            "offline" => FailureKind::Offline,
            "notFound" => FailureKind::NotFound,
            "badManifest" => FailureKind::BadManifest,
            "noDownload" => FailureKind::NoDownload,
            "signature" => FailureKind::Signature,
            "install" => FailureKind::Install,
            _ => FailureKind::Other,
        };
        Some(UpdateFailure::new(kind, answer.detail.clone()))
    }

    fn outcome(&self, name: &str) -> Result<(), UpdateFailure> {
        match self.answer(name).as_ref().and_then(Self::failure) {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }
}

/// `blocker`'s text as a [`Blocker`].
fn parse_blocker(text: &str) -> Option<Blocker> {
    let text = text.trim();
    let (word, rest) = text.split_once(' ').unwrap_or((text, ""));
    Some(match word {
        "translocated" => Blocker::Translocated,
        "diskImage" => Blocker::DiskImage,
        "notWritable" => Blocker::NotWritable(Path::new(rest.trim()).to_path_buf()),
        "linuxPackage" => Blocker::LinuxPackage,
        "notInstalled" => Blocker::NotInstalled,
        _ => return None,
    })
}

impl UpdateBackend for FakeUpdates {
    fn current_version(&self) -> String {
        fs::read_to_string(self.dir.join("version"))
            .map(|v| v.trim().to_owned())
            .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_owned())
    }

    fn blocker(&self) -> Option<Blocker> {
        parse_blocker(&fs::read_to_string(self.dir.join("blocker")).ok()?)
    }

    fn check(&self) -> BoxFuture<'_, Result<Option<Offer>, UpdateFailure>> {
        let answer = self.answer("check.json");
        Box::pin(async move {
            let Some(answer) = answer else {
                return Ok(None);
            };
            if let Some(failure) = Self::failure(&answer) {
                return Err(failure);
            }
            Ok(answer.offer.map(|version| Offer { version }))
        })
    }

    fn download<'a>(
        &'a self,
        _version: &'a str,
        mut progress: Progress<'a>,
    ) -> BoxFuture<'a, Result<(), UpdateFailure>> {
        let gate = self.dir.join("download-gate");
        Box::pin(async move {
            const TOTAL: u64 = 12_000_000;
            for step in 0..=10 {
                progress(TOTAL / 10 * step, Some(TOTAL));
                while step == 5 && gate.exists() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
            self.outcome("download.json")
        })
    }

    fn install_and_restart(&self, _version: &str) -> Result<(), UpdateFailure> {
        self.outcome("install.json")?;
        let version = self.current_version();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("restarts"))
            .map_err(|e| UpdateFailure::new(FailureKind::Install, e.to_string()))?;
        writeln!(file, "{version}")
            .map_err(|e| UpdateFailure::new(FailureKind::Install, e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blockers_are_read_by_name() {
        assert_eq!(parse_blocker("translocated\n"), Some(Blocker::Translocated));
        assert_eq!(
            parse_blocker("notWritable /Applications"),
            Some(Blocker::NotWritable(PathBuf::from("/Applications")))
        );
        assert_eq!(parse_blocker("linuxPackage"), Some(Blocker::LinuxPackage));
        assert_eq!(parse_blocker("nonsense"), None);
    }

    #[tokio::test]
    async fn it_answers_from_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let fake = FakeUpdates::new(dir.path().to_path_buf());
        assert_eq!(fake.check().await, Ok(None));
        fs::write(dir.path().join("check.json"), r#"{"offer": "9.0.0"}"#).unwrap();
        assert_eq!(
            fake.check().await,
            Ok(Some(Offer {
                version: "9.0.0".into()
            }))
        );
        fs::write(
            dir.path().join("check.json"),
            r#"{"fail": "offline", "detail": "no route"}"#,
        )
        .unwrap();
        assert_eq!(
            fake.check().await,
            Err(UpdateFailure::new(FailureKind::Offline, "no route"))
        );
        let mut seen = Vec::new();
        fake.download("9.0.0", Box::new(|r, t| seen.push((r, t))))
            .await
            .unwrap();
        assert_eq!(seen.len(), 11);
        fs::write(dir.path().join("version"), "0.3.9\n").unwrap();
        fake.install_and_restart("9.0.0").unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("restarts")).unwrap(),
            "0.3.9\n"
        );
    }
}
