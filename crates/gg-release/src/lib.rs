//! What a release attaches for gg to update itself, and the checks made before the draft
//! release is created (`.github/workflows/release.yml`):
//!
//! - each platform's update file (the macOS universal `.app.tar.gz`, the Linux AppImage, the
//!   Windows installer) with its signature, made by `tauri signer sign --app-version`;
//! - `latest.json`, the updater plugin's static manifest: the version, the date, and for each
//!   platform key the plugin looks up (`darwin-aarch64` and `darwin-x86_64` both name the
//!   universal archive) the download URL of that tag's asset and the signature's text.
//!
//! The checks: every signature verifies against the public key in the app's config with the
//! verifier the plugin itself uses, and was made for the release's version; the manifest
//! names that version and only URLs of that tag's assets that are being uploaded.

use base64::Engine as _;
use minisign_verify::{PublicKey, Signature};
use serde_json::{Value, json};

/// Where a tag's assets are downloaded from: `<this>/<tag>/<file>`.
pub const DOWNLOADS: &str = "https://github.com/KhanShaheb34/gg/releases/download";

/// The update file for each platform key the updater looks up (`{os}-{arch}`).
pub const PLATFORMS: [(&str, &str); 4] = [
    ("darwin-aarch64", "gg-macos-universal.app.tar.gz"),
    ("darwin-x86_64", "gg-macos-universal.app.tar.gz"),
    ("linux-x86_64", "gg-linux-x86_64.AppImage"),
    ("windows-x86_64", "gg-windows-x64-setup.exe"),
];

/// The update files, each once.
pub fn update_files() -> Vec<&'static str> {
    let mut files: Vec<&str> = PLATFORMS.iter().map(|(_, f)| *f).collect();
    files.dedup();
    files
}

fn unbase64(what: &str, text: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|e| format!("{what} is not base64: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{what} is not text"))
}

/// The updater's public key in the app's config (`plugins.updater.pubkey` of
/// tauri.conf.json's text).
pub fn pubkey_from_config(config: &str) -> Result<String, String> {
    let config: Value =
        serde_json::from_str(config).map_err(|e| format!("the config is not JSON: {e}"))?;
    config["plugins"]["updater"]["pubkey"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "the config has no plugins.updater.pubkey".to_owned())
}

/// Checks that `sig` (a `.sig` file's text: base64 of a minisign signature) is a signature
/// of `data` by `pubkey` (base64 of a minisign public key, as the config has it), as the
/// updater checks it, and that it was made for `version` (its trusted comment says so).
pub fn verify(pubkey: &str, data: &[u8], sig: &str, version: &str) -> Result<(), String> {
    let key = PublicKey::decode(&unbase64("the public key", pubkey)?)
        .map_err(|e| format!("the public key cannot be read: {e}"))?;
    let signature = Signature::decode(&unbase64("the signature", sig)?)
        .map_err(|e| format!("the signature cannot be read: {e}"))?;
    key.verify(data, &signature, true)
        .map_err(|e| format!("the signature does not verify with the app's key: {e}"))?;
    let signed = signature
        .trusted_comment()
        .split('\t')
        .find_map(|field| field.strip_prefix("version:"));
    match signed {
        Some(v) if v.trim_start_matches('v') == version.trim_start_matches('v') => Ok(()),
        Some(v) => Err(format!("it was signed for version {v}, not {version}")),
        None => Err("it was signed without a version (sign with --app-version)".to_owned()),
    }
}

/// Whether `s` is an RFC 3339 date and time (the plugin refuses a manifest whose `pub_date`
/// is not): `2026-11-21T10:00:00Z`, with optional fractions and an offset instead of `Z`.
pub fn is_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    let digits =
        |r: std::ops::Range<usize>| r.clone().all(|i| b.get(i).is_some_and(u8::is_ascii_digit));
    if b.len() < 20
        || !digits(0..4)
        || b[4] != b'-'
        || !digits(5..7)
        || b[7] != b'-'
        || !digits(8..10)
        || !(b[10] == b'T' || b[10] == b't')
        || !digits(11..13)
        || b[13] != b':'
        || !digits(14..16)
        || b[16] != b':'
        || !digits(17..19)
    {
        return false;
    }
    let mut rest = &s[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let n = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return false;
        }
        rest = &fraction[n..];
    }
    let zone = rest.as_bytes();
    rest == "Z"
        || rest == "z"
        || (zone.len() == 6
            && (zone[0] == b'+' || zone[0] == b'-')
            && zone[1..3].iter().all(u8::is_ascii_digit)
            && zone[3] == b':'
            && zone[4..6].iter().all(u8::is_ascii_digit))
}

/// The manifest for `version`, released as `tag` on `pub_date`, with `notes`; `sig(file)` is
/// the text of `file`'s `.sig`.
pub fn manifest(
    version: &str,
    tag: &str,
    notes: &str,
    pub_date: &str,
    sig: impl Fn(&str) -> Result<String, String>,
) -> Result<Value, String> {
    if tag != format!("v{version}") {
        return Err(format!("the tag {tag} is not v{version}"));
    }
    if !is_rfc3339(pub_date) {
        return Err(format!("{pub_date} is not an RFC 3339 date"));
    }
    let mut platforms = serde_json::Map::new();
    for (platform, file) in PLATFORMS {
        platforms.insert(
            platform.to_owned(),
            json!({
                "signature": sig(file)?.trim(),
                "url": format!("{DOWNLOADS}/{tag}/{file}"),
            }),
        );
    }
    Ok(json!({
        "version": version,
        "notes": notes,
        "pub_date": pub_date,
        "platforms": platforms,
    }))
}

/// What is wrong with `manifest` for `version`, released as `tag`, whose assets being
/// uploaded are `uploaded` (file names); `read(file)` gives an asset's bytes, `pubkey` is the
/// app's. Empty when nothing is.
pub fn check_manifest(
    manifest: &Value,
    version: &str,
    tag: &str,
    pubkey: &str,
    uploaded: &[String],
    read: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Vec<String> {
    let mut problems = Vec::new();
    if tag != format!("v{version}") {
        problems.push(format!("the tag {tag} is not v{version}"));
    }
    if manifest["version"].as_str() != Some(version) {
        problems.push(format!(
            "it names version {}, not {version}",
            manifest["version"]
        ));
    }
    match manifest["pub_date"].as_str() {
        Some(date) if is_rfc3339(date) => {}
        other => problems.push(format!("its pub_date {other:?} is not an RFC 3339 date")),
    }
    let Some(platforms) = manifest["platforms"].as_object() else {
        problems.push("it has no platforms".to_owned());
        return problems;
    };
    let expected: Vec<&str> = PLATFORMS.iter().map(|(p, _)| *p).collect();
    let mut named: Vec<&str> = platforms.keys().map(String::as_str).collect();
    named.sort_unstable();
    let mut wanted = expected.clone();
    wanted.sort_unstable();
    if named != wanted {
        problems.push(format!("its platforms are {named:?}, not {wanted:?}"));
    }
    let prefix = format!("{DOWNLOADS}/{tag}/");
    for (platform, entry) in platforms {
        let Some(url) = entry["url"].as_str() else {
            problems.push(format!("{platform} has no url"));
            continue;
        };
        let Some(file) = url.strip_prefix(&prefix) else {
            problems.push(format!(
                "{platform}'s url {url} is not one of {tag}'s assets"
            ));
            continue;
        };
        if file.is_empty() || file.contains('/') || !uploaded.iter().any(|u| u == file) {
            problems.push(format!(
                "{platform}'s url {url} names {file}, which is not being uploaded"
            ));
            continue;
        }
        if let Some((_, want)) = PLATFORMS.iter().find(|(p, _)| p == platform)
            && *want != file
        {
            problems.push(format!("{platform} names {file}, not {want}"));
        }
        let Some(sig) = entry["signature"].as_str() else {
            problems.push(format!("{platform} has no signature"));
            continue;
        };
        match read(file) {
            Ok(data) => {
                if let Err(e) = verify(pubkey, &data, sig, version) {
                    problems.push(format!("{platform} ({file}): {e}"));
                }
            }
            Err(e) => problems.push(format!("{platform}: {e}")),
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = include_str!("../tests/fixtures/test-key.pub");
    const MAC: &[u8] = include_bytes!("../tests/fixtures/gg-macos-universal.app.tar.gz");
    const MAC_SIG: &str = include_str!("../tests/fixtures/gg-macos-universal.app.tar.gz.sig");
    const UNVERSIONED: &[u8] = include_bytes!("../tests/fixtures/unversioned.bin");
    const UNVERSIONED_SIG: &str = include_str!("../tests/fixtures/unversioned.bin.sig");

    fn fixture(file: &str) -> Result<Vec<u8>, String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(file);
        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn sig_of(file: &str) -> Result<String, String> {
        String::from_utf8(fixture(&format!("{file}.sig"))?).map_err(|e| e.to_string())
    }

    fn uploaded() -> Vec<String> {
        update_files().iter().map(|f| (*f).to_owned()).collect()
    }

    #[test]
    fn a_signature_verifies_for_its_file_and_version_only() {
        assert_eq!(verify(KEY, MAC, MAC_SIG, "1.2.3"), Ok(()));
        assert_eq!(verify(KEY, MAC, MAC_SIG, "v1.2.3"), Ok(()));
        let tampered = [MAC, b"x"].concat();
        assert!(
            verify(KEY, &tampered, MAC_SIG, "1.2.3")
                .unwrap_err()
                .contains("does not verify")
        );
        assert_eq!(
            verify(KEY, MAC, MAC_SIG, "1.2.4"),
            Err("it was signed for version 1.2.3, not 1.2.4".to_owned())
        );
        assert_eq!(
            verify(KEY, UNVERSIONED, UNVERSIONED_SIG, "1.2.3"),
            Err("it was signed without a version (sign with --app-version)".to_owned())
        );
    }

    #[test]
    fn a_signature_by_another_key_does_not_verify() {
        let app_key =
            pubkey_from_config(include_str!("../../../src-tauri/tauri.conf.json")).unwrap();
        assert!(
            verify(&app_key, MAC, MAC_SIG, "1.2.3")
                .unwrap_err()
                .contains("does not verify")
        );
        assert!(verify("not base64!", MAC, MAC_SIG, "1.2.3").is_err());
        assert!(verify(KEY, MAC, "bm90IGEgc2lnbmF0dXJl", "1.2.3").is_err());
    }

    #[test]
    fn the_key_is_read_from_the_app_config() {
        let key = pubkey_from_config(include_str!("../../../src-tauri/tauri.conf.json")).unwrap();
        assert!(
            unbase64("k", &key)
                .unwrap()
                .starts_with("untrusted comment: minisign public key")
        );
        assert!(pubkey_from_config("{}").is_err());
        assert!(pubkey_from_config("nope").is_err());
    }

    #[test]
    fn dates_are_rfc_3339() {
        for ok in [
            "2026-11-21T10:00:00Z",
            "2026-11-21T10:00:00.123Z",
            "2026-11-21T10:00:00+06:00",
            "2026-11-21t10:00:00-01:30",
        ] {
            assert!(is_rfc3339(ok), "{ok}");
        }
        for bad in [
            "",
            "2026-11-21",
            "2026-11-21 10:00:00Z",
            "2026-11-21T10:00:00",
            "2026-11-21T10:00:00.Z",
            "2026-11-21T10:00:00+0600",
            "21 Nov 2026",
        ] {
            assert!(!is_rfc3339(bad), "{bad}");
        }
    }

    #[test]
    fn the_manifest_names_each_platform_s_asset_of_the_tag() {
        let m = manifest(
            "1.2.3",
            "v1.2.3",
            "gg 1.2.3",
            "2026-11-21T10:00:00Z",
            sig_of,
        )
        .unwrap();
        assert_eq!(m["version"], "1.2.3");
        assert_eq!(m["notes"], "gg 1.2.3");
        assert_eq!(m["pub_date"], "2026-11-21T10:00:00Z");
        let mac = format!("{DOWNLOADS}/v1.2.3/gg-macos-universal.app.tar.gz");
        assert_eq!(m["platforms"]["darwin-aarch64"]["url"], mac);
        assert_eq!(m["platforms"]["darwin-x86_64"]["url"], mac);
        assert_eq!(m["platforms"]["darwin-x86_64"]["signature"], MAC_SIG.trim());
        assert_eq!(
            m["platforms"]["linux-x86_64"]["url"],
            format!("{DOWNLOADS}/v1.2.3/gg-linux-x86_64.AppImage")
        );
        assert_eq!(
            m["platforms"]["windows-x86_64"]["url"],
            format!("{DOWNLOADS}/v1.2.3/gg-windows-x64-setup.exe")
        );
        assert_eq!(m["platforms"].as_object().unwrap().len(), 4);
        assert!(check_manifest(&m, "1.2.3", "v1.2.3", KEY, &uploaded(), fixture).is_empty());
        // A tag that is not the version's, a bad date, a missing signature: refused.
        assert!(manifest("1.2.3", "v1.2.4", "", "2026-11-21T10:00:00Z", sig_of).is_err());
        assert!(manifest("1.2.3", "v1.2.3", "", "today", sig_of).is_err());
        assert!(
            manifest("1.2.3", "v1.2.3", "", "2026-11-21T10:00:00Z", |_| Err(
                "no .sig".into()
            ))
            .is_err()
        );
    }

    #[test]
    fn a_manifest_that_names_another_version_tag_or_file_is_refused() {
        let good = manifest("1.2.3", "v1.2.3", "", "2026-11-21T10:00:00Z", sig_of).unwrap();
        let check = |m: &Value, uploaded: &[String]| {
            check_manifest(m, "1.2.3", "v1.2.3", KEY, uploaded, fixture)
        };
        let mut other_version = good.clone();
        other_version["version"] = json!("1.2.4");
        assert_eq!(
            check(&other_version, &uploaded()),
            ["it names version \"1.2.4\", not 1.2.3"]
        );
        let mut other_tag = good.clone();
        other_tag["platforms"]["linux-x86_64"]["url"] =
            json!(format!("{DOWNLOADS}/v1.2.2/gg-linux-x86_64.AppImage"));
        assert!(check(&other_tag, &uploaded())[0].contains("is not one of v1.2.3's assets"));
        let mut elsewhere = good.clone();
        elsewhere["platforms"]["linux-x86_64"]["url"] =
            json!("https://example.com/gg-linux-x86_64.AppImage");
        assert_eq!(check(&elsewhere, &uploaded()).len(), 1);
        // An asset that is not uploaded.
        let without_linux: Vec<String> = uploaded()
            .into_iter()
            .filter(|f| !f.ends_with("AppImage"))
            .collect();
        assert!(check(&good, &without_linux)[0].contains("which is not being uploaded"));
        // A signature of another file.
        let mut swapped = good.clone();
        swapped["platforms"]["linux-x86_64"]["signature"] = json!(MAC_SIG.trim());
        assert!(check(&swapped, &uploaded())[0].contains("does not verify"));
        // A missing or extra platform.
        let mut missing = good.clone();
        missing["platforms"]
            .as_object_mut()
            .unwrap()
            .remove("darwin-x86_64");
        assert!(check(&missing, &uploaded())[0].contains("its platforms are"));
        let mut bad_date = good.clone();
        bad_date["pub_date"] = json!("yesterday");
        assert_eq!(check(&bad_date, &uploaded()).len(), 1);
        // Another key.
        let app_key =
            pubkey_from_config(include_str!("../../../src-tauri/tauri.conf.json")).unwrap();
        assert_eq!(
            check_manifest(&good, "1.2.3", "v1.2.3", &app_key, &uploaded(), fixture).len(),
            4
        );
    }
}
