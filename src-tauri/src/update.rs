//! gg's updater: Tauri's updater plugin behind gg-core's [`UpdateBackend`]. gg-core decides
//! when to check and what the user is told; this asks the one update source the app's config
//! names (`plugins.updater` in tauri.conf.json: GitHub's latest release's `latest.json`, over
//! HTTPS), downloads the offered version, has the plugin verify its signature against the
//! public key in that config, and on Restart puts it in place and restarts.
//!
//! Nothing here can point the updater elsewhere: the endpoint and key are the config's,
//! compiled into the app. (Performance builds can be given a local test source; see
//! `update_hook.rs`, compiled only with the `perf` feature.)

use std::sync::Mutex;
use std::time::Duration;

use gg_core::update::{
    BoxFuture, FailureKind, Install, Offer, Progress, UpdateBackend, UpdateFailure, is_newer,
};
use tauri::AppHandle;
#[cfg(target_os = "linux")]
use tauri::Manager;
use tauri_plugin_updater::{Error, Update, UpdaterExt};

/// How long the check may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the download may take, all of it.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub struct TauriUpdates {
    app: AppHandle,
    /// The last check's offer.
    offered: Mutex<Option<Update>>,
    /// The downloaded, verified version: its update and bytes.
    downloaded: Mutex<Option<(Update, Vec<u8>)>>,
}

impl TauriUpdates {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            offered: Mutex::new(None),
            downloaded: Mutex::new(None),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl UpdateBackend for TauriUpdates {
    fn current_version(&self) -> String {
        self.app.package_info().version.to_string()
    }

    fn blocker(&self) -> Option<gg_core::update::Blocker> {
        gg_core::update::blocker(&install(&self.app))
    }

    fn check(&self) -> BoxFuture<'_, Result<Option<Offer>, UpdateFailure>> {
        Box::pin(async move {
            let updater = self
                .app
                .updater_builder()
                // Strictly newer only, by gg-core's rule (never a downgrade or a pre-release).
                .version_comparator(|current, release| {
                    is_newer(&current.to_string(), &release.version.to_string())
                })
                .timeout(CHECK_TIMEOUT)
                .build()
                .map_err(failure)?;
            let update = updater.check().await.map_err(failure)?;
            let offer = update.as_ref().map(|u| Offer {
                version: u.version.clone(),
            });
            *lock(&self.offered) = update;
            Ok(offer)
        })
    }

    fn download<'a>(
        &'a self,
        version: &'a str,
        mut progress: Progress<'a>,
    ) -> BoxFuture<'a, Result<(), UpdateFailure>> {
        Box::pin(async move {
            let Some(mut update) = lock(&self.offered).clone().filter(|u| u.version == version)
            else {
                return Err(UpdateFailure::new(
                    FailureKind::Other,
                    format!("{version} was not offered"),
                ));
            };
            update.timeout = Some(DOWNLOAD_TIMEOUT);
            let mut received = 0u64;
            // Verifies the signature (and the version it was signed for) before it returns.
            let bytes = update
                .download(
                    |chunk, total| {
                        received += chunk as u64;
                        progress(received, total);
                    },
                    || {},
                )
                .await
                .map_err(failure)?;
            *lock(&self.downloaded) = Some((update, bytes));
            Ok(())
        })
    }

    fn install_and_restart(&self) -> Result<(), UpdateFailure> {
        let Some((update, bytes)) = lock(&self.downloaded).take() else {
            return Err(UpdateFailure::new(
                FailureKind::Other,
                "nothing was downloaded",
            ));
        };
        // Verifies the bytes again, then puts them in place (on Windows the installer takes
        // over: the app exits here and the installer starts it again).
        update.install(&bytes).map_err(failure)?;
        self.app.request_restart();
        Ok(())
    }
}

/// How the running copy was installed.
fn install(app: &AppHandle) -> Install {
    let Ok(exe) = tauri::utils::platform::current_exe() else {
        return Install::Unbundled;
    };
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        match gg_core::update::mac_bundle_of(&exe) {
            Some(bundle) => Install::MacBundle(bundle),
            None => Install::Unbundled,
        }
    }
    #[cfg(target_os = "linux")]
    {
        use tauri::utils::config::BundleType;
        let _ = exe;
        match tauri::utils::platform::bundle_type() {
            Some(BundleType::Deb | BundleType::Rpm) => Install::LinuxPackage,
            Some(BundleType::AppImage) => app
                .env()
                .appimage
                .map_or(Install::Unbundled, |p| Install::AppImage(p.into())),
            _ => Install::Unbundled,
        }
    }
    #[cfg(windows)]
    {
        use tauri::utils::config::BundleType;
        let _ = app;
        match (tauri::utils::platform::bundle_type(), exe.parent()) {
            (Some(BundleType::Nsis | BundleType::Msi), Some(dir)) => {
                Install::WindowsInstalled(dir.to_path_buf())
            }
            _ => Install::Unbundled,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (app, exe);
        Install::Unbundled
    }
}

/// The plugin's error, by what the user can make of it.
fn failure(error: Error) -> UpdateFailure {
    UpdateFailure::new(kind(&error), error.to_string())
}

fn kind(error: &Error) -> FailureKind {
    match error {
        Error::Reqwest(e) if e.is_decode() => FailureKind::BadManifest,
        Error::Reqwest(e) if e.is_status() => FailureKind::NotFound,
        Error::Reqwest(_) => FailureKind::Offline,
        // A download that did not answer 2xx; no release that answered with a manifest.
        Error::Network(_) | Error::ReleaseNotFound => FailureKind::NotFound,
        Error::Serialization(_) | Error::Semver(_) | Error::UrlParse(_) => FailureKind::BadManifest,
        Error::TargetNotFound(_) | Error::TargetsNotFound(_) => FailureKind::NoDownload,
        Error::Minisign(_)
        | Error::Base64(_)
        | Error::SignatureUtf8(_)
        | Error::SignedVersionMismatch { .. }
        | Error::MissingSignedVersion => FailureKind::Signature,
        Error::Io(_)
        | Error::TempDirNotOnSameMountPoint
        | Error::PreviousAppNotRestored(_)
        | Error::BinaryNotFoundInArchive
        | Error::TempDirNotFound
        | Error::AuthenticationFailed
        | Error::DebInstallFailed
        | Error::PackageInstallFailed
        | Error::InvalidUpdaterFormat => FailureKind::Install,
        _ => FailureKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_errors_are_told_by_what_they_mean() {
        assert_eq!(kind(&Error::ReleaseNotFound), FailureKind::NotFound);
        assert_eq!(
            kind(&Error::Network(
                "Download request failed with status: 404".into()
            )),
            FailureKind::NotFound
        );
        assert_eq!(
            kind(&Error::TargetsNotFound(vec!["linux-x86_64".into()])),
            FailureKind::NoDownload
        );
        assert_eq!(
            kind(&Error::SignedVersionMismatch {
                signed: "0.4.0".into(),
                announced: "0.4.1".into()
            }),
            FailureKind::Signature
        );
        assert_eq!(kind(&Error::MissingSignedVersion), FailureKind::Signature);
        assert_eq!(
            kind(&Error::SignatureUtf8("x".into())),
            FailureKind::Signature
        );
        assert_eq!(
            kind(&Error::Io(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            ))),
            FailureKind::Install
        );
        assert_eq!(kind(&Error::InvalidUpdaterFormat), FailureKind::Install);
        assert_eq!(
            kind(&Error::Serialization(
                serde_json::from_str::<u8>("x").unwrap_err()
            )),
            FailureKind::BadManifest
        );
        assert_eq!(kind(&Error::EmptyEndpoints), FailureKind::Other);
        // A signature that cannot be read.
        let Err(e) = minisign_verify::Signature::decode("not a signature") else {
            panic!("decoded");
        };
        assert_eq!(kind(&Error::Minisign(e)), FailureKind::Signature);
    }
}
