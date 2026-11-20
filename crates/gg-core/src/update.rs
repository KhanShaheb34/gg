//! gg updating itself: whether a newer version exists, downloading it in the background, and
//! putting it in place when the user restarts.
//!
//! The work that touches the network and the app's own files is the transport's (the desktop
//! app drives Tauri's updater plugin; see `src-tauri/src/update.rs`), behind
//! [`UpdateBackend`]. What is decided here is the same everywhere: when to check (shortly
//! after the UI has shown its first rows, then about once a day by the wall clock, never while
//! a check or download runs), that only a strictly newer version is ever offered (no
//! downgrade, and no pre-release to a release build), whether this copy can replace itself
//! ([`blocker`]), and what the user is told, in plain words, in each state
//! ([`UpdateStatus`], sent to the UI as `update-changed` events).
//!
//! A check that fails is quiet: the status says why, and the next one is tried an hour later.
//! A download that is ready stays ready until the user restarts (or gg quits; it is looked for
//! again at the next start). gg never restarts by itself.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::Notify;

use crate::error::{CoreError, ErrorCode, Result};
use crate::{EventSink, lock_ignoring_poison};

/// The event every change of [`UpdateStatus`] is sent as.
pub const EVENT: &str = "update-changed";

/// Where gg's downloads are, for a copy that cannot update itself.
pub const DOWNLOAD_PAGE: &str = crate::launch::HOME_PAGE;

/// A boxed future, as the backend's methods return them.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Called as a download goes: bytes received so far, and the total when the server said.
pub type Progress<'a> = Box<dyn FnMut(u64, Option<u64>) + Send + 'a>;

/// What a check found: a version the update source offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
}

/// What does the work for [`Updates`]: the Tauri app's updater, or a fake in the devserver
/// and the tests.
pub trait UpdateBackend: Send + Sync + 'static {
    /// The running version.
    fn current_version(&self) -> String;
    /// Why this copy cannot replace itself, if it cannot.
    fn blocker(&self) -> Option<Blocker>;
    /// Asks the update source for the newest version: `None` when it is not newer.
    fn check(&self) -> BoxFuture<'_, std::result::Result<Option<Offer>, UpdateFailure>>;
    /// Downloads `version` (the last check's offer) and verifies its signature; keeps it for
    /// [`UpdateBackend::install_and_restart`].
    fn download<'a>(
        &'a self,
        version: &'a str,
        progress: Progress<'a>,
    ) -> BoxFuture<'a, std::result::Result<(), UpdateFailure>>;
    /// Puts the downloaded version in place (verifying it again) and restarts gg. Blocks; it
    /// returns only when that failed (or, in a fake, once it has been recorded).
    fn install_and_restart(&self) -> std::result::Result<(), UpdateFailure>;
}

/// What went wrong, by what the user can make of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// No answer: no network, the name did not resolve, the connection failed or timed out.
    Offline,
    /// The server answered, but not with the file (a 404, a 500).
    NotFound,
    /// The update information could not be read.
    BadManifest,
    /// The update information has no download for this system.
    NoDownload,
    /// The download's signature does not match gg's key, or was made for another version.
    Signature,
    /// The new version could not be put in place.
    Install,
    /// Anything else.
    Other,
}

/// A failed step: what kind, and the technical text (shown smaller, under the message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFailure {
    pub kind: FailureKind,
    pub detail: String,
}

impl UpdateFailure {
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// The step that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Step {
    Check,
    Download,
    Install,
}

/// The message for the user when `step` (of `version`, for a download or install) failed
/// with `kind`: what failed, then why, each a sentence.
pub fn failure_message(step: Step, version: Option<&str>, kind: FailureKind) -> String {
    let what = match (step, version) {
        (Step::Check, _) | (_, None) => "Could not check for updates.".to_owned(),
        (Step::Download, Some(v)) => format!("Could not download gg {v}."),
        (Step::Install, Some(v)) => format!("Could not install gg {v}."),
    };
    let why = match (kind, step) {
        (FailureKind::Offline, _) => "GitHub could not be reached.",
        (FailureKind::NotFound, Step::Check) => {
            "GitHub has no update information for the latest release."
        }
        (FailureKind::NotFound, _) => "The download was not found.",
        (FailureKind::BadManifest, _) => "The update information is not in a form gg reads.",
        (FailureKind::NoDownload, _) => "The new version has no download for this system.",
        (FailureKind::Signature, _) => {
            "Its signature does not match gg’s, so nothing was installed."
        }
        (FailureKind::Install, _) => "Its files could not be put in place.",
        (FailureKind::Other, _) => "Something went wrong.",
    };
    format!("{what} {why}")
}

/// Why this copy of gg cannot replace itself. Then a newer version is only announced, with
/// a link to the download page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// macOS runs the app from a random, read-only place (App Translocation): it was opened
    /// where it was downloaded or from its disk image, with its quarantine still on.
    Translocated,
    /// Running from a mounted disk image.
    DiskImage,
    /// The folder the app is in (or the app bundle itself) cannot be written to.
    NotWritable(PathBuf),
    /// Installed by a Linux package (.deb, .rpm): only its package manager updates it.
    LinuxPackage,
    /// Not an installed release: a development build, or a bare executable.
    NotInstalled,
}

impl Blocker {
    /// Why, for the user.
    pub fn reason(&self) -> String {
        match self {
            Self::Translocated => "macOS runs this copy of gg from a temporary place, so it cannot update itself. Move gg to the Applications folder and open it from there.".to_owned(),
            Self::DiskImage => "gg is running from its disk image, so it cannot update itself. Drag gg to the Applications folder and open it from there.".to_owned(),
            Self::NotWritable(dir) => format!(
                "gg cannot update itself: it may not change {}.",
                dir.display()
            ),
            Self::LinuxPackage => "gg was installed from a package, which your package manager updates; only the AppImage updates itself.".to_owned(),
            Self::NotInstalled => "This copy of gg was not installed from a release, so it does not update itself.".to_owned(),
        }
    }
}

/// How the running copy of gg was installed, as the transport sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// macOS: the `.app` bundle the executable is in.
    MacBundle(PathBuf),
    /// Linux: the AppImage file.
    AppImage(PathBuf),
    /// Linux: from a .deb or .rpm package.
    LinuxPackage,
    /// Windows: installed by the installer; the folder the executable is in.
    WindowsInstalled(PathBuf),
    /// Anything else (a development build, an executable outside a bundle).
    Unbundled,
}

/// The `.app` bundle executable `exe` is in (`<x>.app/Contents/MacOS/<exe>`), if it is in one.
pub fn mac_bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is = |p: &Path, name: &str| p.file_name().is_some_and(|n| n == name);
    let app = bundle
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (is(macos, "MacOS") && is(contents, "Contents") && app).then(|| bundle.to_path_buf())
}

/// Why `install` cannot replace itself, if it cannot. Looks at the file system: whether the
/// places the update must change can be written to.
pub fn blocker(install: &Install) -> Option<Blocker> {
    match install {
        Install::MacBundle(bundle) => {
            let text = bundle.to_string_lossy();
            if text.contains("/AppTranslocation/") {
                return Some(Blocker::Translocated);
            }
            let parent = bundle.parent().unwrap_or(Path::new("/"));
            // The bundle is swapped with the new one inside its folder: both are renamed, so
            // the folder and the bundle itself must be writable.
            if writable_dir(parent) && writable_dir(bundle) {
                return None;
            }
            if bundle.starts_with("/Volumes") {
                return Some(Blocker::DiskImage);
            }
            Some(Blocker::NotWritable(parent.to_path_buf()))
        }
        Install::AppImage(file) => {
            // The AppImage is moved aside and the new one written in its place.
            let parent = file.parent().unwrap_or(Path::new("/"));
            (!writable_dir(parent)).then(|| Blocker::NotWritable(parent.to_path_buf()))
        }
        Install::WindowsInstalled(dir) => {
            (!writable_dir(dir)).then(|| Blocker::NotWritable(dir.clone()))
        }
        Install::LinuxPackage => Some(Blocker::LinuxPackage),
        Install::Unbundled => Some(Blocker::NotInstalled),
    }
}

/// Whether the user may create and rename files in folder `dir`.
fn writable_dir(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        // Folder attributes say nothing on Windows: try it.
        if !dir.is_dir() {
            return false;
        }
        let probe = dir.join(format!(".gg-update-probe-{}", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(file) => {
                drop(file);
                let _ = std::fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        crate::cli::is_writable_dir(dir)
    }
}

/// Whether `offered` is a version to update `current` to: strictly newer by semantic
/// version precedence (build metadata does not count), and not a pre-release unless
/// `current` is one. Anything that is not a version is never newer.
pub fn is_newer(current: &str, offered: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim().trim_start_matches('v')).ok();
    let (Some(current), Some(offered)) = (parse(current), parse(offered)) else {
        return false;
    };
    offered.cmp_precedence(&current).is_gt() && (offered.pre.is_empty() || !current.pre.is_empty())
}

/// Where updating stands, as the UI shows it (Settings → About and the notice).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UpdateStatus {
    /// This build does not update itself (the browser build without a fake).
    Unavailable,
    /// Not checked yet.
    Idle,
    Checking,
    /// The newest version is this one, as of `checked_at_ms` (unix ms).
    UpToDate {
        checked_at_ms: u64,
    },
    Downloading {
        version: String,
        received: u64,
        total: Option<u64>,
    },
    /// Downloaded and verified: restarting puts it in place.
    Ready {
        version: String,
    },
    /// Being put in place; gg restarts next.
    Installing {
        version: String,
    },
    /// A newer version exists, but this copy cannot replace itself (`reason`); the user gets
    /// it from the download page.
    Available {
        version: String,
        reason: String,
    },
    Failed {
        step: Step,
        version: Option<String>,
        message: String,
        detail: Option<String>,
        at_ms: u64,
    },
}

/// When checks happen. Real values in [`Timing::default`]; the tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// From [`Updates::start`] to the first check.
    pub first_delay: Duration,
    /// How often the schedule looks whether a check is due (by the wall clock, so a machine
    /// that slept still checks once it wakes).
    pub tick: Duration,
    /// After a check that worked, the next one is due this much later.
    pub every: Duration,
    /// After one that failed.
    pub retry: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            first_delay: Duration::from_secs(3),
            tick: Duration::from_secs(10 * 60),
            every: Duration::from_secs(24 * 3600),
            retry: Duration::from_secs(3600),
        }
    }
}

/// The wall clock, unix ms.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

fn system_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct State {
    status: UpdateStatus,
    /// The version downloaded and verified, waiting for a restart.
    ready: Option<String>,
    /// The setting: automatic checks.
    enabled: bool,
    /// The schedule runs.
    started: bool,
    /// A check, download or install runs.
    busy: bool,
    /// When the next automatic check is due (unix ms); `None` once started: at once.
    due_ms: Option<u64>,
}

/// Updating, for one app: its state and its schedule.
pub struct Updates {
    backend: OnceLock<Arc<dyn UpdateBackend>>,
    state: Mutex<State>,
    sink: Arc<dyn EventSink>,
    wake: Notify,
    timing: Timing,
    clock: Clock,
}

impl Updates {
    /// Updating with no backend yet (status `unavailable`); `enabled`: the setting.
    pub fn new(sink: Arc<dyn EventSink>, enabled: bool) -> Arc<Self> {
        Self::with(sink, enabled, Timing::default(), Arc::new(system_clock))
    }

    /// As [`Updates::new`], with its timing and clock given (tests).
    pub fn with(
        sink: Arc<dyn EventSink>,
        enabled: bool,
        timing: Timing,
        clock: Clock,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend: OnceLock::new(),
            state: Mutex::new(State {
                status: UpdateStatus::Unavailable,
                ready: None,
                enabled,
                started: false,
                busy: false,
                due_ms: None,
            }),
            sink,
            wake: Notify::new(),
            timing,
            clock,
        })
    }

    /// Gives updating its backend (once; later ones are ignored): the status is then `idle`.
    pub fn set_backend(&self, backend: Arc<dyn UpdateBackend>) {
        if self.backend.set(backend).is_ok() {
            self.set_status(UpdateStatus::Idle);
        }
    }

    pub fn status(&self) -> UpdateStatus {
        self.lock().status.clone()
    }

    /// Starts the schedule (once; later calls do nothing): the first check after
    /// `Timing::first_delay` if checks are on, then whenever one is due. Call inside a Tokio
    /// runtime, once the UI has shown its first rows, so the check never slows the start.
    pub fn start(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            if state.started || self.backend.get().is_none() {
                return;
            }
            state.started = true;
        }
        let this = Arc::downgrade(self);
        let timing = self.timing;
        tokio::spawn(async move {
            tokio::time::sleep(timing.first_delay).await;
            loop {
                let Some(updates) = this.upgrade() else {
                    return;
                };
                if updates.due() {
                    updates.check().await;
                }
                let woken = updates.clone();
                drop(updates);
                let _ = tokio::time::timeout(timing.tick, woken.wake.notified()).await;
            }
        });
    }

    /// Turns automatic checks on or off (the setting). Turned on, a check that is due runs
    /// now.
    pub fn set_enabled(&self, enabled: bool) {
        self.lock().enabled = enabled;
        if enabled {
            self.wake.notify_one();
        }
    }

    /// Whether an automatic check should run now.
    fn due(&self) -> bool {
        let state = self.lock();
        state.enabled && !state.busy && state.due_ms.is_none_or(|due| (self.clock)() >= due)
    }

    /// "Check now": checks on a task of its own (a caller that goes away does not stop it).
    /// Nothing happens while a check, download or install runs.
    pub fn check_soon(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move { this.check().await });
    }

    /// Checks for a newer version and, when there is one this copy can install, downloads
    /// it. Nothing happens while a check, download or install runs, or without a backend.
    pub async fn check(self: &Arc<Self>) {
        let Some(backend) = self.backend.get().cloned() else {
            return;
        };
        {
            let mut state = self.lock();
            if state.busy {
                return;
            }
            state.busy = true;
            // A ready download stays shown while the source is asked again.
            if state.ready.is_none() {
                state.status = UpdateStatus::Checking;
            }
        }
        self.emit();
        let outcome = backend.check().await;
        let now = (self.clock)();
        let current = backend.current_version();
        let offer = match outcome {
            Err(failure) => {
                let mut state = self.lock();
                state.due_ms = Some(now + self.timing.retry.as_millis() as u64);
                state.busy = false;
                // A failed look again leaves a ready download ready.
                if let Some(version) = state.ready.clone() {
                    state.status = UpdateStatus::Ready { version };
                } else {
                    state.status = failed(Step::Check, None, &failure, now);
                }
                drop(state);
                self.emit();
                return;
            }
            Ok(offer) => offer.filter(|o| is_newer(&current, &o.version)),
        };
        let blocker = offer.as_ref().and_then(|_| backend.blocker());
        {
            let mut state = self.lock();
            state.due_ms = Some(now + self.timing.every.as_millis() as u64);
            match (&offer, blocker) {
                (None, _) => {
                    state.ready = None;
                    state.busy = false;
                    state.status = UpdateStatus::UpToDate { checked_at_ms: now };
                }
                (Some(offer), Some(blocker)) => {
                    state.busy = false;
                    state.status = UpdateStatus::Available {
                        version: offer.version.clone(),
                        reason: blocker.reason(),
                    };
                }
                (Some(offer), None) if state.ready.as_deref() == Some(offer.version.as_str()) => {
                    state.busy = false;
                    state.status = UpdateStatus::Ready {
                        version: offer.version.clone(),
                    };
                }
                (Some(offer), None) => {
                    state.status = UpdateStatus::Downloading {
                        version: offer.version.clone(),
                        received: 0,
                        total: None,
                    };
                }
            }
        }
        self.emit();
        let Some(offer) = offer else { return };
        if !matches!(self.status(), UpdateStatus::Downloading { .. }) {
            return;
        }
        let version = offer.version;
        let mut shown = Shown::default();
        let progress: Progress<'_> = Box::new(|received, total| {
            if shown.worth_showing(received, total) {
                self.set_status(UpdateStatus::Downloading {
                    version: version.clone(),
                    received,
                    total,
                });
            }
        });
        let downloaded = backend.download(&version, progress).await;
        let now = (self.clock)();
        {
            let mut state = self.lock();
            state.busy = false;
            match downloaded {
                Ok(()) => {
                    state.ready = Some(version.clone());
                    state.status = UpdateStatus::Ready { version };
                }
                Err(failure) => {
                    state.ready = None;
                    // Tried again with the next check, an hour on.
                    state.due_ms = Some(now + self.timing.retry.as_millis() as u64);
                    state.status = failed(Step::Download, Some(&version), &failure, now);
                }
            }
        }
        self.emit();
    }

    /// Puts the ready version in place and restarts gg (blocks: call off the async
    /// runtime). `BadRequest` when nothing is ready or something else runs; when the install
    /// fails, the status says so and the error is returned.
    pub fn install_and_restart(&self) -> Result<()> {
        let backend = self.backend.get().cloned();
        let version = {
            let mut state = self.lock();
            let version = match (&backend, &state.ready) {
                (Some(_), Some(version)) if !state.busy => version.clone(),
                _ => {
                    return Err(CoreError::new(
                        ErrorCode::BadRequest,
                        "No update is ready to install.",
                    ));
                }
            };
            state.busy = true;
            state.status = UpdateStatus::Installing {
                version: version.clone(),
            };
            version
        };
        self.emit();
        let Some(backend) = backend else {
            unreachable!("checked above")
        };
        let outcome = backend.install_and_restart();
        let mut state = self.lock();
        state.busy = false;
        match outcome {
            Ok(()) => Ok(()),
            Err(failure) => {
                // Downloaded again with the next check.
                state.ready = None;
                let now = (self.clock)();
                state.due_ms = Some(now + self.timing.retry.as_millis() as u64);
                state.status = failed(Step::Install, Some(&version), &failure, now);
                let message = failure_message(Step::Install, Some(&version), failure.kind);
                drop(state);
                self.emit();
                Err(CoreError::new(ErrorCode::Io, message).with_detail(failure.detail))
            }
        }
    }

    fn set_status(&self, status: UpdateStatus) {
        self.lock().status = status;
        self.emit();
    }

    fn emit(&self) {
        let status = self.status();
        if let Ok(payload) = serde_json::to_value(status) {
            self.sink.emit(EVENT, payload);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        lock_ignoring_poison(&self.state)
    }
}

fn failed(step: Step, version: Option<&str>, failure: &UpdateFailure, now: u64) -> UpdateStatus {
    UpdateStatus::Failed {
        step,
        version: version.map(str::to_owned),
        message: failure_message(step, version, failure.kind),
        detail: (!failure.detail.is_empty()).then(|| failure.detail.clone()),
        at_ms: now,
    }
}

/// Which download progress is worth an event: each whole percent, or each half megabyte
/// when the size is not known.
#[derive(Default)]
struct Shown {
    last: Option<u64>,
}

impl Shown {
    fn worth_showing(&mut self, received: u64, total: Option<u64>) -> bool {
        let step = match total {
            Some(total) if total > 0 => received.saturating_mul(100) / total,
            _ => received / (512 * 1024),
        };
        if self.last == Some(step) {
            return false;
        }
        self.last = Some(step);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::RecordingSink;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    #[test]
    fn only_a_strictly_newer_version_is_newer() {
        for (current, offered) in [
            ("0.4.0", "0.4.1"),
            ("0.4.0", "0.5.0"),
            ("0.4.0", "1.0.0"),
            ("0.4.0", "v0.4.1"),
            ("0.4.9", "0.4.10"),
            ("0.4.0-beta.1", "0.4.0"),
            ("0.4.0-beta.1", "0.4.0-beta.2"),
        ] {
            assert!(is_newer(current, offered), "{offered} over {current}");
        }
        for (current, offered) in [
            ("0.4.0", "0.4.0"),
            ("0.4.0", "v0.4.0"),
            ("0.4.1", "0.4.0"),
            ("1.0.0", "0.9.9"),
            ("0.4.10", "0.4.9"),
            // Build metadata is not a newer version.
            ("0.4.0", "0.4.0+1"),
            // A release build is never offered a pre-release.
            ("0.4.0", "0.5.0-rc.1"),
            ("0.4.0", "0.4.0-rc.1"),
            ("0.4.0-beta.2", "0.4.0-beta.1"),
            // Not versions.
            ("0.4.0", ""),
            ("0.4.0", "latest"),
            ("0.4.0", "0.5"),
            ("", "0.5.0"),
        ] {
            assert!(!is_newer(current, offered), "{offered} over {current}");
        }
    }

    #[test]
    fn the_bundle_of_a_mac_executable() {
        assert_eq!(
            mac_bundle_of(Path::new("/Applications/gg.app/Contents/MacOS/gg-desktop")),
            Some(PathBuf::from("/Applications/gg.app"))
        );
        assert_eq!(
            mac_bundle_of(Path::new("/tmp/x/My Apps/gg.app/Contents/MacOS/gg-desktop")),
            Some(PathBuf::from("/tmp/x/My Apps/gg.app"))
        );
        for exe in [
            "/repo/target/release/gg-desktop",
            "/Applications/gg.app/Contents/Resources/gg",
            "/x/gg/Contents/MacOS/gg-desktop",
            "gg-desktop",
        ] {
            assert_eq!(mac_bundle_of(Path::new(exe)), None, "{exe}");
        }
    }

    #[test]
    fn packages_and_development_builds_do_not_update_themselves() {
        assert_eq!(blocker(&Install::LinuxPackage), Some(Blocker::LinuxPackage));
        assert_eq!(blocker(&Install::Unbundled), Some(Blocker::NotInstalled));
    }

    #[test]
    fn a_translocated_app_does_not_update_itself() {
        let bundle =
            PathBuf::from("/private/var/folders/ab/xyz/T/AppTranslocation/1234-5678/d/gg.app");
        assert_eq!(
            blocker(&Install::MacBundle(bundle)),
            Some(Blocker::Translocated)
        );
    }

    #[test]
    fn an_app_in_a_writable_folder_updates_itself() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("gg.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        assert_eq!(blocker(&Install::MacBundle(bundle)), None);
        let appimage = dir.path().join("gg.AppImage");
        std::fs::write(&appimage, "x").unwrap();
        assert_eq!(blocker(&Install::AppImage(appimage)), None);
        assert_eq!(
            blocker(&Install::WindowsInstalled(dir.path().to_path_buf())),
            None
        );
    }

    #[test]
    fn a_missing_folder_is_not_writable() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone");
        assert_eq!(
            blocker(&Install::WindowsInstalled(gone.clone())),
            Some(Blocker::NotWritable(gone.clone()))
        );
        assert_eq!(
            blocker(&Install::AppImage(gone.join("gg.AppImage"))),
            Some(Blocker::NotWritable(gone))
        );
    }

    /// A folder (or bundle) the user may not change: the app stays as it is and says why.
    /// (Root may write anywhere, so this is checked only when the tests do not run as root.)
    #[cfg(unix)]
    #[test]
    fn an_app_in_a_read_only_folder_does_not_update_itself() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: getuid has no preconditions.
        if unsafe { libc::getuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("Applications");
        let bundle = apps.join("gg.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let mode =
            |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m));
        mode(&apps, 0o555).unwrap();
        let in_read_only = blocker(&Install::MacBundle(bundle.clone()));
        mode(&apps, 0o755).unwrap();
        mode(&bundle, 0o555).unwrap();
        let read_only_bundle = blocker(&Install::MacBundle(bundle.clone()));
        mode(&bundle, 0o755).unwrap();
        assert_eq!(in_read_only, Some(Blocker::NotWritable(apps.clone())));
        assert_eq!(read_only_bundle, Some(Blocker::NotWritable(apps.clone())));
        let image = std::fs::write(apps.join("gg.AppImage"), "x");
        image.unwrap();
        mode(&apps, 0o555).unwrap();
        let appimage = blocker(&Install::AppImage(apps.join("gg.AppImage")));
        mode(&apps, 0o755).unwrap();
        assert_eq!(appimage, Some(Blocker::NotWritable(apps)));
    }

    #[test]
    fn every_blocker_says_why_in_a_sentence() {
        for b in [
            Blocker::Translocated,
            Blocker::DiskImage,
            Blocker::NotWritable(PathBuf::from("/Applications")),
            Blocker::LinuxPackage,
            Blocker::NotInstalled,
        ] {
            let reason = b.reason();
            assert!(reason.ends_with('.'), "{reason}");
            // A capital, or a name that is written in lower case (gg, macOS).
            assert!(
                reason.chars().next().unwrap().is_uppercase()
                    || reason.starts_with("gg ")
                    || reason.starts_with("macOS "),
                "{reason}"
            );
        }
        assert_eq!(
            Blocker::NotWritable(PathBuf::from("/Applications")).reason(),
            "gg cannot update itself: it may not change /Applications."
        );
    }

    #[test]
    fn failures_say_what_failed_and_why() {
        assert_eq!(
            failure_message(Step::Check, None, FailureKind::Offline),
            "Could not check for updates. GitHub could not be reached."
        );
        assert_eq!(
            failure_message(Step::Check, None, FailureKind::NotFound),
            "Could not check for updates. GitHub has no update information for the latest release."
        );
        assert_eq!(
            failure_message(Step::Download, Some("0.4.1"), FailureKind::Signature),
            "Could not download gg 0.4.1. Its signature does not match gg’s, so nothing was installed."
        );
        assert_eq!(
            failure_message(Step::Install, Some("0.4.1"), FailureKind::Install),
            "Could not install gg 0.4.1. Its files could not be put in place."
        );
        assert_eq!(
            failure_message(Step::Download, Some("0.4.1"), FailureKind::NotFound),
            "Could not download gg 0.4.1. The download was not found."
        );
    }

    #[test]
    fn download_progress_is_shown_by_whole_percents() {
        let mut shown = Shown::default();
        let total = Some(1000);
        let seen: Vec<u64> = [0, 5, 9, 10, 15, 20, 999, 1000]
            .into_iter()
            .filter(|&r| shown.worth_showing(r, total))
            .collect();
        assert_eq!(seen, [0, 10, 20, 999, 1000]);
        let mut unknown = Shown::default();
        let half = 512 * 1024;
        let seen: Vec<u64> = [1, 2, half, half + 1, 2 * half]
            .into_iter()
            .filter(|&r| unknown.worth_showing(r, None))
            .collect();
        assert_eq!(seen, [1, half, 2 * half]);
    }

    /// A backend that answers as the test says, and counts what it was asked.
    struct Fake {
        current: String,
        check: Mutex<std::result::Result<Option<Offer>, UpdateFailure>>,
        download: Mutex<std::result::Result<(), UpdateFailure>>,
        install: Mutex<std::result::Result<(), UpdateFailure>>,
        blocker: Mutex<Option<Blocker>>,
        checks: AtomicUsize,
        downloads: Mutex<Vec<String>>,
        installs: AtomicUsize,
    }

    impl Fake {
        fn new(current: &str) -> Arc<Self> {
            Arc::new(Self {
                current: current.to_owned(),
                check: Mutex::new(Ok(None)),
                download: Mutex::new(Ok(())),
                install: Mutex::new(Ok(())),
                blocker: Mutex::new(None),
                checks: AtomicUsize::new(0),
                downloads: Mutex::new(Vec::new()),
                installs: AtomicUsize::new(0),
            })
        }

        fn offers(&self, version: &str) {
            *self.check.lock().unwrap() = Ok(Some(Offer {
                version: version.to_owned(),
            }));
        }
    }

    impl UpdateBackend for Fake {
        fn current_version(&self) -> String {
            self.current.clone()
        }
        fn blocker(&self) -> Option<Blocker> {
            self.blocker.lock().unwrap().clone()
        }
        fn check(&self) -> BoxFuture<'_, std::result::Result<Option<Offer>, UpdateFailure>> {
            self.checks.fetch_add(1, Ordering::SeqCst);
            let answer = self.check.lock().unwrap().clone();
            Box::pin(async move { answer })
        }
        fn download<'a>(
            &'a self,
            version: &'a str,
            mut progress: Progress<'a>,
        ) -> BoxFuture<'a, std::result::Result<(), UpdateFailure>> {
            self.downloads.lock().unwrap().push(version.to_owned());
            let answer = self.download.lock().unwrap().clone();
            Box::pin(async move {
                progress(0, Some(100));
                progress(50, Some(100));
                progress(100, Some(100));
                answer
            })
        }
        fn install_and_restart(&self) -> std::result::Result<(), UpdateFailure> {
            self.installs.fetch_add(1, Ordering::SeqCst);
            self.install.lock().unwrap().clone()
        }
    }

    fn updates(
        fake: &Arc<Fake>,
        enabled: bool,
    ) -> (Arc<Updates>, Arc<RecordingSink>, Arc<AtomicU64>) {
        let sink = Arc::new(RecordingSink::default());
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock = {
            let now = now.clone();
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let updates = Updates::with(sink.clone(), enabled, Timing::default(), clock);
        updates.set_backend(fake.clone());
        (updates, sink, now)
    }

    fn statuses(sink: &RecordingSink) -> Vec<String> {
        sink.events()
            .into_iter()
            .filter(|(n, _)| n == EVENT)
            .map(|(_, v)| v["state"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn without_a_backend_updating_is_unavailable() {
        let sink = Arc::new(RecordingSink::default());
        let updates = Updates::new(sink.clone(), true);
        assert_eq!(updates.status(), UpdateStatus::Unavailable);
        updates.check().await;
        updates.start();
        assert_eq!(updates.status(), UpdateStatus::Unavailable);
        assert!(updates.install_and_restart().is_err());
        assert!(sink.events().is_empty());
    }

    #[tokio::test]
    async fn the_same_or_an_older_version_is_never_offered() {
        let fake = Fake::new("0.4.0");
        let (updates, _, _) = updates(&fake, true);
        assert_eq!(updates.status(), UpdateStatus::Idle);
        for offered in ["0.4.0", "0.3.9", "0.4.0+build", "0.5.0-rc.1", "nonsense"] {
            fake.offers(offered);
            updates.check().await;
            assert_eq!(
                updates.status(),
                UpdateStatus::UpToDate {
                    checked_at_ms: 1_000_000
                },
                "{offered}"
            );
        }
        assert!(fake.downloads.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_newer_version_is_downloaded_then_ready_then_installed() {
        let fake = Fake::new("0.4.0");
        let (updates, sink, _) = updates(&fake, true);
        fake.offers("0.4.1");
        updates.check().await;
        assert_eq!(
            updates.status(),
            UpdateStatus::Ready {
                version: "0.4.1".into()
            }
        );
        assert_eq!(*fake.downloads.lock().unwrap(), ["0.4.1"]);
        assert_eq!(
            statuses(&sink),
            [
                "idle",
                "checking",
                "downloading",
                "downloading",
                "downloading",
                "downloading",
                "ready"
            ]
        );
        let progress: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|(_, v)| v["state"] == "downloading")
            .map(|(_, v)| (v["received"].as_u64(), v["total"].as_u64()))
            .collect();
        assert_eq!(
            progress,
            [
                (Some(0), None),
                (Some(0), Some(100)),
                (Some(50), Some(100)),
                (Some(100), Some(100))
            ]
        );
        // Asked again: still ready, not downloaded twice.
        updates.check().await;
        assert_eq!(fake.downloads.lock().unwrap().len(), 1);
        assert!(matches!(updates.status(), UpdateStatus::Ready { .. }));
        updates.install_and_restart().unwrap();
        assert_eq!(fake.installs.load(Ordering::SeqCst), 1);
        assert_eq!(
            updates.status(),
            UpdateStatus::Installing {
                version: "0.4.1".into()
            }
        );
    }

    #[tokio::test]
    async fn a_newer_version_than_the_ready_one_is_downloaded() {
        let fake = Fake::new("0.4.0");
        let (updates, _, _) = updates(&fake, true);
        fake.offers("0.4.1");
        updates.check().await;
        fake.offers("0.4.2");
        updates.check().await;
        assert_eq!(*fake.downloads.lock().unwrap(), ["0.4.1", "0.4.2"]);
        assert_eq!(
            updates.status(),
            UpdateStatus::Ready {
                version: "0.4.2".into()
            }
        );
    }

    #[tokio::test]
    async fn a_failed_check_is_quiet_and_retried_an_hour_on() {
        let fake = Fake::new("0.4.0");
        let (updates, sink, now) = updates(&fake, true);
        *fake.check.lock().unwrap() = Err(UpdateFailure::new(
            FailureKind::Offline,
            "error sending request",
        ));
        assert!(updates.due());
        updates.check().await;
        match updates.status() {
            UpdateStatus::Failed {
                step,
                version,
                message,
                detail,
                at_ms,
            } => {
                assert_eq!(step, Step::Check);
                assert_eq!(version, None);
                assert_eq!(
                    message,
                    "Could not check for updates. GitHub could not be reached."
                );
                assert_eq!(detail.as_deref(), Some("error sending request"));
                assert_eq!(at_ms, 1_000_000);
            }
            other => panic!("{other:?}"),
        }
        // Only the status event: nothing else for the user.
        assert!(sink.events().iter().all(|(n, _)| n == EVENT));
        assert!(!updates.due());
        now.fetch_add(3_599_999, Ordering::SeqCst);
        assert!(!updates.due());
        now.fetch_add(1, Ordering::SeqCst);
        assert!(updates.due());
    }

    #[tokio::test]
    async fn after_a_good_check_the_next_is_due_a_day_on() {
        let fake = Fake::new("0.4.0");
        let (updates, _, now) = updates(&fake, true);
        updates.check().await;
        assert!(!updates.due());
        now.fetch_add(24 * 3600 * 1000 - 1, Ordering::SeqCst);
        assert!(!updates.due());
        now.fetch_add(1, Ordering::SeqCst);
        assert!(updates.due());
        updates.set_enabled(false);
        assert!(!updates.due(), "no automatic check while they are off");
        updates.set_enabled(true);
        assert!(updates.due());
    }

    #[tokio::test]
    async fn a_failed_check_leaves_a_ready_download_ready() {
        let fake = Fake::new("0.4.0");
        let (updates, _, _) = updates(&fake, true);
        fake.offers("0.4.1");
        updates.check().await;
        *fake.check.lock().unwrap() = Err(UpdateFailure::new(FailureKind::Offline, "x"));
        updates.check().await;
        assert_eq!(
            updates.status(),
            UpdateStatus::Ready {
                version: "0.4.1".into()
            }
        );
        updates.install_and_restart().unwrap();
    }

    #[tokio::test]
    async fn a_bad_signature_installs_nothing() {
        let fake = Fake::new("0.4.0");
        let (updates, _, _) = updates(&fake, true);
        fake.offers("0.4.1");
        *fake.download.lock().unwrap() = Err(UpdateFailure::new(
            FailureKind::Signature,
            "the signature does not verify",
        ));
        updates.check().await;
        match updates.status() {
            UpdateStatus::Failed { step, message, .. } => {
                assert_eq!(step, Step::Download);
                assert!(
                    message.starts_with("Could not download gg 0.4.1."),
                    "{message}"
                );
            }
            other => panic!("{other:?}"),
        }
        let refused = updates.install_and_restart().unwrap_err();
        assert_eq!(refused.code, ErrorCode::BadRequest);
        assert_eq!(fake.installs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_copy_that_cannot_update_itself_only_says_a_version_is_available() {
        let fake = Fake::new("0.4.0");
        let (updates, _, _) = updates(&fake, true);
        *fake.blocker.lock().unwrap() = Some(Blocker::LinuxPackage);
        updates.check().await;
        assert!(matches!(updates.status(), UpdateStatus::UpToDate { .. }));
        fake.offers("0.4.1");
        updates.check().await;
        assert_eq!(
            updates.status(),
            UpdateStatus::Available {
                version: "0.4.1".into(),
                reason: Blocker::LinuxPackage.reason(),
            }
        );
        assert!(fake.downloads.lock().unwrap().is_empty());
        assert!(updates.install_and_restart().is_err());
        assert_eq!(fake.installs.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_failed_install_says_so_and_downloads_again_later() {
        let fake = Fake::new("0.4.0");
        let (updates, _, _) = updates(&fake, true);
        fake.offers("0.4.1");
        updates.check().await;
        *fake.install.lock().unwrap() = Err(UpdateFailure::new(
            FailureKind::Install,
            "Permission denied",
        ));
        let err = updates.install_and_restart().unwrap_err();
        assert_eq!(err.code, ErrorCode::Io);
        assert_eq!(
            err.message,
            "Could not install gg 0.4.1. Its files could not be put in place."
        );
        assert!(matches!(
            updates.status(),
            UpdateStatus::Failed {
                step: Step::Install,
                ..
            }
        ));
        updates.check().await;
        assert_eq!(fake.downloads.lock().unwrap().len(), 2);
    }

    #[test]
    fn statuses_are_sent_as_the_ui_reads_them() {
        let json = |s: UpdateStatus| serde_json::to_value(s).unwrap();
        assert_eq!(
            json(UpdateStatus::UpToDate { checked_at_ms: 5 }),
            serde_json::json!({"state": "upToDate", "checkedAtMs": 5})
        );
        assert_eq!(
            json(UpdateStatus::Downloading {
                version: "1.0.0".into(),
                received: 1,
                total: None
            }),
            serde_json::json!({"state": "downloading", "version": "1.0.0", "received": 1, "total": null})
        );
        assert_eq!(
            json(UpdateStatus::Failed {
                step: Step::Download,
                version: Some("1.0.0".into()),
                message: "m".into(),
                detail: None,
                at_ms: 2
            }),
            serde_json::json!({"state": "failed", "step": "download", "version": "1.0.0", "message": "m", "detail": null, "atMs": 2})
        );
    }

    /// The schedule: nothing before the first delay, then a check; the next only once due.
    #[tokio::test(start_paused = true)]
    async fn checks_run_after_the_first_delay_and_when_due() {
        let fake = Fake::new("0.4.0");
        let (updates, _, now) = updates(&fake, true);
        updates.start();
        updates.start();
        tokio::time::sleep(Duration::from_millis(2_900)).await;
        assert_eq!(fake.checks.load(Ordering::SeqCst), 0);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(fake.checks.load(Ordering::SeqCst), 1);
        // Hours pass by the timer, but by the wall clock it is not yet due.
        tokio::time::sleep(Duration::from_secs(5 * 3600)).await;
        assert_eq!(fake.checks.load(Ordering::SeqCst), 1);
        // A day later by the wall clock (the machine slept): the next tick checks.
        now.fetch_add(24 * 3600 * 1000, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(10 * 60)).await;
        assert_eq!(fake.checks.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn with_checks_off_none_runs_until_they_are_turned_on() {
        let fake = Fake::new("0.4.0");
        let (updates, _, _) = updates(&fake, false);
        updates.start();
        tokio::time::sleep(Duration::from_secs(3 * 24 * 3600)).await;
        assert_eq!(fake.checks.load(Ordering::SeqCst), 0);
        assert_eq!(updates.status(), UpdateStatus::Idle);
        updates.set_enabled(true);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(fake.checks.load(Ordering::SeqCst), 1);
    }
}
