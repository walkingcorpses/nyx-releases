// conf.rs: hand-written parser for WireGuard `.conf` files (the kind VPN providers hand out).
// It accepts exactly one [Interface] and one [Peer], requires a full-tunnel AllowedIPs, and
// never executes anything (PostUp/PreUp etc. are ignored). Privacy rule: the private key is never
// placed in an error message or Debug output, so a failed import cannot leak it into logs or the UI.

use base64::{engine::general_purpose::STANDARD, Engine};
use std::fmt;
use std::net::IpAddr;

/// Largest config we will even look at; real files are well under 1 KiB, so this just blocks abuse.
const MAX_CONF_BYTES: usize = 64 * 1024;

/// The peer's `host:port`. The host stays a string because it may be a DNS name resolved later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// A validated single-peer WireGuard configuration.
#[derive(Clone)]
pub struct WgConfig {
    pub private_key: [u8; 32],
    pub addresses: Vec<(IpAddr, u8)>,
    pub dns: Vec<IpAddr>,
    pub peer_public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    pub endpoint: Endpoint,
    pub keepalive: Option<u16>,
}

// Manual Debug so a stray `{:?}` (log line, assert, panic) can never print key material.
impl fmt::Debug for WgConfig {
    /// Prints every field except the two secrets, which show as "<redacted>".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WgConfig")
            .field("private_key", &"<redacted>")
            .field("addresses", &self.addresses)
            .field("dns", &self.dns)
            .field("peer_public_key", &self.peer_public_key)
            .field("preshared_key", &self.preshared_key.map(|_| "<redacted>"))
            .field("endpoint", &self.endpoint)
            .field("keepalive", &self.keepalive)
            .finish()
    }
}

/// Why a config was rejected. Messages are shown in the UI, so they are specific and never echo
/// the offending value (values may be secrets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfError {
    TooLarge,
    /// A line that is neither a comment, a `[section]` nor `key = value` (1-based line number).
    BadLine(usize),
    UnknownSection(String),
    /// A `key = value` line before any section header.
    OutsideSection(usize),
    TooManyPeers,
    Duplicate(&'static str),
    Missing(&'static str),
    /// A value that could not be parsed: (key name, what is wrong).
    BadValue(&'static str, &'static str),
    NotFullRoute,
}

impl fmt::Display for ConfError {
    /// Human-readable one-line message for each rejection reason.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfError::TooLarge => write!(f, "The WireGuard config is larger than 64 KiB, so it is not a normal config file."),
            ConfError::BadLine(n) => write!(f, "Line {n} is not a comment, a [section] header or a 'key = value' pair."),
            ConfError::UnknownSection(s) => write!(f, "Unknown section [{s}]; only [Interface] and [Peer] are supported."),
            ConfError::OutsideSection(n) => write!(f, "Line {n} has a setting before any [Interface] or [Peer] header."),
            ConfError::TooManyPeers => write!(f, "The config has more than one [Peer]; Nyx supports only one peer (one VPN server)."),
            ConfError::Duplicate(k) => write!(f, "{k} appears more than once."),
            ConfError::Missing(k) => write!(f, "The config is missing {k}."),
            ConfError::BadValue(k, why) => write!(f, "{k} is invalid: {why}."),
            ConfError::NotFullRoute => write!(f, "AllowedIPs must include both 0.0.0.0/0 and ::/0; Nyx only supports full-tunnel configs so no traffic can leak outside the VPN."),
        }
    }
}

impl std::error::Error for ConfError {}

/// Decodes a base64 WireGuard key into 32 bytes; the error names the key but never shows it.
fn parse_key(name: &'static str, v: &str) -> Result<[u8; 32], ConfError> {
    let bytes = STANDARD
        .decode(v)
        .map_err(|_| ConfError::BadValue(name, "not valid base64"))?;
    bytes
        .try_into()
        .map_err(|_| ConfError::BadValue(name, "must decode to exactly 32 bytes"))
}

/// Parses `ip` or `ip/prefix` (prefix defaults to a single host); used by Address and AllowedIPs.
fn parse_cidr(name: &'static str, v: &str) -> Result<(IpAddr, u8), ConfError> {
    let (ip, prefix) = match v.split_once('/') {
        Some((ip, p)) => (ip.trim(), Some(p.trim())),
        None => (v, None),
    };
    let ip: IpAddr = ip
        .parse()
        .map_err(|_| ConfError::BadValue(name, "an entry is not a valid IP address"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix = match prefix {
        None => max,
        Some(p) => p
            .parse::<u8>()
            .ok()
            .filter(|p| *p <= max)
            .ok_or(ConfError::BadValue(name, "an entry has an invalid prefix length"))?,
    };
    Ok((ip, prefix))
}

/// Parses `host:port`, `v4:port` or `[v6]:port`; the host is stored without brackets.
fn parse_endpoint(v: &str) -> Result<Endpoint, ConfError> {
    const BAD: ConfError = ConfError::BadValue("Endpoint", "expected host:port, e.g. vpn.example.com:51820 or [2001:db8::1]:51820");
    let (host, port) = if let Some(rest) = v.strip_prefix('[') {
        let (h, after) = rest.split_once(']').ok_or(BAD)?;
        (h, after.strip_prefix(':').ok_or(BAD)?)
    } else {
        v.rsplit_once(':').ok_or(BAD)?
    };
    // An unbracketed host containing ':' is ambiguous IPv6 and is rejected.
    if host.is_empty() || (!v.starts_with('[') && host.contains(':')) {
        return Err(BAD);
    }
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|p| *p != 0)
        .ok_or(ConfError::BadValue("Endpoint", "the port must be a number from 1 to 65535"))?;
    Ok(Endpoint { host: host.to_string(), port })
}

/// Sets a single-valued field, failing if it was already set (e.g. two PrivateKey lines).
fn set_once<T>(slot: &mut Option<T>, name: &'static str, val: T) -> Result<(), ConfError> {
    if slot.is_some() {
        return Err(ConfError::Duplicate(name));
    }
    *slot = Some(val);
    Ok(())
}

/// Parses and validates a WireGuard config. Input over 64 KiB is rejected before any parsing.
pub fn parse_conf(text: &str) -> Result<WgConfig, ConfError> {
    if text.len() > MAX_CONF_BYTES {
        return Err(ConfError::TooLarge);
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    #[derive(PartialEq, Clone, Copy)]
    enum Section {
        None,
        Interface,
        Peer,
    }
    let mut section = Section::None;
    let mut peers = 0;
    let (mut private_key, mut public_key, mut psk, mut endpoint, mut keepalive) = (None, None, None, None, None);
    let (mut addresses, mut dns, mut allowed) = (Vec::new(), Vec::new(), Vec::new());

    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = match name.trim().to_ascii_lowercase().as_str() {
                "interface" => Section::Interface,
                "peer" => {
                    peers += 1;
                    if peers > 1 {
                        return Err(ConfError::TooManyPeers);
                    }
                    Section::Peer
                }
                other => {
                    // Echo only short plain names: a mangled secret pasted as a header must not leak.
                    let safe = other.len() <= 20 && other.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                    return Err(ConfError::UnknownSection(if safe { other.to_string() } else { "...".to_string() }));
                }
            };
            continue;
        }
        let (key, val) = line.split_once('=').ok_or(ConfError::BadLine(n))?;
        let (key, val) = (key.trim().to_ascii_lowercase(), val.trim());
        match (section, key.as_str()) {
            (Section::None, _) => return Err(ConfError::OutsideSection(n)),
            (Section::Interface, "privatekey") => set_once(&mut private_key, "PrivateKey", parse_key("PrivateKey", val)?)?,
            (Section::Interface, "address") => {
                for part in val.split(',') {
                    addresses.push(parse_cidr("Address", part.trim())?);
                }
            }
            (Section::Interface, "dns") => {
                for part in val.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    dns.push(part.parse::<IpAddr>().map_err(|_| ConfError::BadValue("DNS", "an entry is not a valid IP address"))?);
                }
            }
            (Section::Peer, "publickey") => set_once(&mut public_key, "PublicKey", parse_key("PublicKey", val)?)?,
            (Section::Peer, "presharedkey") => set_once(&mut psk, "PresharedKey", parse_key("PresharedKey", val)?)?,
            (Section::Peer, "endpoint") => set_once(&mut endpoint, "Endpoint", parse_endpoint(val)?)?,
            (Section::Peer, "allowedips") => {
                for part in val.split(',') {
                    allowed.push(parse_cidr("AllowedIPs", part.trim())?);
                }
            }
            (Section::Peer, "persistentkeepalive") => {
                let k = val.parse::<u16>().map_err(|_| ConfError::BadValue("PersistentKeepalive", "must be a number from 0 to 65535"))?;
                set_once(&mut keepalive, "PersistentKeepalive", k)?;
            }
            // MTU/Table/PostUp/PreUp/PostDown/PreDown/SaveConfig/ListenPort/FwMark and anything else:
            // deliberately ignored. We never run hooks or touch system routing.
            _ => {}
        }
    }

    if peers == 0 {
        return Err(ConfError::Missing("a [Peer] section"));
    }
    let full = |ip: IpAddr| allowed.iter().any(|(a, p)| *a == ip && *p == 0);
    if !(full("0.0.0.0".parse().unwrap()) && full("::".parse().unwrap())) {
        // Missing AllowedIPs entirely is the same problem: no full route.
        return Err(ConfError::NotFullRoute);
    }
    if addresses.is_empty() {
        return Err(ConfError::Missing("Address"));
    }
    Ok(WgConfig {
        private_key: private_key.ok_or(ConfError::Missing("PrivateKey"))?,
        addresses,
        dns,
        peer_public_key: public_key.ok_or(ConfError::Missing("PublicKey"))?,
        preshared_key: psk,
        endpoint: endpoint.ok_or(ConfError::Missing("Endpoint"))?,
        keepalive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIV: &str = "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const PUB: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";
    const PSK: &str = "FpCyhws9cxwWoV4xELtfJvjJN+zQVRPISllRWgeopVE=";

    /// Mullvad-style sample used as the baseline valid config.
    fn sample() -> String {
        format!(
            "[Interface]\nPrivateKey = {PRIV}\nAddress = 10.2.0.2/32, fc00::2/128\nDNS = 10.64.0.1\n\n\
             [Peer]\nPublicKey = {PUB}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = 193.32.126.66:51820\n"
        )
    }

    #[test]
    fn parses_valid_sample() {
        let c = parse_conf(&sample()).unwrap();
        assert_eq!(c.private_key, STANDARD.decode(PRIV).unwrap().as_slice());
        assert_eq!(c.addresses, vec![("10.2.0.2".parse().unwrap(), 32), ("fc00::2".parse().unwrap(), 128)]);
        assert_eq!(c.dns, vec!["10.64.0.1".parse::<IpAddr>().unwrap()]);
        assert_eq!(c.endpoint, Endpoint { host: "193.32.126.66".into(), port: 51820 });
        assert_eq!(c.keepalive, None);
        assert_eq!(c.preshared_key, None);
    }

    #[test]
    fn accepts_bom_crlf_and_comments() {
        let text = format!(
            "\u{feff}# provider\r\n; note\r\n\r\n[interface]\r\nprivatekey=  {PRIV}\r\nADDRESS = 10.2.0.2/32\r\n\r\n\
             [PEER]\r\nPublicKey = {PUB}\r\nPresharedKey = {PSK}\r\nAllowedIPs = 0.0.0.0/0, ::/0\r\n\
             Endpoint = [2001:db8::1]:51820\r\nPersistentKeepalive = 25\r\n"
        );
        let c = parse_conf(&text).unwrap();
        assert_eq!(c.endpoint, Endpoint { host: "2001:db8::1".into(), port: 51820 });
        assert_eq!(c.keepalive, Some(25));
        assert!(c.preshared_key.is_some());
        assert!(c.dns.is_empty());
    }

    #[test]
    fn accepts_hostname_endpoint_and_ignores_harmless_extras() {
        let text = sample()
            .replace("DNS = 10.64.0.1", "MTU = 1380\nTable = off\nPostUp = rm -rf /\nListenPort = 5\nSaveConfig = true\nFwMark = 1")
            .replace("193.32.126.66", "vpn.example.com");
        let c = parse_conf(&text).unwrap();
        assert_eq!(c.endpoint.host, "vpn.example.com");
    }

    #[test]
    fn rejects_two_peers() {
        let text = format!("{}\n[Peer]\nPublicKey = {PUB}\n", sample());
        let e = parse_conf(&text).unwrap_err();
        assert!(e.to_string().contains("only one peer"), "{e}");
    }

    #[test]
    fn rejects_missing_endpoint() {
        let text = sample().replace("Endpoint = 193.32.126.66:51820\n", "");
        assert_eq!(parse_conf(&text).unwrap_err(), ConfError::Missing("Endpoint"));
    }

    #[test]
    fn rejects_short_or_non_base64_key() {
        let short = sample().replace(PRIV, "AAAA");
        assert!(matches!(parse_conf(&short), Err(ConfError::BadValue("PrivateKey", _))));
        let junk = sample().replace(PRIV, "not base64 !!!");
        assert!(matches!(parse_conf(&junk), Err(ConfError::BadValue("PrivateKey", _))));
        let bad_pub = sample().replace(PUB, "AAAA");
        assert!(matches!(parse_conf(&bad_pub), Err(ConfError::BadValue("PublicKey", _))));
    }

    #[test]
    fn rejects_allowed_ips_not_full_route() {
        let text = sample().replace("0.0.0.0/0, ::/0", "10.0.0.0/8");
        assert_eq!(parse_conf(&text).unwrap_err(), ConfError::NotFullRoute);
        let v4_only = sample().replace("0.0.0.0/0, ::/0", "0.0.0.0/0");
        assert_eq!(parse_conf(&v4_only).unwrap_err(), ConfError::NotFullRoute);
    }

    #[test]
    fn accepts_allowed_ips_with_both_default_routes() {
        let text = sample().replace("0.0.0.0/0, ::/0", "10.0.0.0/8, 0.0.0.0/0, ::/0");
        assert!(parse_conf(&text).is_ok());
    }

    #[test]
    fn rejects_duplicate_private_key() {
        let text = sample().replace("DNS = 10.64.0.1", &format!("PrivateKey = {PRIV}"));
        assert_eq!(parse_conf(&text).unwrap_err(), ConfError::Duplicate("PrivateKey"));
    }

    #[test]
    fn rejects_unknown_section_and_bad_lines() {
        let text = format!("{}\n[Extra]\n", sample());
        assert!(matches!(parse_conf(&text), Err(ConfError::UnknownSection(_))));
        assert!(matches!(parse_conf("[Interface]\nnonsense\n"), Err(ConfError::BadLine(2))));
        assert!(matches!(parse_conf("PrivateKey = x\n"), Err(ConfError::OutsideSection(1))));
    }

    #[test]
    fn rejects_input_over_64_kib() {
        let text = format!("{}\n# {}\n", sample(), "x".repeat(64 * 1024));
        assert_eq!(parse_conf(&text).unwrap_err(), ConfError::TooLarge);
    }

    #[test]
    fn error_never_contains_the_private_key() {
        // Every failing variant of a config that carries the real private key.
        let cases = [
            format!("{}\n[Peer]\nPublicKey = {PUB}\n", sample()),
            sample().replace("Endpoint = 193.32.126.66:51820\n", ""),
            sample().replace(PUB, "AAAA"),
            sample().replace("0.0.0.0/0, ::/0", "10.0.0.0/8"),
            sample().replace("DNS = 10.64.0.1", &format!("PrivateKey = {PRIV}")),
            format!("{}\n# {}\n", sample(), "x".repeat(64 * 1024)),
            format!("{}\n[{PRIV}]\n", sample()),
            format!("[Interface]\nPrivateKey {PRIV}\n"),
            format!("PrivateKey = {PRIV}\n"),
            sample().replace("Address = 10.2.0.2/32, fc00::2/128", &format!("Address = {PRIV}")),
            sample().replace("51820", &format!("{PRIV}")),
        ];
        for text in cases {
            let e = parse_conf(&text).expect_err("case must fail");
            assert!(!e.to_string().contains(PRIV), "leaked in: {e}");
            assert!(!format!("{e:?}").contains(PRIV), "leaked in debug: {e:?}");
        }
    }

    #[test]
    fn debug_redacts_secrets() {
        let text = sample().replace("AllowedIPs", &format!("PresharedKey = {PSK}\nAllowedIPs"));
        let c = parse_conf(&text).unwrap();
        let d = format!("{c:?}");
        assert!(!d.contains(PRIV) && !d.contains(PSK) && d.contains("<redacted>"), "{d}");
        // Raw bytes must not appear either, in any common rendering.
        assert!(!d.contains(&format!("{:?}", c.private_key)));
    }
}
