// Untrusted-input validation: magnet links, .torrent bytes, and file paths
// from torrent metadata. Everything here must reject bad input, never panic.

use std::path::{Path, PathBuf};

use data_encoding::{BASE32_NOPAD, HEXLOWER};
use percent_encoding::percent_decode_str;

/// Largest .torrent file we will hand to the engine.
const MAX_TORRENT_BYTES: usize = 10 * 1024 * 1024;
/// Deepest bencode list/dict nesting we accept.
const MAX_BENCODE_DEPTH: usize = 64;
/// Longest full output path (Windows MAX_PATH-ish) we accept.
const MAX_PATH_CHARS: usize = 260;

/// Why an untrusted input was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotMagnet,
    MissingHash,
    BadHash,
    TooLarge,
    TooDeep,
    /// Carries the offending input / reason for the UI and logs.
    UnsafePath(String),
}

/// Parsed magnet link. `info_hash` is lowercase hex (40 chars v1, 64 chars v2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Magnet {
    pub info_hash: String,
    pub name: Option<String>,
    pub trackers: Vec<String>,
}

/// Turn a user-pasted magnet URI into a `Magnet`, normalising the hash to lowercase hex.
/// Why: the engine and UI key torrents by hex hash, and pasted links come in hex, base32 and v2 forms.
/// Unknown parameters are ignored; with several `xt` the first valid one wins (hybrid links).
pub fn parse_magnet(uri: &str) -> Result<Magnet, ParseError> {
    // Scheme is case-insensitive; compare bytes so non-ASCII input can't hit a char boundary.
    let b = uri.as_bytes();
    if b.len() < 8 || !b[..8].eq_ignore_ascii_case(b"magnet:?") {
        return Err(ParseError::NotMagnet);
    }
    let (mut hash, mut saw_xt) = (None, false);
    let (mut name, mut trackers) = (None, Vec::new());
    for pair in uri[8..].split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        match k.to_ascii_lowercase().as_str() {
            "xt" => {
                saw_xt = true;
                if hash.is_none() {
                    hash = decode_xt(&percent_decode_str(v).decode_utf8_lossy());
                }
            }
            // '+' means space in magnet display names.
            "dn" if name.is_none() => {
                name = Some(percent_decode_str(&v.replace('+', " ")).decode_utf8_lossy().into_owned())
            }
            "tr" => {
                let t = percent_decode_str(v).decode_utf8_lossy().into_owned();
                if !t.is_empty() {
                    trackers.push(t);
                }
            }
            _ => {}
        }
    }
    match (hash, saw_xt) {
        (Some(info_hash), _) => Ok(Magnet { info_hash, name, trackers }),
        (None, true) => Err(ParseError::BadHash),
        (None, false) => Err(ParseError::MissingHash),
    }
}

/// Decode one `xt` value (btih hex/base32 or btmh multihash) to lowercase hex, or None if invalid.
fn decode_xt(xt: &str) -> Option<String> {
    let lower = xt.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("urn:btih:") {
        let is_hex = |s: &str| s.bytes().all(|c| c.is_ascii_hexdigit());
        if h.len() == 40 && is_hex(h) {
            return Some(h.to_string());
        }
        // 32-char base32 = 20 bytes; base32 decoding wants uppercase.
        if h.len() == 32 {
            let bytes = BASE32_NOPAD.decode(h.to_ascii_uppercase().as_bytes()).ok()?;
            return (bytes.len() == 20).then(|| HEXLOWER.encode(&bytes));
        }
    } else if let Some(h) = lower.strip_prefix("urn:btmh:1220") {
        // 0x12 = sha2-256, 0x20 = 32 bytes.
        if h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Some(h.to_string());
        }
    }
    None
}

/// Cheap pre-check of untrusted .torrent bytes before the engine parses them: size cap and
/// bencode nesting cap. Why: deeply nested input can blow the stack of recursive parsers.
/// Iterative (only a depth counter, no allocation), so hostile input can't overflow our stack.
/// Structural errors (bad lengths, stray bytes) are not reported here: the scan just stops and
/// the engine's full parser rejects them. This is a guard, not a validator.
pub fn check_torrent_bytes(bytes: &[u8]) -> Result<(), ParseError> {
    if bytes.len() > MAX_TORRENT_BYTES {
        return Err(ParseError::TooLarge);
    }
    let (mut i, mut depth) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'l' | b'd' => {
                depth += 1;
                if depth > MAX_BENCODE_DEPTH {
                    return Err(ParseError::TooDeep);
                }
                i += 1;
            }
            b'e' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            // Integer: skip to its terminating 'e'.
            b'i' => match bytes[i..].iter().position(|&c| c == b'e') {
                Some(n) => i += n + 1,
                None => break,
            },
            // String "<len>:<payload>": skip the payload so its bytes aren't misread as structure.
            b'0'..=b'9' => {
                let mut len = 0usize;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    len = len.saturating_mul(10).saturating_add((bytes[i] - b'0') as usize);
                    i += 1;
                }
                if bytes.get(i) != Some(&b':') {
                    break;
                }
                i = i.saturating_add(1).saturating_add(len);
            }
            _ => break,
        }
    }
    Ok(())
}

/// Windows device names that can never be file names (with or without an extension).
const RESERVED: [&str; 24] = [
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM1", "COM2", "COM3", "COM4", "COM5",
    "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
    "LPT8", "LPT9",
];

/// Join a torrent-supplied relative path onto `base`, refusing anything that could escape it
/// or that Windows would treat specially. Why: torrent metadata is attacker-controlled and
/// a path like `..\..\Startup\x.exe` or `C:\...` would write outside the download folder.
/// Accepts both `/` and `\` as separators regardless of host OS.
pub fn safe_join(base: &Path, rel: &str) -> Result<PathBuf, ParseError> {
    let bad = |why: &str| ParseError::UnsafePath(format!("{rel:?}: {why}"));
    if rel.is_empty() {
        return Err(bad("empty"));
    }
    // ':' covers drive letters and NTFS alternate streams; the rest are illegal on Windows.
    if rel.chars().any(|c| c.is_control() || matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*')) {
        return Err(bad("forbidden character"));
    }
    let mut out = base.to_path_buf();
    for part in rel.split(['/', '\\']) {
        // Empty parts cover leading separators (absolute, UNC) and doubled separators.
        if part.is_empty() || part == "." || part == ".." {
            return Err(bad("empty, '.' or '..' component"));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(bad("trailing dot or space"));
        }
        // Device name is the part before the first '.', ignoring trailing spaces ("con .txt").
        let stem = part.split('.').next().unwrap_or("").trim_end_matches(' ');
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
            return Err(bad("reserved device name"));
        }
        out.push(part);
    }
    if out.to_string_lossy().chars().count() > MAX_PATH_CHARS {
        return Err(bad("path too long"));
    }
    Ok(out)
}

/// Builds a shareable magnet link for a torrent id (the inverse of `parse_magnet`).
/// A 64-char id is a v2 hash and goes out as a multihash (`btmh`, prefix 1220).
/// Why: the "copy magnet" action only knows the id, the name and the tracker list.
pub fn build_magnet(id: &str, name: &str, trackers: &[String]) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let xt = if id.len() == 64 { format!("urn:btmh:1220{id}") } else { format!("urn:btih:{id}") };
    let mut uri = format!("magnet:?xt={xt}&dn={}", utf8_percent_encode(name, NON_ALPHANUMERIC));
    for t in trackers {
        uri.push_str("&tr=");
        uri.push_str(&utf8_percent_encode(t, NON_ALPHANUMERIC).to_string());
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const HEX: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn magnet_uppercase_hex_normalised_to_lowercase() {
        let m = parse_magnet(&format!("magnet:?xt=urn:btih:{}", HEX.to_uppercase())).unwrap();
        assert_eq!(m.info_hash, HEX);
    }

    #[test]
    fn magnet_base32_hash_decoded_to_hex() {
        // 20 bytes of 0x00 -> 32 x 'A' in base32; 20 bytes of 0xff -> 32 x '7'
        let m = parse_magnet("magnet:?xt=urn:btih:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
        assert_eq!(m.info_hash, "0".repeat(40));
        let m = parse_magnet("magnet:?xt=urn:btih:77777777777777777777777777777777").unwrap();
        assert_eq!(m.info_hash, "f".repeat(40));
    }

    #[test]
    fn magnet_v2_btmh_accepted() {
        let h = "ab".repeat(32);
        let m = parse_magnet(&format!("magnet:?xt=urn:btmh:1220{h}")).unwrap();
        assert_eq!(m.info_hash.len(), 64);
        assert_eq!(m.info_hash, h);
    }

    #[test]
    fn magnet_missing_xt_is_missing_hash() {
        assert_eq!(parse_magnet("magnet:?dn=x"), Err(ParseError::MissingHash));
    }

    #[test]
    fn magnet_bad_hash_is_bad_hash() {
        assert_eq!(parse_magnet("magnet:?xt=urn:btih:zzzz"), Err(ParseError::BadHash));
        assert_eq!(parse_magnet("magnet:?xt=urn:btmh:1220abcd"), Err(ParseError::BadHash));
    }

    #[test]
    fn magnet_wrong_scheme_is_not_magnet() {
        assert_eq!(parse_magnet("http://x/?xt=urn:btih:abc"), Err(ParseError::NotMagnet));
        assert_eq!(parse_magnet(""), Err(ParseError::NotMagnet));
    }

    #[test]
    fn magnet_junk_params_ignored() {
        let uri = format!(
            "magnet:?foo=bar&xt=urn:btih:{HEX}&dn=My%20Show+S01&tr=udp%3A%2F%2Fa%3A1%2Fann&tr=http%3A%2F%2Fb%2Fann&&=&zzz"
        );
        let m = parse_magnet(&uri).unwrap();
        assert_eq!(m.info_hash, HEX);
        assert_eq!(m.name.as_deref(), Some("My Show S01"));
        assert_eq!(m.trackers, vec!["udp://a:1/ann", "http://b/ann"]);
    }

    #[test]
    fn torrent_over_10mib_is_too_large() {
        let big = vec![b'x'; 10 * 1024 * 1024 + 1];
        assert_eq!(check_torrent_bytes(&big), Err(ParseError::TooLarge));
        assert_eq!(check_torrent_bytes(&big[..10 * 1024 * 1024]), Ok(()));
    }

    #[test]
    fn torrent_nesting_65_deep_is_too_deep() {
        let nest = |n: usize| [vec![b'l'; n], vec![b'e'; n]].concat();
        assert_eq!(check_torrent_bytes(&nest(65)), Err(ParseError::TooDeep));
        assert_eq!(check_torrent_bytes(&nest(64)), Ok(()));
        // unterminated, very deep input must not overflow the stack either
        assert_eq!(check_torrent_bytes(&vec![b'd'; 5_000_000]), Err(ParseError::TooDeep));
    }

    #[test]
    fn torrent_skips_strings_and_ints() {
        // string payload made of 'l' bytes must not count as nesting
        assert_eq!(check_torrent_bytes(b"d4:llll3:lleli5ee"), Ok(()));
    }

    #[test]
    fn safe_join_rejects() {
        let base = Path::new(r"C:\dl");
        let long = "a".repeat(300);
        for bad in [
            "../x", "a/../../x", r"..\x", r"C:\x", "C:x", r"\\srv\share\x", "/abs", "CON",
            "con.txt", "NUL.log", "dir/com1.tar.gz", "lpt9 .x", "name.", "name ", "dir/name.",
            "a//b", "", "x:stream", "a\0b", "a<b", "q?", &long,
        ] {
            assert!(
                matches!(safe_join(base, bad), Err(ParseError::UnsafePath(_))),
                "should reject {bad:?}"
            );
        }
    }

    #[test]
    fn safe_join_accepts_unicode_and_nested() {
        let base = Path::new(r"C:\dl");
        let p = safe_join(base, "dir/файл.txt").unwrap();
        assert!(p.starts_with(base));
        assert!(p.ends_with("файл.txt"));
        assert!(safe_join(base, r"a\b\c.mkv").unwrap().starts_with(base));
        // names that merely contain a reserved word are fine
        assert!(safe_join(base, "console.txt").is_ok());
    }

    #[test]
    fn build_magnet_round_trips_through_parse() {
        let id = "ab".repeat(20);
        let uri = build_magnet(&id, "My Film (2020)", &["http://t.example/announce".to_string()]);
        let m = parse_magnet(&uri).unwrap();
        assert_eq!(m.info_hash, id);
        assert_eq!(m.name.as_deref(), Some("My Film (2020)"));
        assert_eq!(m.trackers, vec!["http://t.example/announce".to_string()]);
        let v2 = build_magnet(&"cd".repeat(32), "x", &[]);
        assert_eq!(parse_magnet(&v2).unwrap().info_hash.len(), 64);
    }
}
