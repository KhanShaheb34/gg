//! What a release attaches for gg to update itself, and the checks made before the draft
//! release is created (`.github/workflows/release.yml`):
//!
//! - each platform's update file (the macOS universal `.app.tar.gz`, the Linux AppImage, the
//!   Windows installer) with its signature, made by `tauri signer sign --app-version`;
//! - `latest.json`, the updater plugin's static manifest: the version, the date, and for each
//!   platform key the plugin looks up (`darwin-aarch64` and `darwin-x86_64` both name the
//!   universal archive) the download URL of that tag's asset and the signature's text.
//!
//! The signatures are made here too ([`sign`], `gg-release sign`), in a job of the release
//! workflow that builds nothing else: from the secret key as `tauri signer generate` writes it
//! (base64 of minisign's password-protected secret key) and its password, a signature in the
//! format `tauri signer sign --app-version` writes, which the plugin verifies.
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

/// The untrusted comment Tauri's CLI writes in a signature (not covered by it).
const UNTRUSTED_COMMENT: &str = "signature from tauri secret key";

/// Signs `data`, the file named `file_name`, for `version`, with `secret_key` (the text
/// `tauri signer generate` writes: base64 of minisign's password-protected secret key) and its
/// `password`, as `tauri signer sign --app-version <version>` does at unix time `timestamp`:
/// returns the `.sig` file's text, base64 of a minisign signature (prehashed) whose trusted
/// comment is `timestamp:<t>\tfile:<name>\tversion:<version>`.
///
/// Errors never quote the key or the password.
pub fn sign(
    secret_key: &str,
    password: &str,
    data: &[u8],
    file_name: &str,
    version: &str,
    timestamp: u64,
) -> Result<String, String> {
    let plain = |s: &str| !s.is_empty() && !s.contains(['\t', '\n', '\r']);
    if !plain(version) || !plain(file_name) {
        return Err("the version and the file name must be one line of text, not empty".into());
    }
    // Not `unbase64`: a decoding error would quote a character of the key.
    let text = base64::engine::general_purpose::STANDARD
        .decode(secret_key.trim())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or("the secret key is not what `tauri signer generate` writes (base64 text)")?;
    let key = minisign::SecretKeyBox::from_string(&text)
        .and_then(|b| b.into_secret_key(Some(password.to_owned())))
        .map_err(|e| format!("the secret key cannot be used: {e}"))?;
    let trusted = format!("timestamp:{timestamp}\tfile:{file_name}\tversion:{version}");
    let signature = minisign::sign(
        None,
        &key,
        std::io::Cursor::new(data),
        Some(&trusted),
        Some(UNTRUSTED_COMMENT),
    )
    .map_err(|e| format!("could not sign: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(signature.to_string()))
}

/// Whether `s` is an RFC 3339 date and time that exists (the plugin refuses a manifest whose
/// `pub_date` it cannot read): `2026-11-21T10:00:00Z`, with optional fractions (up to
/// nanoseconds) and an offset instead of `Z`. Months, days (leap years included), hours,
/// minutes, seconds and offsets out of range are refused, and so is a leap second.
pub fn is_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    let number = |r: std::ops::Range<usize>| -> Option<u32> {
        let digits = b.get(r)?;
        digits
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| digits.iter().fold(0, |n, d| n * 10 + u32::from(d - b'0')))
    };
    let shape = b.len() >= 20
        && b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b'T' || b[10] == b't')
        && b[13] == b':'
        && b[16] == b':';
    if !shape {
        return false;
    }
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        number(0..4),
        number(5..7),
        number(8..10),
        number(11..13),
        number(14..16),
        number(17..19),
    ) else {
        return false;
    };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    if day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let mut rest = &s[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let n = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 || n > 9 {
            return false;
        }
        rest = &fraction[n..];
    }
    if rest == "Z" || rest == "z" {
        return true;
    }
    let zone = rest.as_bytes();
    let offset = |r: std::ops::Range<usize>| {
        zone.get(r.clone())
            .filter(|d| d.iter().all(u8::is_ascii_digit))
            .map(|d| d.iter().fold(0u32, |n, d| n * 10 + u32::from(d - b'0')))
    };
    zone.len() == 6
        && (zone[0] == b'+' || zone[0] == b'-')
        && zone[3] == b':'
        && offset(1..3).is_some_and(|h| h <= 23)
        && offset(4..6).is_some_and(|m| m <= 59)
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
        for ok in [
            "2028-02-29T23:59:59Z",
            "2000-02-29T00:00:00Z",
            "2026-12-31T00:00:00.123456789Z",
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
            // Dates and times that do not exist.
            "2026-13-01T10:00:00Z",
            "2026-00-01T10:00:00Z",
            "2026-11-31T10:00:00Z",
            "2026-11-00T10:00:00Z",
            "2026-02-29T10:00:00Z",
            "1900-02-29T10:00:00Z",
            "2026-11-21T24:00:00Z",
            "2026-11-21T10:60:00Z",
            "2026-11-21T10:00:60Z",
            "2026-11-21T10:00:00+24:00",
            "2026-11-21T10:00:00+06:60",
            "2026-11-21T10:00:00.1234567890Z",
            "2026-1x-21T10:00:00Z",
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

    /// A throwaway key pair made by the real Tauri CLI for these tests only (`tauri signer
    /// generate -p gg-test-only`), and the CLI's own signature of `compat.bin` with it
    /// (`tauri signer sign --app-version 2.3.4`).
    const CLI_KEY: &str = include_str!("../tests/fixtures/cli-test.key");
    const CLI_PUB: &str = include_str!("../tests/fixtures/cli-test.key.pub");
    const CLI_PASSWORD: &str = "gg-test-only";
    const COMPAT: &[u8] = include_bytes!("../tests/fixtures/compat.bin");
    const COMPAT_CLI_SIG: &str = include_str!("../tests/fixtures/compat.bin.cli.sig");
    const CLI_TIMESTAMP: u64 = 1795247674;

    #[test]
    fn signs_with_a_tauri_cli_key_as_the_updater_verifies() {
        let sig = sign(CLI_KEY, CLI_PASSWORD, COMPAT, "compat.bin", "2.3.4", 1).unwrap();
        // The plugin's verifier, with the version bound (requireSignedVersion).
        assert_eq!(verify(CLI_PUB, COMPAT, &sig, "2.3.4"), Ok(()));
        assert_eq!(
            verify(CLI_PUB, COMPAT, &sig, "2.3.5"),
            Err("it was signed for version 2.3.4, not 2.3.5".to_owned())
        );
        let tampered = [COMPAT, b"x"].concat();
        assert!(
            verify(CLI_PUB, &tampered, &sig, "2.3.4")
                .unwrap_err()
                .contains("does not verify")
        );
        // Another key's public half does not verify it.
        assert!(
            verify(KEY, COMPAT, &sig, "2.3.4")
                .unwrap_err()
                .contains("does not verify")
        );
    }

    #[test]
    fn a_wrong_password_or_key_signs_nothing_and_says_so_without_the_secret() {
        let secrets = |e: &str| {
            e.contains(CLI_PASSWORD) || e.contains(CLI_KEY.trim()) || e.contains("wrong-one")
        };
        let e = sign(CLI_KEY, "wrong-one", COMPAT, "compat.bin", "2.3.4", 1).unwrap_err();
        assert!(e.contains("Wrong password"), "{e}");
        assert!(!secrets(&e), "{e}");
        let e = sign(CLI_KEY, "", COMPAT, "compat.bin", "2.3.4", 1).unwrap_err();
        assert!(e.contains("Wrong password"), "{e}");
        for key in ["", "not base64 at all!", "bm90IGEga2V5"] {
            let e = sign(key, CLI_PASSWORD, COMPAT, "compat.bin", "2.3.4", 1).unwrap_err();
            assert!(!secrets(&e) && !e.contains(key) || key.is_empty(), "{e}");
        }
        // A key that is only the public half.
        assert!(sign(CLI_PUB, CLI_PASSWORD, COMPAT, "compat.bin", "2.3.4", 1).is_err());
        for (name, version) in [
            ("a\tb", "1.0.0"),
            ("compat.bin", "1.0\n0"),
            ("compat.bin", ""),
        ] {
            assert!(sign(CLI_KEY, CLI_PASSWORD, COMPAT, name, version, 1).is_err());
        }
    }

    /// The parts of a `.sig` file's text: its lines, and its signature's algorithm and key id.
    fn parts(sig: &str) -> (Vec<String>, Vec<u8>) {
        let text = unbase64("sig", sig).unwrap();
        let lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&lines[1])
            .unwrap();
        (lines, raw[..10].to_vec())
    }

    /// What the plugin reads from a signature is what `tauri signer sign --app-version`
    /// writes: the same untrusted comment, algorithm (prehashed Ed25519) and key id, the
    /// same trusted comment (timestamp, file, version, in that order), the same layout.
    #[test]
    fn the_signature_is_laid_out_as_the_tauri_cli_writes_it() {
        assert_eq!(
            verify(CLI_PUB, COMPAT, COMPAT_CLI_SIG, "2.3.4"),
            Ok(()),
            "the CLI's own"
        );
        let ours = sign(
            CLI_KEY,
            CLI_PASSWORD,
            COMPAT,
            "compat.bin",
            "2.3.4",
            CLI_TIMESTAMP,
        )
        .unwrap();
        let (cli_lines, cli_head) = parts(COMPAT_CLI_SIG);
        let (our_lines, our_head) = parts(&ours);
        assert_eq!(our_lines.len(), cli_lines.len());
        assert_eq!(our_lines.len(), 5, "four lines, each ended by a newline");
        assert_eq!(our_lines[4], "");
        assert_eq!(our_lines[0], cli_lines[0]);
        assert_eq!(
            our_lines[0],
            "untrusted comment: signature from tauri secret key"
        );
        assert_eq!(our_head, cli_head, "algorithm and key id");
        assert_eq!(&our_head[..2], b"ED", "prehashed");
        assert_eq!(our_lines[2], cli_lines[2], "the trusted comment");
        assert_eq!(
            our_lines[2],
            format!("trusted comment: timestamp:{CLI_TIMESTAMP}\tfile:compat.bin\tversion:2.3.4")
        );
        assert_eq!(verify(CLI_PUB, COMPAT, &ours, "2.3.4"), Ok(()));
    }
}
