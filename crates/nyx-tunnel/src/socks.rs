// socks.rs: a loopback-only SOCKS5 server (RFC 1928 CONNECT + UDP ASSOCIATE, RFC 1929 password
// auth only) that forwards everything through a pluggable `Backend`. libtorrent and the HTTP client
// point at it, and the backend (the WireGuard tunnel) is the only way out, so nothing can leak.
// Privacy rules: the server never resolves names itself (domains go to the backend untouched),
// binds 127.0.0.1 only, and requires random per-instance credentials so other local programs
// cannot ride the tunnel.

use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rand::distr::Alphanumeric;
use rand::Rng;

/// Where a client wants to go. A domain is kept as text: only the backend may resolve it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Addr(SocketAddr),
    Domain(String, u16),
}

/// A bidirectional byte stream produced by a backend (a tunnelled TCP connection).
pub trait Stream: Read + Write + Send {
    /// Second handle to the same stream so each relay direction owns one.
    fn try_clone_box(&self) -> io::Result<Box<dyn Stream>>;
    /// Unblock readers on every handle and close the stream. Default does nothing, which would
    /// leave a relay thread parked until the far end closes, so real backends should override it.
    fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }
}

/// One UDP association's channel through the backend.
pub trait UdpPort: Send {
    /// Send a payload to `to` through the tunnel.
    fn send_to(&self, payload: &[u8], to: &Target) -> io::Result<()>;
    /// Wait up to `timeout` for a datagram; `None` means nothing arrived in time.
    fn recv(&self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<(usize, SocketAddr)>>;
}

/// The way out. The SOCKS server only speaks the protocol; the backend moves the bytes.
pub trait Backend: Send + Sync + 'static {
    fn connect_tcp(&self, t: &Target) -> io::Result<Box<dyn Stream>>;
    fn udp_open(&self) -> io::Result<Box<dyn UdpPort>>;
}

/// Username/password the clients must present; random for every server instance.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// Handles to a running server; dropping it stops the listener.
pub struct SocksServer {
    addr: SocketAddr,
    creds: Credentials,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Whole-handshake budget; a client that stalls longer is dropped.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent connection cap; extra connections are closed at once so a flood cannot exhaust threads.
const MAX_CONNECTIONS: usize = 512;
/// How long each UDP relay direction waits before checking the other direction and the stop flag.
const UDP_POLL: Duration = Duration::from_millis(10);
/// Random characters in each generated credential.
const CRED_LEN: usize = 32;

impl SocksServer {
    /// Binds 127.0.0.1 on a free port, makes fresh random credentials and starts accepting.
    pub fn start(backend: Arc<dyn Backend>) -> io::Result<SocksServer> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let creds = Credentials { username: random_token(), password: random_token() };
        let shutdown = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let (flag, c) = (shutdown.clone(), creds.clone());
        let thread = thread::Builder::new().name("socks-accept".into()).spawn(move || {
            for conn in listener.incoming() {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(conn) = conn else {
                    // Persistent accept errors (e.g. out of handles) must not become a busy loop.
                    thread::sleep(Duration::from_millis(50));
                    continue;
                };
                if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    active.fetch_sub(1, Ordering::SeqCst);
                    continue; // dropping `conn` closes it
                }
                let guard = ConnGuard(active.clone());
                let (backend, c) = (backend.clone(), c.clone());
                // If spawn fails the closure (and its guard) is dropped, releasing the slot.
                let _ = thread::Builder::new().name("socks-conn".into()).spawn(move || {
                    let _guard = guard;
                    let _ = handle(conn, backend.as_ref(), &c);
                });
            }
        })?;
        Ok(SocksServer { addr, creds, shutdown, thread: Some(thread) })
    }

    /// Address clients connect to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The credentials clients must use.
    pub fn credentials(&self) -> Credentials {
        self.creds.clone()
    }
}

impl Drop for SocksServer {
    /// Stops the listener: set the flag, then connect to ourselves so the blocking accept wakes up.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Releases a connection slot when its thread ends, however it ends.
struct ConnGuard(Arc<AtomicUsize>);
impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Random alphanumeric string from the OS-seeded CSPRNG, used for credentials.
fn random_token() -> String {
    rand::rng().sample_iter(Alphanumeric).take(CRED_LEN).map(char::from).collect()
}

/// Compares secrets without exiting early on the first differing byte, so timing leaks nothing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

/// Maps a backend error to the SOCKS5 reply code (RFC 1928 section 6).
fn reply_code(e: &io::Error) -> u8 {
    match e.kind() {
        io::ErrorKind::NetworkUnreachable => 0x03,
        io::ErrorKind::TimedOut => 0x04,
        io::ErrorKind::ConnectionRefused => 0x05,
        _ => 0x01,
    }
}

/// Reads exactly `buf.len()` bytes but gives up when `deadline` passes, bounding slow-drip clients.
fn read_by(s: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> io::Result<()> {
    let left = deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
    s.set_read_timeout(Some(left))?;
    s.read_exact(buf)
}

/// Writes a reply with a zeroed IPv4 bound address (used for failures and for CONNECT success).
fn reply(s: &mut TcpStream, code: u8) -> io::Result<()> {
    s.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0])
}

/// Parses ATYP-specific address bytes (`rest` begins after ATYP; for domains it starts at the length
/// byte) into a Target. Returns the target and bytes consumed, or None if truncated/invalid.
fn parse_addr(atyp: u8, rest: &[u8]) -> Option<(Target, usize)> {
    match atyp {
        1 => {
            let b: [u8; 4] = rest.get(..4)?.try_into().ok()?;
            let p = u16::from_be_bytes(rest.get(4..6)?.try_into().ok()?);
            Some((Target::Addr(SocketAddr::new(Ipv4Addr::from(b).into(), p)), 6))
        }
        4 => {
            let b: [u8; 16] = rest.get(..16)?.try_into().ok()?;
            let p = u16::from_be_bytes(rest.get(16..18)?.try_into().ok()?);
            Some((Target::Addr(SocketAddr::new(Ipv6Addr::from(b).into(), p)), 18))
        }
        3 => {
            let n = *rest.first()? as usize;
            let name = std::str::from_utf8(rest.get(1..1 + n)?).ok().filter(|s| !s.is_empty())?;
            let p = u16::from_be_bytes(rest.get(1 + n..3 + n)?.try_into().ok()?);
            Some((Target::Domain(name.to_string(), p), 3 + n))
        }
        _ => None,
    }
}

/// Reads one address of type `atyp` from the stream with fixed, bounded reads (domain length is
/// one byte, so at most 255 + 3 bytes). None means an unsupported or malformed address.
fn read_addr(s: &mut TcpStream, atyp: u8, deadline: Instant) -> io::Result<Option<Target>> {
    let mut buf = Vec::new();
    match atyp {
        1 | 4 => buf.resize(if atyp == 1 { 6 } else { 18 }, 0),
        3 => {
            let mut n = [0u8; 1];
            read_by(s, &mut n, deadline)?;
            buf.push(n[0]);
            buf.resize(1 + n[0] as usize + 2, 0);
            read_by(s, &mut buf[1..], deadline)?;
            return Ok(parse_addr(3, &buf).map(|t| t.0));
        }
        _ => return Ok(None),
    }
    read_by(s, &mut buf, deadline)?;
    Ok(parse_addr(atyp, &buf).map(|t| t.0))
}

/// One client connection: method negotiation, password login, then CONNECT or UDP ASSOCIATE.
fn handle(mut s: TcpStream, backend: &dyn Backend, creds: &Credentials) -> io::Result<()> {
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    s.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;

    // Method negotiation: only 0x02 (username/password) is acceptable.
    let mut head = [0u8; 2];
    read_by(&mut s, &mut head, deadline)?;
    let mut methods = vec![0u8; head[1] as usize];
    read_by(&mut s, &mut methods, deadline)?;
    if head[0] != 5 || !methods.contains(&2) {
        return s.write_all(&[5, 0xFF]);
    }
    s.write_all(&[5, 2])?;

    // RFC 1929 sub-negotiation.
    let mut v = [0u8; 2];
    read_by(&mut s, &mut v, deadline)?; // VER, ULEN
    let mut user = vec![0u8; v[1] as usize];
    read_by(&mut s, &mut user, deadline)?;
    let mut plen = [0u8; 1];
    read_by(&mut s, &mut plen, deadline)?;
    let mut pass = vec![0u8; plen[0] as usize];
    read_by(&mut s, &mut pass, deadline)?;
    // Evaluate both comparisons (no short-circuit) so timing does not reveal which one failed.
    let ok = ct_eq(&user, creds.username.as_bytes()) & ct_eq(&pass, creds.password.as_bytes());
    if v[0] != 1 || !ok {
        return s.write_all(&[1, 1]);
    }
    s.write_all(&[1, 0])?;

    // Request.
    let mut req = [0u8; 4];
    read_by(&mut s, &mut req, deadline)?;
    if req[0] != 5 {
        return reply(&mut s, 0x01);
    }
    let Some(target) = read_addr(&mut s, req[3], deadline)? else {
        return reply(&mut s, 0x08); // address type not supported
    };
    match req[1] {
        1 => {
            let stream = match backend.connect_tcp(&target) {
                Ok(st) => st,
                Err(e) => return reply(&mut s, reply_code(&e)),
            };
            reply(&mut s, 0x00)?;
            s.set_read_timeout(None)?;
            s.set_write_timeout(None)?;
            relay(s, stream)
        }
        3 => {
            let announced = match target {
                Target::Addr(a) => a.port(),
                Target::Domain(_, p) => p,
            };
            udp_associate(s, backend, announced)
        }
        _ => reply(&mut s, 0x07), // BIND and anything unknown: command not supported
    }
}

/// Copies one direction until it ends, then closes both sides so the opposite direction unblocks.
fn pump(mut from: impl Read, mut to: impl Write, client: TcpStream, backend: Box<dyn Stream>) {
    let _ = io::copy(&mut from, &mut to);
    let _ = client.shutdown(Shutdown::Both);
    let _ = backend.shutdown();
}

/// Relays bytes both ways between the client and the backend stream until either side ends.
fn relay(client: TcpStream, backend: Box<dyn Stream>) -> io::Result<()> {
    let up = std::thread::Builder::new().name("socks-up".into()).spawn({
        let (from, ctl) = (client.try_clone()?, client.try_clone()?);
        let (to, bctl) = (backend.try_clone_box()?, backend.try_clone_box()?);
        move || pump(from, to, ctl, bctl)
    })?;
    let bctl = backend.try_clone_box()?;
    pump(backend, client.try_clone()?, client, bctl);
    let _ = up.join();
    Ok(())
}

/// UDP ASSOCIATE: opens a backend UDP port and a loopback relay socket, tells the client where it
/// is, and keeps the association alive exactly as long as the TCP control connection stays open.
fn udp_associate(mut ctl: TcpStream, backend: &dyn Backend, announced_port: u16) -> io::Result<()> {
    let port = match backend.udp_open() {
        Ok(p) => p,
        Err(e) => return reply(&mut ctl, reply_code(&e)),
    };
    let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let bound = sock.local_addr()?;
    let mut r = vec![5, 0, 0, 1];
    if let IpAddr::V4(ip) = bound.ip() {
        r.extend(ip.octets());
    }
    r.extend(bound.port().to_be_bytes());
    ctl.write_all(&r)?;
    ctl.set_read_timeout(None)?;
    ctl.set_write_timeout(None)?;

    let stop = Arc::new(AtomicBool::new(false));
    let peer_ip = ctl.peer_addr()?.ip();
    let t = thread::Builder::new().name("socks-udp".into()).spawn({
        let stop = stop.clone();
        move || udp_loop(sock, port, peer_ip, announced_port, stop)
    })?;
    // The client never sends on the control connection; any read result (EOF/error) ends the association.
    let mut sink = [0u8; 64];
    while matches!(ctl.read(&mut sink), Ok(n) if n > 0) {}
    stop.store(true, Ordering::SeqCst);
    let _ = t.join();
    Ok(())
}

/// Shuttles datagrams between the client's UDP socket and the backend until `stop` is set.
/// Only the control peer's IP, and the announced (or first-used) source port, is accepted.
/// Single loop with short polls because `UdpPort` is only `Send`, so it cannot be shared by threads.
fn udp_loop(sock: UdpSocket, port: Box<dyn UdpPort>, peer_ip: IpAddr, announced: u16, stop: Arc<AtomicBool>) {
    let mut client_port = (announced != 0).then_some(announced);
    let mut buf = vec![0u8; 65536];
    let mut inbuf = vec![0u8; 65536];
    if sock.set_read_timeout(Some(UDP_POLL)).is_err() {
        return;
    }
    while !stop.load(Ordering::SeqCst) {
        // Client -> backend.
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if from.ip() == peer_ip && client_port.is_none_or(|p| p == from.port()) {
                    client_port = Some(from.port());
                    if let Some((target, payload)) = parse_udp(&buf[..n]) {
                        let _ = port.send_to(payload, &target);
                    }
                }
            }
            // Timeout is the normal idle case; Windows also reports ICMP port-unreachable as a reset.
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset) => {}
            Err(_) => return,
        }
        // Backend -> client.
        match port.recv(&mut inbuf, UDP_POLL) {
            Ok(Some((n, src))) => {
                if let Some(p) = client_port {
                    let _ = sock.send_to(&wrap_udp(src, &inbuf[..n]), (peer_ip, p));
                }
            }
            Ok(None) => {}
            Err(_) => return,
        }
    }
}

/// Splits a client UDP datagram into target and payload. Fragmented datagrams (FRAG != 0) and
/// malformed headers are dropped, as RFC 1928 permits.
fn parse_udp(d: &[u8]) -> Option<(Target, &[u8])> {
    if d.len() < 4 || d[0] != 0 || d[1] != 0 || d[2] != 0 {
        return None;
    }
    let (target, used) = parse_addr(d[3], &d[4..])?;
    Some((target, &d[4 + used..]))
}

/// Builds the UDP header (RSV, FRAG=0, address of the remote sender) in front of a payload.
fn wrap_udp(src: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0, 0, 0];
    match src.ip() {
        IpAddr::V4(ip) => {
            v.push(1);
            v.extend(ip.octets());
        }
        IpAddr::V6(ip) => {
            v.push(4);
            v.extend(ip.octets());
        }
    }
    v.extend(src.port().to_be_bytes());
    v.extend(payload);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::sync::Mutex;

    // ---- test backends ----

    impl Stream for TcpStream {
        fn try_clone_box(&self) -> io::Result<Box<dyn Stream>> {
            Ok(Box::new(self.try_clone()?))
        }
        fn shutdown(&self) -> io::Result<()> {
            TcpStream::shutdown(self, Shutdown::Both)
        }
    }

    /// UDP pass-through over a real loopback socket.
    struct LoopbackUdp(UdpSocket);
    impl UdpPort for LoopbackUdp {
        fn send_to(&self, payload: &[u8], to: &Target) -> io::Result<()> {
            match to {
                Target::Addr(a) => self.0.send_to(payload, a).map(|_| ()),
                Target::Domain(..) => Err(io::Error::other("no dns in tests")),
            }
        }
        fn recv(&self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<(usize, SocketAddr)>> {
            self.0.set_read_timeout(Some(timeout))?;
            match self.0.recv_from(buf) {
                Ok((n, a)) => Ok(Some((n, a))),
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(None),
                Err(e) => Err(e),
            }
        }
    }

    /// Connects to loopback servers; optionally maps one domain to a local address and records
    /// every target it is asked for. `fail_with` makes every connect fail with that kind.
    #[derive(Default)]
    struct TestBackend {
        seen: Mutex<Vec<Target>>,
        domain_to: Option<SocketAddr>,
        fail_with: Option<io::ErrorKind>,
    }
    impl Backend for TestBackend {
        fn connect_tcp(&self, t: &Target) -> io::Result<Box<dyn Stream>> {
            self.seen.lock().unwrap().push(t.clone());
            if let Some(k) = self.fail_with {
                return Err(io::Error::from(k));
            }
            let addr = match t {
                Target::Addr(a) => *a,
                Target::Domain(..) => self.domain_to.ok_or_else(|| io::Error::other("unresolvable"))?,
            };
            Ok(Box::new(TcpStream::connect(addr)?))
        }
        fn udp_open(&self) -> io::Result<Box<dyn UdpPort>> {
            Ok(Box::new(LoopbackUdp(UdpSocket::bind("127.0.0.1:0")?)))
        }
    }

    // ---- helpers ----

    /// TCP echo server on loopback (one thread per connection).
    fn echo_server() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        thread::spawn(move || {
            for s in l.incoming().flatten() {
                thread::spawn(move || {
                    let (mut r, mut w) = (s.try_clone().unwrap(), s);
                    let _ = io::copy(&mut r, &mut w);
                });
            }
        });
        addr
    }

    /// Opens a client connection with a read timeout so a broken server fails the test, not hangs it.
    fn client(server: &SocksServer) -> TcpStream {
        let s = TcpStream::connect(server.addr()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s
    }

    /// Greeting offering password auth, then the RFC 1929 login. Returns the auth status byte.
    fn login(s: &mut TcpStream, user: &str, pass: &str) -> u8 {
        s.write_all(&[5, 1, 2]).unwrap();
        let mut r = [0u8; 2];
        s.read_exact(&mut r).unwrap();
        assert_eq!(r, [5, 2]);
        let mut m = vec![1, user.len() as u8];
        m.extend(user.as_bytes());
        m.push(pass.len() as u8);
        m.extend(pass.as_bytes());
        s.write_all(&m).unwrap();
        let mut a = [0u8; 2];
        s.read_exact(&mut a).unwrap();
        assert_eq!(a[0], 1);
        a[1]
    }

    /// Full handshake up to and including the request; returns the reply code and bound port.
    fn request(s: &mut TcpStream, server: &SocksServer, cmd: u8, addr_bytes: &[u8]) -> (u8, SocketAddr) {
        let c = server.credentials();
        assert_eq!(login(s, &c.username, &c.password), 0);
        let mut m = vec![5, cmd, 0];
        m.extend(addr_bytes);
        s.write_all(&m).unwrap();
        let mut h = [0u8; 4];
        s.read_exact(&mut h).unwrap();
        assert_eq!(h[0], 5);
        let ip: IpAddr = match h[3] {
            1 => {
                let mut b = [0u8; 4];
                s.read_exact(&mut b).unwrap();
                Ipv4Addr::from(b).into()
            }
            4 => {
                let mut b = [0u8; 16];
                s.read_exact(&mut b).unwrap();
                Ipv6Addr::from(b).into()
            }
            t => panic!("bad atyp {t}"),
        };
        let mut p = [0u8; 2];
        s.read_exact(&mut p).unwrap();
        (h[1], SocketAddr::new(ip, u16::from_be_bytes(p)))
    }

    /// ATYP+address+port bytes for an IPv4 target.
    fn v4(a: SocketAddr) -> Vec<u8> {
        let SocketAddr::V4(a) = a else { panic!() };
        let mut v = vec![1];
        v.extend(a.ip().octets());
        v.extend(a.port().to_be_bytes());
        v
    }

    fn start(b: TestBackend) -> (SocksServer, Arc<TestBackend>) {
        let b = Arc::new(b);
        (SocksServer::start(b.clone()).unwrap(), b)
    }

    fn assert_echoes(s: &mut TcpStream) {
        s.write_all(b"hello nyx").unwrap();
        let mut got = [0u8; 9];
        s.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"hello nyx");
    }

    // ---- tests ----

    #[test]
    fn connect_by_ipv4_relays_bytes() {
        let echo = echo_server();
        let (srv, _b) = start(TestBackend::default());
        let mut c = client(&srv);
        let (code, _) = request(&mut c, &srv, 1, &v4(echo));
        assert_eq!(code, 0);
        assert_echoes(&mut c);
    }

    #[test]
    fn connect_by_domain_passes_the_name_to_the_backend() {
        let echo = echo_server();
        let (srv, b) = start(TestBackend { domain_to: Some(echo), ..Default::default() });
        let mut c = client(&srv);
        let mut a = vec![3, 9];
        a.extend(b"echo.test");
        a.extend(7u16.to_be_bytes());
        let (code, _) = request(&mut c, &srv, 1, &a);
        // Success proves the name reached the backend unresolved (it cannot resolve locally either).
        assert_eq!(code, 0);
        assert_eq!(*b.seen.lock().unwrap(), vec![Target::Domain("echo.test".into(), 7)]);
        assert_echoes(&mut c);
    }

    #[test]
    fn wrong_password_is_refused() {
        let (srv, b) = start(TestBackend::default());
        let mut c = client(&srv);
        let user = srv.credentials().username;
        assert_ne!(login(&mut c, &user, "not-the-password"), 0);
        // Connection is closed afterwards.
        let mut buf = [0u8; 1];
        assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)));
        assert!(b.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn no_auth_method_is_refused() {
        let (srv, _b) = start(TestBackend::default());
        let mut c = client(&srv);
        c.write_all(&[5, 1, 0]).unwrap();
        let mut r = [0u8; 2];
        c.read_exact(&mut r).unwrap();
        assert_eq!(r, [5, 0xFF]);
        let mut buf = [0u8; 1];
        assert!(matches!(c.read(&mut buf), Ok(0) | Err(_)));
    }

    #[test]
    fn backend_error_gives_general_failure_reply() {
        let (srv, _b) = start(TestBackend { fail_with: Some(io::ErrorKind::Other), ..Default::default() });
        let mut c = client(&srv);
        let (code, _) = request(&mut c, &srv, 1, &v4("127.0.0.1:9".parse().unwrap()));
        assert_eq!(code, 1);
    }

    #[test]
    fn network_unreachable_backend_gives_reply_3() {
        let (srv, _b) = start(TestBackend { fail_with: Some(io::ErrorKind::NetworkUnreachable), ..Default::default() });
        let mut c = client(&srv);
        let (code, _) = request(&mut c, &srv, 1, &v4("127.0.0.1:9".parse().unwrap()));
        assert_eq!(code, 3);
    }

    #[test]
    fn refused_and_timeout_map_to_5_and_4() {
        for (kind, want) in [(io::ErrorKind::ConnectionRefused, 5), (io::ErrorKind::TimedOut, 4)] {
            let (srv, _b) = start(TestBackend { fail_with: Some(kind), ..Default::default() });
            let mut c = client(&srv);
            let (code, _) = request(&mut c, &srv, 1, &v4("127.0.0.1:9".parse().unwrap()));
            assert_eq!(code, want);
        }
    }

    #[test]
    fn bind_command_is_not_supported() {
        let (srv, _b) = start(TestBackend::default());
        let mut c = client(&srv);
        let (code, _) = request(&mut c, &srv, 2, &v4("127.0.0.1:9".parse().unwrap()));
        assert_eq!(code, 7);
    }

    #[test]
    fn ipv6_target_is_forwarded_to_backend_which_may_refuse() {
        let (srv, b) = start(TestBackend { fail_with: Some(io::ErrorKind::ConnectionRefused), ..Default::default() });
        let mut c = client(&srv);
        let mut a = vec![4];
        a.extend(Ipv6Addr::LOCALHOST.octets());
        a.extend(80u16.to_be_bytes());
        let (code, _) = request(&mut c, &srv, 1, &a);
        assert_ne!(code, 0);
        assert_eq!(*b.seen.lock().unwrap(), vec![Target::Addr("[::1]:80".parse().unwrap())]);
    }

    /// UDP echo server: replies to whoever sent each datagram.
    fn udp_echo() -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = s.local_addr().unwrap();
        thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while let Ok((n, from)) = s.recv_from(&mut buf) {
                let _ = s.send_to(&buf[..n], from);
            }
        });
        addr
    }

    /// Wraps a payload in the RFC 1928 UDP request header for an IPv4 destination.
    fn udp_wrap(dst: SocketAddr, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0, 0, 0];
        v.extend(v4(dst));
        v.extend(payload);
        v
    }

    /// Opens an association announcing `cu`'s port; returns the control connection and relay address.
    fn associate(srv: &SocksServer, cu: &UdpSocket) -> (TcpStream, SocketAddr) {
        let mut ctl = client(srv);
        let announced = v4(cu.local_addr().unwrap());
        let (code, relay) = request(&mut ctl, srv, 3, &announced);
        assert_eq!(code, 0);
        assert!(relay.ip().is_loopback());
        (ctl, relay)
    }

    #[test]
    fn udp_associate_relays_a_datagram_both_ways() {
        let echo = udp_echo();
        let (srv, _b) = start(TestBackend::default());
        let cu = UdpSocket::bind("127.0.0.1:0").unwrap();
        cu.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (_ctl, relay) = associate(&srv, &cu);
        cu.send_to(&udp_wrap(echo, b"ping"), relay).unwrap();
        let mut buf = [0u8; 2048];
        let (n, from) = cu.recv_from(&mut buf).unwrap();
        assert_eq!(from, relay);
        // Header: RSV RSV FRAG ATYP(1) addr port, then payload from the echo server.
        assert_eq!(&buf[..n], &udp_wrap(echo, b"ping")[..]);
    }

    #[test]
    fn udp_relay_ignores_other_senders() {
        let echo = udp_echo();
        let (srv, _b) = start(TestBackend::default());
        let cu = UdpSocket::bind("127.0.0.1:0").unwrap();
        cu.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (_ctl, relay) = associate(&srv, &cu);
        let rogue = UdpSocket::bind("127.0.0.1:0").unwrap();
        rogue.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        rogue.send_to(&udp_wrap(echo, b"rogue"), relay).unwrap();
        cu.send_to(&udp_wrap(echo, b"legit"), relay).unwrap();
        let mut buf = [0u8; 2048];
        let (n, _) = cu.recv_from(&mut buf).unwrap();
        assert!(buf[..n].ends_with(b"legit"));
        // The rogue sender gets nothing back either.
        assert!(rogue.recv_from(&mut buf).is_err());
    }

    #[test]
    fn udp_association_ends_when_control_connection_closes() {
        let echo = udp_echo();
        let (srv, _b) = start(TestBackend::default());
        let cu = UdpSocket::bind("127.0.0.1:0").unwrap();
        cu.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let (ctl, relay) = associate(&srv, &cu);
        drop(ctl);
        thread::sleep(Duration::from_millis(300));
        cu.send_to(&udp_wrap(echo, b"late"), relay).unwrap();
        let mut buf = [0u8; 2048];
        assert!(cu.recv_from(&mut buf).is_err(), "no reply once the control connection is gone");
    }

    #[test]
    fn each_server_gets_distinct_long_credentials() {
        let (a, _) = start(TestBackend::default());
        let (b, _) = start(TestBackend::default());
        let (ca, cb) = (a.credentials(), b.credentials());
        assert!(ca.username.len() >= 24 && ca.password.len() >= 24);
        assert_ne!(ca.password, cb.password);
        assert!(a.addr().ip().is_loopback());
    }

    #[test]
    fn dropping_the_server_stops_the_listener() {
        let (srv, _b) = start(TestBackend::default());
        let addr = srv.addr();
        drop(srv);
        assert!(TcpStream::connect(addr).is_err());
    }
}
