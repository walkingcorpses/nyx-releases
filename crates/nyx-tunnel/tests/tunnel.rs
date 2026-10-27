// tests/tunnel.rs: end-to-end tests of the userspace WireGuard tunnel. Each test starts an
// in-process `TestPeer` (a second WireGuard endpoint with TCP/UDP echo and a DNS responder on
// 10.0.0.1), brings a `Tunnel` up against it on loopback, and talks raw SOCKS5 bytes to the
// tunnel's local port, exactly as libtorrent would.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use nyx_tunnel::testpeer::TestPeer;
use nyx_tunnel::tunnel::Tunnel;

/// The echo service inside the test peer.
const ECHO: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 7);

/// Starts a peer and a tunnel to it, and waits (bounded) for the first handshake.
fn up() -> (TestPeer, Tunnel) {
    let peer = TestPeer::start();
    let t = Tunnel::up(&peer.client_config(), peer.endpoint()).unwrap();
    wait_handshake(&t);
    (peer, t)
}

/// Polls the tunnel stats until a handshake is reported; the spec wants one within a second on
/// loopback, the 3 s bound only keeps a slow CI box from flaking.
fn wait_handshake(t: &Tunnel) {
    let end = Instant::now() + Duration::from_secs(3);
    while t.stats().handshake_age.is_none() {
        assert!(Instant::now() < end, "no WireGuard handshake within 3 s");
        thread::sleep(Duration::from_millis(10));
    }
}

/// ATYP + address + port bytes for an IP target.
fn addr_bytes(a: SocketAddr) -> Vec<u8> {
    let mut v = match a.ip() {
        IpAddr::V4(ip) => [vec![1], ip.octets().to_vec()].concat(),
        IpAddr::V6(ip) => [vec![4], ip.octets().to_vec()].concat(),
    };
    v.extend(a.port().to_be_bytes());
    v
}

/// ATYP + name + port bytes for a domain target.
fn domain_bytes(name: &str, port: u16) -> Vec<u8> {
    let mut v = vec![3, name.len() as u8];
    v.extend(name.as_bytes());
    v.extend(port.to_be_bytes());
    v
}

/// Logs in to the tunnel's SOCKS port and sends one request (`cmd` 1 = CONNECT, 3 = UDP
/// ASSOCIATE). Returns the control stream, the reply code and the bound address.
fn socks(t: &Tunnel, cmd: u8, target: &[u8]) -> (TcpStream, u8, SocketAddr) {
    let mut s = TcpStream::connect(t.socks_addr()).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    let c = t.credentials();
    s.write_all(&[5, 1, 2]).unwrap();
    let mut r = [0u8; 2];
    s.read_exact(&mut r).unwrap();
    assert_eq!(r, [5, 2]);
    let mut m = vec![1, c.username.len() as u8];
    m.extend(c.username.as_bytes());
    m.push(c.password.len() as u8);
    m.extend(c.password.as_bytes());
    s.write_all(&m).unwrap();
    s.read_exact(&mut r).unwrap();
    assert_eq!(r, [1, 0], "login refused");
    s.write_all(&[[5, cmd, 0].as_slice(), target].concat()).unwrap();
    let mut h = [0u8; 4];
    s.read_exact(&mut h).unwrap();
    assert_eq!(h[0], 5);
    assert_eq!(h[3], 1, "replies always carry an IPv4 bound address");
    let mut b = [0u8; 6];
    s.read_exact(&mut b).unwrap();
    let bound = SocketAddr::new(Ipv4Addr::new(b[0], b[1], b[2], b[3]).into(), u16::from_be_bytes([b[4], b[5]]));
    (s, h[1], bound)
}

/// Writes a message and expects the echo service to send it back unchanged.
fn assert_echoes(s: &mut TcpStream, msg: &[u8]) {
    s.write_all(msg).unwrap();
    let mut got = vec![0u8; msg.len()];
    s.read_exact(&mut got).unwrap();
    assert_eq!(got, msg);
}

#[test]
fn tcp_echo_through_socks_and_tunnel() {
    let (_peer, t) = up();
    let (mut s, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 0);
    assert_echoes(&mut s, b"hello through wireguard");
    // A second message on the same connection and a second connection both work.
    assert_echoes(&mut s, &vec![0x5a; 100_000]);
    let (mut s2, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 0);
    assert_echoes(&mut s2, b"second");
}

#[test]
fn domain_name_is_resolved_inside_the_tunnel() {
    let (peer, t) = up();
    let (mut s, code, _) = socks(&t, 1, &domain_bytes("echo.test", 7));
    assert_eq!(code, 0);
    assert_echoes(&mut s, b"resolved in the tunnel");
    assert!(peer.dns_queries().iter().any(|q| q == "echo.test"), "queries: {:?}", peer.dns_queries());
}

#[test]
fn domain_target_fails_without_dns() {
    let peer = TestPeer::start();
    let mut cfg = peer.client_config();
    cfg.dns.clear();
    let t = Tunnel::up(&cfg, peer.endpoint()).unwrap();
    wait_handshake(&t);
    let (_s, code, _) = socks(&t, 1, &domain_bytes("echo.test", 7));
    assert_ne!(code, 0);
    assert!(peer.dns_queries().is_empty());
}

#[test]
fn udp_associate_echo_through_tunnel() {
    let (_peer, t) = up();
    let cu = UdpSocket::bind("127.0.0.1:0").unwrap();
    cu.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let (_ctl, code, relay) = socks(&t, 3, &addr_bytes(cu.local_addr().unwrap()));
    assert_eq!(code, 0);
    let datagram = [[0u8, 0, 0].as_slice(), &addr_bytes(ECHO), b"ping"].concat();
    cu.send_to(&datagram, relay).unwrap();
    let mut buf = [0u8; 2048];
    let (n, from) = cu.recv_from(&mut buf).unwrap();
    assert_eq!(from, relay);
    // Reply header names the echo service as the sender, then the echoed payload.
    assert_eq!(&buf[..n], &datagram[..]);
}

#[test]
fn socks_refuses_before_first_handshake() {
    let peer = TestPeer::start();
    peer.set_paused(true);
    let t = Tunnel::up(&peer.client_config(), peer.endpoint()).unwrap();
    let (_s, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 3, "network unreachable before any handshake");
    let (_u, code, _) = socks(&t, 3, &addr_bytes("127.0.0.1:0".parse().unwrap()));
    assert_eq!(code, 3);
    assert!(t.stats().handshake_age.is_none());
}

#[test]
fn ipv6_target_is_refused_on_v4_only_tunnel() {
    let (_peer, t) = up();
    let v6 = SocketAddr::new(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1).into(), 7);
    let (_s, code, _) = socks(&t, 1, &addr_bytes(v6));
    assert_ne!(code, 0);
}

#[test]
fn stats_report_handshake_age_and_bytes() {
    let (_peer, t) = up();
    let before = t.stats();
    let (mut s, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 0);
    assert_echoes(&mut s, &vec![7u8; 50_000]);
    let st = t.stats();
    assert!(st.handshake_age.unwrap() < Duration::from_secs(10));
    assert!(st.tx_bytes >= before.tx_bytes + 50_000, "tx {} -> {}", before.tx_bytes, st.tx_bytes);
    assert!(st.rx_bytes >= before.rx_bytes + 50_000, "rx {} -> {}", before.rx_bytes, st.rx_bytes);
}

/// The tunnel's reported age (time since the last confirmed contact); panics before the first.
fn age(t: &Tunnel) -> Duration {
    t.stats().handshake_age.expect("tunnel has had a confirmed contact")
}

/// Samples the age every 100 ms for `d` and fails if it ever goes down: no confirmed contact may
/// happen in that time. Returns the last sample.
fn assert_age_only_grows(t: &Tunnel, d: Duration) -> Duration {
    let end = Instant::now() + d;
    let mut prev = age(t);
    while Instant::now() < end {
        thread::sleep(Duration::from_millis(100));
        let a = age(t);
        assert!(a >= prev, "age went back from {prev:?} to {a:?}: something counted as contact");
        prev = a;
    }
    prev
}

/// F1 (lab): a one-way path failure. The peer drops everything from the client but keeps sending
/// handshake initiations; the client answers each one, the answers never arrive. That is not a
/// live tunnel: the age must keep growing past the (shortened) stale threshold, and fall back to
/// small once the path is whole again and a real handshake completes.
#[test]
fn half_handshake_does_not_count_as_contact() {
    let peer = TestPeer::start();
    // Re-key at 2 s, so the stale threshold an app would use is 3 s (2/3 rule in netlock.rs).
    let t = Tunnel::up_with_rekey(&peer.client_config(), peer.endpoint(), Duration::from_secs(2)).unwrap();
    wait_handshake(&t);
    peer.set_inbound_cut(true);
    thread::sleep(Duration::from_millis(500)); // a re-key answered just before the cut may still land
    let last = assert_age_only_grows(&t, Duration::from_secs(7));
    assert!(last > Duration::from_secs(6), "age {last:?} should be well past the 3 s stale threshold");
    peer.set_inbound_cut(false);
    let end = Instant::now() + Duration::from_secs(10);
    while age(&t) > Duration::from_secs(1) {
        assert!(Instant::now() < end, "no fresh handshake within 10 s of the path coming back: age {:?}", age(&t));
        thread::sleep(Duration::from_millis(50));
    }
}

/// Initiations alone (no re-key of our own due, default 120 s re-key) never reset the age.
#[test]
fn initiation_alone_does_not_reset_age() {
    let (peer, t) = up();
    peer.set_inbound_cut(true);
    let last = assert_age_only_grows(&t, Duration::from_secs(4));
    assert!(last >= Duration::from_secs(4), "age {last:?}");
}

/// An authenticated transport packet from the peer is confirmed contact: echo data received long
/// after the handshake (no re-key due for 120 s) brings the age back to about zero.
#[test]
fn data_or_keepalive_counts_as_contact() {
    let (_peer, t) = up();
    let (mut s, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 0);
    thread::sleep(Duration::from_millis(1500));
    assert!(age(&t) >= Duration::from_millis(1400), "age {:?}", age(&t));
    assert_echoes(&mut s, b"proof of life");
    assert!(age(&t) < Duration::from_millis(500), "echo data did not count as contact: age {:?}", age(&t));
}

#[test]
fn blocked_tunnel_refuses_everything() {
    let t = Tunnel::blocked().unwrap();
    let (_s, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 3);
    let (_s, code, _) = socks(&t, 1, &domain_bytes("echo.test", 7));
    assert_eq!(code, 3);
    let (_u, code, _) = socks(&t, 3, &addr_bytes("127.0.0.1:0".parse().unwrap()));
    assert_eq!(code, 3);
    let st = t.stats();
    assert!(st.handshake_age.is_none());
    assert_eq!((st.rx_bytes, st.tx_bytes), (0, 0));
}

#[test]
fn shutdown_closes_the_socks_port() {
    let (_peer, t) = up();
    let addr = t.socks_addr();
    let (mut s, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 0);
    assert_echoes(&mut s, b"before shutdown");
    let started = Instant::now();
    drop(t);
    assert!(started.elapsed() < Duration::from_secs(5), "drop took {:?}", started.elapsed());
    assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_err(), "SOCKS port still accepts");
    // The proxied connection ends too: EOF or an error, never a hang.
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut buf = [0u8; 16];
    match s.read(&mut buf) {
        Ok(0) | Err(_) => {}
        Ok(n) => panic!("read {n} bytes after shutdown"),
    }
}

#[test]
#[ignore = "throughput benchmark; run with --ignored"]
fn throughput_through_tunnel_on_loopback() {
    const TOTAL: usize = 200 * 1024 * 1024;
    let (_peer, t) = up();
    let (s, code, _) = socks(&t, 1, &addr_bytes(ECHO));
    assert_eq!(code, 0);
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut w = s.try_clone().unwrap();
    let start = Instant::now();
    let writer = thread::spawn(move || {
        let chunk = vec![0xa5u8; 64 * 1024];
        let mut sent = 0;
        while sent < TOTAL {
            let n = chunk.len().min(TOTAL - sent);
            w.write_all(&chunk[..n]).unwrap();
            sent += n;
        }
    });
    let mut r = s;
    let mut buf = vec![0u8; 256 * 1024];
    let mut got = 0;
    while got < TOTAL {
        let n = r.read(&mut buf).unwrap();
        assert!(n > 0, "echo ended early after {got} bytes");
        got += n;
    }
    let secs = start.elapsed().as_secs_f64();
    writer.join().unwrap();
    let rate = TOTAL as f64 / secs / 1_000_000.0;
    println!("throughput: {} MB echoed in {secs:.2} s = {rate:.1} MB/s", TOTAL / 1_000_000);
    assert!(rate >= 10.0, "tunnel throughput {rate:.1} MB/s is under the 10 MB/s minimum");
}
