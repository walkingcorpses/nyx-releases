// update.rs - self-update from the public releases repo. Pure rules plus the download and swap.
//   check:    GitHub "latest release" JSON -> a newer version, its exe asset and the SHA-256 GitHub
//             reports for that asset
//   download: only from this repo's release downloads, size-capped, hash verified before it is kept
//   swap:     a running exe can be renamed but not overwritten, so the old exe becomes `nyx.exe.old`
//             and the new one takes its place; the old one is deleted at the next start
// Release rules (see the README of the releases repo): tag `vX.Y.Z`, asset `Nyx-X.Y.Z-windows-x64.exe`.

use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::http;

/// The public repo that holds the releases.
pub const REPO: &str = "walkingcorpses/nyx-releases";
/// Largest exe Nyx will download for an update.
const MAX_EXE_BYTES: u64 = 120 * 1024 * 1024;

/// A newer release that can be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub sha256: String,
}

/// Parses "1.2.3" or "v1.2.3" into numbers; anything else is None.
fn parse_version(v: &str) -> Option<[u64; 3]> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let out = [it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?];
    if it.next().is_some() { None } else { Some(out) }
}

/// Is `candidate` a strictly higher version than `current`? Unparseable versions are never newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Reads GitHub's release JSON. Returns the update if the release is newer than `current` and has
/// the expected exe asset with a SHA-256 digest; otherwise None.
pub fn parse_release(json: &[u8], current: &str) -> Option<UpdateInfo> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    if v["draft"].as_bool().unwrap_or(false) || v["prerelease"].as_bool().unwrap_or(false) {
        return None;
    }
    let tag = v["tag_name"].as_str()?;
    let version = tag.trim_start_matches('v').to_string();
    if !is_newer(&version, current) {
        return None;
    }
    let want = format!("Nyx-{version}-windows-x64.exe");
    let asset = v["assets"].as_array()?.iter().find(|a| a["name"].as_str() == Some(want.as_str()))?;
    let url = asset["browser_download_url"].as_str()?.to_string();
    let digest = asset["digest"].as_str()?;
    let sha256 = digest.strip_prefix("sha256:")?.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) || !asset_url_ok(&url) {
        return None;
    }
    Some(UpdateInfo { version, notes: v["body"].as_str().unwrap_or("").chars().take(4000).collect(), url, sha256 })
}

/// Only this repo's release downloads are trusted as an update source.
pub fn asset_url_ok(url: &str) -> bool {
    url.starts_with(&format!("https://github.com/{REPO}/releases/download/"))
}

/// Asks GitHub for the latest release and returns an update if there is one newer than `current`.
pub fn check(agent: &ureq::Agent, current: &str) -> Result<Option<UpdateInfo>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = http::get_bytes(agent, &url, 1024 * 1024)?;
    Ok(parse_release(&body, current))
}

/// Downloads the update to `dest`, but only keeps it if its SHA-256 matches what GitHub reported.
pub fn download(agent: &ureq::Agent, info: &UpdateInfo, dest: &Path) -> Result<(), String> {
    if !asset_url_ok(&info.url) {
        return Err("update URL is not from the official releases repo".into());
    }
    download_unchecked(agent, &info.url, &info.sha256, MAX_EXE_BYTES, dest)
}

/// The download itself, without the URL allow-list (tests use a local server).
fn download_unchecked(agent: &ureq::Agent, url: &str, sha256: &str, max: u64, dest: &Path) -> Result<(), String> {
    let bytes = http::get_bytes(agent, url, max)?;
    let got = hex(&Sha256::digest(&bytes));
    if !got.eq_ignore_ascii_case(sha256) {
        return Err(format!("download failed its checksum (expected {sha256}, got {got}); not installed"));
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(dest, &bytes).map_err(|e| e.to_string())
}

/// Lowercase hex of a byte slice.
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Path the previous exe is parked at during a swap.
fn old_path(current: &Path) -> std::path::PathBuf {
    let mut s = current.as_os_str().to_os_string();
    s.push(".old");
    s.into()
}

/// Replaces `current` (the running exe) with `new`: parks the old one as `<exe>.old`, copies the new
/// one into place, and puts the old one back if the copy fails.
pub fn swap_exe(new: &Path, current: &Path) -> io::Result<()> {
    let old = old_path(current);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(current, &old)?;
    if let Err(e) = std::fs::copy(new, current) {
        let _ = std::fs::remove_file(current);
        let _ = std::fs::rename(&old, current);
        return Err(e);
    }
    Ok(())
}

/// Deletes the parked old exe from a previous update, if any (called at startup).
pub fn cleanup_old(current: &Path) {
    let _ = std::fs::remove_file(old_path(current));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn version_ordering() {
        assert!(is_newer("1.0.1", "1.0.0"));
        assert!(is_newer("v1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(!is_newer("1.0.0-beta", "0.1.0"), "unparseable is never newer");
        assert!(!is_newer("garbage", "1.0.0"));
        assert!(!is_newer("1.0", "0.1.0"));
    }

    fn release(tag: &str, asset: &str, url: &str, digest: &str, extra: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","body":"notes","draft":false,"prerelease":false,{extra}
              "assets":[{{"name":"{asset}","browser_download_url":"{url}","digest":"{digest}"}}]}}"#
        )
    }

    const SHA: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn parse_release_accepts_a_proper_newer_release() {
        let url = format!("https://github.com/{REPO}/releases/download/v1.2.0/Nyx-1.2.0-windows-x64.exe");
        let j = release("v1.2.0", "Nyx-1.2.0-windows-x64.exe", &url, SHA, "");
        let u = parse_release(j.as_bytes(), "1.1.0").unwrap();
        assert_eq!((u.version.as_str(), u.notes.as_str(), u.url.as_str()), ("1.2.0", "notes", url.as_str()));
        assert_eq!(u.sha256.len(), 64);
        assert!(parse_release(j.as_bytes(), "1.2.0").is_none(), "same version is not an update");
    }

    #[test]
    fn parse_release_rejects_bad_releases() {
        let good = format!("https://github.com/{REPO}/releases/download/v1.2.0/Nyx-1.2.0-windows-x64.exe");
        // wrong asset name, missing digest, bad digest, foreign host, prerelease
        let cases = [
            release("v1.2.0", "other.exe", &good, SHA, ""),
            release("v1.2.0", "Nyx-1.2.0-windows-x64.exe", &good, "", ""),
            release("v1.2.0", "Nyx-1.2.0-windows-x64.exe", &good, "sha256:abc", ""),
            release("v1.2.0", "Nyx-1.2.0-windows-x64.exe", "https://evil.example/Nyx-1.2.0-windows-x64.exe", SHA, ""),
            release("v1.2.0", "Nyx-1.2.0-windows-x64.exe", &good, SHA, r#""prerelease":true,"#).replace(r#""prerelease":false,"#, ""),
        ];
        for c in cases {
            assert!(parse_release(c.as_bytes(), "1.1.0").is_none(), "{c}");
        }
        assert!(parse_release(b"not json", "1.0.0").is_none());
    }

    /// Serves `body` once per connection on loopback.
    fn serve(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for conn in l.incoming().flatten() {
                let mut c = conn;
                let mut b = [0u8; 2048];
                let _ = c.read(&mut b);
                let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                let _ = c.write_all(&body);
            }
        });
        format!("http://{addr}/nyx.exe")
    }

    #[test]
    fn download_keeps_only_a_file_that_matches_its_checksum() {
        let data = b"pretend this is an exe".to_vec();
        let url = serve(data.clone());
        let dir = tempfile::tempdir().unwrap();
        let agent = http::agent(None).unwrap();
        let good = hex(&Sha256::digest(&data));
        let dest = dir.path().join("u").join("nyx.exe");
        download_unchecked(&agent, &url, &good, 1000, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);

        let bad_dest = dir.path().join("bad.exe");
        let err = download_unchecked(&agent, &url, &"0".repeat(64), 1000, &bad_dest).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(!bad_dest.exists(), "a file that failed verification must not be kept");
    }

    #[test]
    fn official_download_refuses_foreign_urls() {
        let info = UpdateInfo { version: "9.9.9".into(), notes: String::new(), url: "http://127.0.0.1:1/x".into(), sha256: "0".repeat(64) };
        let dir = tempfile::tempdir().unwrap();
        let err = download(&http::agent(None).unwrap(), &info, &dir.path().join("x")).unwrap_err();
        assert!(err.contains("official"), "{err}");
    }

    #[test]
    fn swap_replaces_the_exe_and_cleanup_removes_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        let (cur, new) = (dir.path().join("nyx.exe"), dir.path().join("new.exe"));
        std::fs::write(&cur, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();
        swap_exe(&new, &cur).unwrap();
        assert_eq!(std::fs::read(&cur).unwrap(), b"new");
        assert_eq!(std::fs::read(old_path(&cur)).unwrap(), b"old");
        cleanup_old(&cur);
        assert!(!old_path(&cur).exists());
    }

    #[test]
    fn swap_puts_the_old_exe_back_when_the_copy_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cur = dir.path().join("nyx.exe");
        std::fs::write(&cur, b"old").unwrap();
        assert!(swap_exe(&dir.path().join("missing.exe"), &cur).is_err());
        assert_eq!(std::fs::read(&cur).unwrap(), b"old");
    }
}
