// http.rs - the little HTTP client behind update checks, RSS feeds, search and .torrent
// downloads. Rules: only http(s); strict size and time limits; and if a SOCKS5 proxy is set, every
// request goes through it with hostnames resolved by the proxy (socks5h), so these requests can
// never reveal the real IP or do a local DNS lookup. If the proxy is down the request just fails.

use std::time::Duration;

use nyx_core::ProxySettings;

/// Whole-request time limit.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Sent with every request.
const USER_AGENT: &str = concat!("Nyx/", env!("CARGO_PKG_VERSION"));

/// Builds an agent that uses `proxy` when given.
pub fn agent(proxy: Option<&ProxySettings>) -> Result<ureq::Agent, String> {
    let mut b = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .max_redirects(5)
        .user_agent(USER_AGENT);
    if let Some(p) = proxy {
        let auth = match (p.username.as_deref().filter(|u| !u.is_empty()), p.password.as_deref()) {
            (Some(u), pw) => format!("{}:{}@", urlencoding::encode(u), urlencoding::encode(pw.unwrap_or(""))),
            _ => String::new(),
        };
        let url = format!("socks5h://{auth}{}:{}", p.host, p.port);
        b = b.proxy(Some(ureq::Proxy::new(&url).map_err(|e| format!("bad proxy: {e}"))?));
    }
    Ok(b.build().into())
}

/// True for an http or https URL (case-insensitive scheme).
pub fn is_http_url(url: &str) -> bool {
    let l = url.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// GETs `url` and returns at most `max` bytes; a longer body is an error, not a truncation.
pub fn get_bytes(agent: &ureq::Agent, url: &str, max: u64) -> Result<Vec<u8>, String> {
    if !is_http_url(url) {
        return Err("only http and https links are allowed".into());
    }
    let mut resp = agent.get(url.trim()).call().map_err(|e| format!("request failed: {e}"))?;
    resp.body_mut()
        .with_config()
        .limit(max)
        .read_to_vec()
        .map_err(|e| format!("could not read the response (too large or interrupted): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-shot HTTP server on loopback answering every request with `body`.
    pub fn serve(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for conn in l.incoming().flatten() {
                let mut c = conn;
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf);
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = c.write_all(head.as_bytes());
                let _ = c.write_all(&body);
            }
        });
        format!("http://{addr}/x")
    }

    #[test]
    fn get_bytes_returns_body_and_enforces_limit() {
        let url = serve(vec![7u8; 1000]);
        let a = agent(None).unwrap();
        assert_eq!(get_bytes(&a, &url, 5000).unwrap().len(), 1000);
        assert!(get_bytes(&a, &url, 500).is_err());
    }

    #[test]
    fn rejects_non_http_schemes() {
        let a = agent(None).unwrap();
        assert!(get_bytes(&a, "file:///C:/Windows/win.ini", 100).is_err());
        assert!(get_bytes(&a, "ftp://example.com/x", 100).is_err());
        assert!(is_http_url(" HTTPS://x"));
    }

    #[test]
    fn unreachable_proxy_means_failure_not_direct() {
        let url = serve(vec![1u8; 10]);
        // A proxy that nothing listens on: the (reachable) server must NOT be contacted directly.
        let dead = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = dead.local_addr().unwrap().port();
        drop(dead);
        let p = ProxySettings { host: "127.0.0.1".into(), port, username: None, password: None };
        let a = agent(Some(&p)).unwrap();
        assert!(get_bytes(&a, &url, 100).is_err());
    }
}
