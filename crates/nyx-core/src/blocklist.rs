// blocklist.rs - parses IP blocklists (PeerGuardian text "name:1.2.3.4-5.6.7.8", plain
// "1.2.3.4-5.6.7.8", single addresses and CIDR "1.2.3.0/24"). The text is untrusted: it is
// size-limited, never panics, and bad lines are skipped.

use std::net::IpAddr;

/// Most ranges kept from one list; a longer list is cut here rather than eating memory.
pub const MAX_RANGES: usize = 2_000_000;
/// Largest list file Nyx will read, in bytes.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;

/// An inclusive range of addresses to block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Range {
    pub first: IpAddr,
    pub last: IpAddr,
}

/// Parses a whole list. Comment lines (`#`, `//`, `;`) and blank lines are ignored, as is anything
/// that is not a valid range. Ranges whose ends are different IP versions or reversed are skipped.
pub fn parse_blocklist(text: &str) -> Vec<Range> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") || line.starts_with(';') {
            continue;
        }
        if let Some(r) = parse_line(line) {
            out.push(r);
            if out.len() >= MAX_RANGES {
                break;
            }
        }
    }
    out
}

/// One line: optional `name:` prefix (PeerGuardian), then `a-b`, `a/len` or a single address.
fn parse_line(line: &str) -> Option<Range> {
    // The name may itself contain ':' (and IPv6 addresses do), so find the last ':' whose tail
    // still starts a range: try the whole line first, then each suffix after a ':'.
    if let Some(r) = parse_range(line) {
        return Some(r);
    }
    line.match_indices(':').find_map(|(i, _)| parse_range(line[i + 1..].trim()))
}

fn parse_range(s: &str) -> Option<Range> {
    if let Some((a, b)) = s.split_once('-') {
        let (first, last) = (a.trim().parse::<IpAddr>().ok()?, b.trim().parse::<IpAddr>().ok()?);
        return ordered(first, last);
    }
    if let Some((a, len)) = s.split_once('/') {
        let first = a.trim().parse::<IpAddr>().ok()?;
        return cidr(first, len.trim().parse::<u8>().ok()?);
    }
    let ip = s.parse::<IpAddr>().ok()?;
    Some(Range { first: ip, last: ip })
}

/// A range only if both ends are the same IP version and first <= last.
fn ordered(first: IpAddr, last: IpAddr) -> Option<Range> {
    match (first, last) {
        (IpAddr::V4(a), IpAddr::V4(b)) if a <= b => Some(Range { first, last }),
        (IpAddr::V6(a), IpAddr::V6(b)) if a <= b => Some(Range { first, last }),
        _ => None,
    }
}

/// Expands `addr/len` to its first and last address.
fn cidr(ip: IpAddr, len: u8) -> Option<Range> {
    match ip {
        IpAddr::V4(a) if len <= 32 => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            let first = u32::from(a) & mask;
            Some(Range { first: IpAddr::V4(first.into()), last: IpAddr::V4((first | !mask).into()) })
        }
        IpAddr::V6(a) if len <= 128 => {
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            let first = u128::from(a) & mask;
            Some(Range { first: IpAddr::V6(first.into()), last: IpAddr::V6((first | !mask).into()) })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: &str, b: &str) -> Range {
        Range { first: a.parse().unwrap(), last: b.parse().unwrap() }
    }

    #[test]
    fn parses_peerguardian_plain_cidr_and_single() {
        let text = "# comment\n\nBad Org:1.2.3.4-1.2.3.9\n5.6.7.8-5.6.7.9\n10.0.0.0/8\n9.9.9.9\nName: with colon:2001:db8::1-2001:db8::ff\n";
        let v = parse_blocklist(text);
        assert_eq!(v[0], r("1.2.3.4", "1.2.3.9"));
        assert_eq!(v[1], r("5.6.7.8", "5.6.7.9"));
        assert_eq!(v[2], r("10.0.0.0", "10.255.255.255"));
        assert_eq!(v[3], r("9.9.9.9", "9.9.9.9"));
        assert_eq!(v[4], r("2001:db8::1", "2001:db8::ff"));
        assert_eq!(v.len(), 5);
    }

    #[test]
    fn skips_junk_reversed_and_mixed_versions() {
        let v = parse_blocklist("garbage\n9.9.9.9-1.1.1.1\n1.1.1.1-::1\n1.2.3.4/33\n//x\n;y\n");
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn cidr_edges() {
        assert_eq!(parse_blocklist("0.0.0.0/0")[0], r("0.0.0.0", "255.255.255.255"));
        assert_eq!(parse_blocklist("1.2.3.4/32")[0], r("1.2.3.4", "1.2.3.4"));
    }

    #[test]
    fn never_panics_on_arbitrary_text() {
        for s in ["", ":", "-", "/", "::", "a:b:c", "1.2.3.4-", "-1.2.3.4", "\u{0}\u{ffff}", "1.2.3.4/-1"] {
            let _ = parse_blocklist(s);
        }
    }
}
