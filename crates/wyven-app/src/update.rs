//! Updates: Wyven reads a small manifest (`update.json`, made by `scripts/release.ps1`) from the
//! address in settings.toml (`[update] url`, a web address or a local/network path). If it names
//! a newer version, Wyven asks; only after "yes" it downloads the installer, checks its SHA-256
//! and runs it (the installer closes Wyven and starts it again).
//!
//! Automatic checks happen at most once a day; "Help: Check for Updates" checks right away.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const DAY: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// The installer: absolute, or relative to the manifest.
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub notes: String,
}

pub enum UpdateEvent {
    /// Result of a check: a newer version, or none (`manual`: say so).
    Checked { found: Option<Manifest>, manual: bool },
    /// The installer is downloaded and verified.
    Ready(PathBuf),
    Failed(String),
}

/// `"0.10.2"` → `[0, 10, 2]` (anything after `-` or `+` is ignored).
fn parts(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches('v').split(['-', '+']).next().unwrap_or("").split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (a, b) = (parts(candidate), parts(current));
    let n = a.len().max(b.len());
    let get = |v: &Vec<u64>, i: usize| v.get(i).copied().unwrap_or(0);
    (0..n).map(|i| get(&a, i).cmp(&get(&b, i))).find(|o| o.is_ne()) == Some(std::cmp::Ordering::Greater)
}

fn is_web(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(60))).build().into()
}

fn read(source: &str) -> Result<Vec<u8>, String> {
    if is_web(source) {
        let resp = agent().get(source).call().map_err(|e| format!("{source}: {e}"))?;
        let mut body = resp.into_body();
        body.with_config().limit(512 << 20).read_to_vec().map_err(|e| format!("{source}: {e}"))
    } else {
        std::fs::read(source).map_err(|e| format!("{source}: {e}"))
    }
}

/// The installer's address: absolute, or next to the manifest.
pub fn resolve(manifest_source: &str, url: &str) -> String {
    if is_web(url) || Path::new(url).is_absolute() {
        return url.to_string();
    }
    if is_web(manifest_source) {
        let base = manifest_source.rsplit_once('/').map_or(manifest_source, |(b, _)| b);
        format!("{base}/{url}")
    } else {
        Path::new(manifest_source).parent().unwrap_or(Path::new(".")).join(url).display().to_string()
    }
}

fn stamp_file() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("last-update-check"))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Automatic checks: at most once a day.
pub fn due() -> bool {
    let last = stamp_file().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse::<u64>().ok());
    last.is_none_or(|t| now().saturating_sub(t) >= DAY)
}

fn remember_check() {
    if let Some(p) = stamp_file() {
        let _ = std::fs::write(p, now().to_string());
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The manifest (a UTF-8 BOM, as Windows tools like to write, is fine).
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|e| format!("update manifest: {e}"))
}

/// Checks in the background; the answer comes through the returned receiver.
pub fn check(source: String, manual: bool, wake: impl Fn() + Send + 'static) -> Receiver<UpdateEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("update-check".into())
        .spawn(move || {
            let result = read(&source).and_then(|bytes| parse_manifest(&bytes));
            remember_check();
            let ev = match result {
                Ok(m) if is_newer(&m.version, CURRENT) => UpdateEvent::Checked { found: Some(m), manual },
                Ok(_) => UpdateEvent::Checked { found: None, manual },
                Err(e) => UpdateEvent::Failed(e),
            };
            let _ = tx.send(ev);
            wake();
        })
        .ok();
    rx
}

/// Downloads (or copies) the installer to the temp folder and checks its hash.
pub fn download(source: String, manifest: Manifest, wake: impl Fn() + Send + 'static) -> Receiver<UpdateEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("update-download".into())
        .spawn(move || {
            let ev = (|| {
                let from = resolve(&source, &manifest.url);
                let bytes = read(&from)?;
                let hash = sha256_hex(&bytes);
                if !hash.eq_ignore_ascii_case(manifest.sha256.trim()) {
                    return Err("the downloaded installer does not match its checksum – not installed".to_string());
                }
                let path = std::env::temp_dir().join(format!("wyven-setup-{}.exe", manifest.version));
                std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                Ok(path)
            })();
            let _ = tx.send(match ev {
                Ok(p) => UpdateEvent::Ready(p),
                Err(e) => UpdateEvent::Failed(e),
            });
            wake();
        })
        .ok();
    rx
}

/// Starts the installer quietly; it closes Wyven and opens the new version.
pub fn run_installer(path: &Path) -> Result<(), String> {
    std::process::Command::new(path)
        .args(["/SILENT", "/SP-", "/NOCANCEL", "/CLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot start the installer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.5"));
        assert!(is_newer("v1.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0-beta", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    #[test]
    fn the_release_manifest_parses_with_or_without_bom() {
        let json = br#"{ "version": "0.2.0", "url": "wyven-setup-0.2.0.exe", "sha256": "ab", "notes": "x" }"#;
        let with_bom = [b"\xEF\xBB\xBF".as_slice(), json].concat();
        assert_eq!(parse_manifest(json).unwrap(), parse_manifest(&with_bom).unwrap());
        assert_eq!(parse_manifest(json).unwrap().version, "0.2.0");
    }

    #[test]
    fn installer_paths_are_relative_to_the_manifest() {
        assert_eq!(resolve("https://x.org/wyven/update.json", "setup.exe"), "https://x.org/wyven/setup.exe");
        assert_eq!(resolve("https://x.org/update.json", "https://y.org/s.exe"), "https://y.org/s.exe");
        let local = resolve(r"\\server\share\wyven\update.json", "setup.exe");
        assert!(local.ends_with(r"wyven\setup.exe"), "{local}");
    }

    #[test]
    fn a_tampered_installer_is_refused() {
        let dir = std::env::temp_dir().join(format!("wyven-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("setup.exe"), b"installer bytes").unwrap();
        let good = sha256_hex(b"installer bytes");
        let manifest = |sha: &str| Manifest { version: "9.9.9".into(), url: "setup.exe".into(), sha256: sha.into(), notes: String::new() };
        let source = dir.join("update.json").display().to_string();
        let rx = download(source.clone(), manifest(&good), || {});
        assert!(matches!(rx.recv().unwrap(), UpdateEvent::Ready(_)));
        let rx = download(source, manifest(&"0".repeat(64)), || {});
        assert!(matches!(rx.recv().unwrap(), UpdateEvent::Failed(e) if e.contains("checksum")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
