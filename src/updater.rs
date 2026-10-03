//! Updates from GitHub Releases.
//!
//! Every release carries SHA256SUMS.txt and an Ed25519 signature of it, made with a key that never
//! leaves the maintainer's machine. Bridge only installs a download whose hash is in a list signed
//! by the public key compiled in below, so a tampered release or a hijacked download is refused.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::path::Path;

const REPO: &str = "ishanmalu/bridge";
const PUBLIC_KEY: &str = "682cc431b9479e0ddb27539bff67e1a18425a1e2d6ace8c6a2223201123b440a";

#[cfg(target_os = "macos")]
const ASSET: &str = "Bridge-mac.zip";
#[cfg(windows)]
const ASSET: &str = "Bridge-windows.exe";
#[cfg(not(any(target_os = "macos", windows)))]
const ASSET: &str = "";

#[derive(Debug, Clone, serde::Serialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    asset: String,
    sums: String,
    sig: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    body: Option<String>,
    html_url: String,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent(concat!("Bridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    agent()
        .get(url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim_start_matches('v').split(['.', '-']).map_while(|p| p.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    parse_version(candidate) > parse_version(current)
}

/// The newest release, if it is newer than this build.
pub fn check() -> Result<Option<Release>, String> {
    let body = get(&format!("https://api.github.com/repos/{REPO}/releases/latest"), 1 << 20)?;
    let r: GhRelease = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if !is_newer(&r.tag_name, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let find = |name: &str| r.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.clone());
    let (Some(asset), Some(sums), Some(sig)) = (find(ASSET), find("SHA256SUMS.txt"), find("SHA256SUMS.txt.sig")) else {
        return Err(format!("{} has no signed download for this system", r.tag_name));
    };
    Ok(Some(Release {
        version: r.tag_name.trim_start_matches('v').to_string(),
        notes: r.body.unwrap_or_default(),
        url: r.html_url,
        asset,
        sums,
        sig,
    }))
}

/// Checks `sums` against `sig` and returns the expected SHA-256 for `name`.
fn expected_hash(sums: &[u8], sig: &[u8], name: &str) -> Result<String, String> {
    let key: [u8; 32] = hex::decode(PUBLIC_KEY).unwrap().try_into().unwrap();
    expected_hash_with(&key, sums, sig, name)
}

fn expected_hash_with(key: &[u8; 32], sums: &[u8], sig: &[u8], name: &str) -> Result<String, String> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let key = VerifyingKey::from_bytes(key).map_err(|e| e.to_string())?;
    let sig = Signature::from_slice(sig).map_err(|_| "malformed signature".to_string())?;
    key.verify(sums, &sig).map_err(|_| "the update's signature doesn't match; refusing to install it".to_string())?;
    let text = std::str::from_utf8(sums).map_err(|e| e.to_string())?;
    text.lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, n)| n.trim().trim_start_matches('*') == name)
        .map(|(h, _)| h.to_lowercase())
        .ok_or_else(|| format!("{name} isn't in the signed checksum list"))
}

/// Download, verify and install `r`, then relaunch. Only returns on failure.
pub fn install(r: &Release, progress: impl Fn(f32)) -> Result<(), String> {
    let sums = get(&r.sums, 1 << 16)?;
    let sig = get(&r.sig, 1 << 10)?;
    let want = expected_hash(&sums, &sig, ASSET)?;

    let resp = agent().get(&r.asset).call().map_err(|e| e.to_string())?;
    let total: Option<u64> = resp.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok());
    let mut reader = resp.into_body().into_with_config().limit(200 << 20).reader();
    let mut data = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(t) = total {
            progress((data.len() as f32 / t as f32).min(1.0));
        }
    }
    let got = hex::encode(Sha256::digest(&data));
    if got != want {
        return Err("the download was corrupted or tampered with; nothing was changed".into());
    }
    log::info!("update {} downloaded and verified", r.version);
    apply(&data)
}

fn temp_dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("bridge-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// The .app bundle this binary lives in.
#[cfg(target_os = "macos")]
pub fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

#[cfg(target_os = "macos")]
fn apply(zip: &[u8]) -> Result<(), String> {
    use std::process::Command;
    let bundle = app_bundle().ok_or("Bridge isn't running from an app bundle")?;
    if bundle.to_string_lossy().contains("/AppTranslocation/") {
        return Err("move Bridge to Applications first, then update".into());
    }
    let tmp = temp_dir();
    let zip_path = tmp.join("Bridge.zip");
    std::fs::write(&zip_path, zip).map_err(|e| e.to_string())?;
    let ok = Command::new("ditto").args(["-x", "-k"]).arg(&zip_path).arg(&tmp).status().is_ok_and(|s| s.success());
    let fresh = tmp.join("Bridge.app");
    if !ok || !fresh.exists() {
        return Err("couldn't unpack the update".into());
    }
    if !Command::new("codesign").args(["--verify", "--deep"]).arg(&fresh).status().is_ok_and(|s| s.success()) {
        return Err("the update's code signature is broken; nothing was changed".into());
    }
    // Swap bundles, keeping the old one until the new one is in place.
    let old = bundle.with_extension("app.old");
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(&bundle, &old).map_err(|e| format!("can't replace {}: {e}", bundle.display()))?;
    if let Err(e) = Command::new("ditto").arg(&fresh).arg(&bundle).status().map_err(|e| e.to_string()).and_then(|s| {
        if s.success() { Ok(()) } else { Err("copy failed".into()) }
    }) {
        let _ = std::fs::remove_dir_all(&bundle);
        let _ = std::fs::rename(&old, &bundle);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&old);
    let _ = std::fs::remove_dir_all(&tmp);
    relaunch_mac(&bundle)
}

#[cfg(target_os = "macos")]
pub fn relaunch_mac(bundle: &Path) -> Result<(), String> {
    log::info!("relaunching {}", bundle.display());
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("sleep 1; /usr/bin/open -n {:?}", bundle))
        .spawn()
        .map_err(|e| e.to_string())?;
    std::process::exit(0);
}

#[cfg(windows)]
fn apply(exe_bytes: &[u8]) -> Result<(), String> {
    // A running exe can't be overwritten, but it can be renamed out of the way.
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = exe.with_file_name("Bridge.old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old).map_err(|e| format!("can't replace {} (run Bridge as administrator): {e}", exe.display()))?;
    if let Err(e) = std::fs::write(&exe, exe_bytes) {
        let _ = std::fs::rename(&old, &exe);
        return Err(e.to_string());
    }
    let _ = temp_dir();
    log::info!("relaunching {}", exe.display());
    crate::install::relaunch_windows(&exe)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn apply(_: &[u8]) -> Result<(), String> {
    Err("unsupported".into())
}

/// Leftovers from the previous version.
pub fn clean_up() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_file_name("Bridge.old.exe"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("v0.1.1", "0.1.1"));
        assert!(!is_newer("0.1.0", "0.1.1"));
    }

    #[test]
    fn signed_sums_verify_and_tampering_is_caught() {
        use ed25519_dalek::{Signer, SigningKey};
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = sk.verifying_key().to_bytes();
        let sums = b"aa11  Bridge-mac.zip\nbb22  Bridge-windows.exe\n";
        let sig = sk.sign(sums).to_bytes();
        assert_eq!(expected_hash_with(&pk, sums, &sig, "Bridge-windows.exe").unwrap(), "bb22");
        assert!(expected_hash_with(&pk, sums, &sig, "Other.zip").is_err());
        let mut forged = sums.to_vec();
        forged[0] = b'c';
        assert!(expected_hash_with(&pk, &forged, &sig, "Bridge-mac.zip").is_err());
    }

    #[test]
    fn built_in_key_is_valid() {
        let key: [u8; 32] = hex::decode(PUBLIC_KEY).unwrap().try_into().unwrap();
        assert!(ed25519_dalek::VerifyingKey::from_bytes(&key).is_ok());
    }

    #[test]
    fn tampered_sums_are_refused() {
        let sig = [0u8; 64];
        assert!(expected_hash(b"abc  Bridge-mac.zip\n", &sig, "Bridge-mac.zip").is_err());
    }
}
