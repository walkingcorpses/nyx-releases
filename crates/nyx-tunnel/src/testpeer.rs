// testpeer.rs (feature `test-support`): an in-process WireGuard peer for tests. It is the "VPN
// server" end: a second boringtun `Tunn` on a loopback UDP socket with its own smoltcp stack at
// 10.0.0.1, running a TCP echo (port 7), a UDP echo (port 7) and a DNS responder (port 53) that
// answers every A query with 10.0.0.1 and records the names asked. `client_config` hands out a
// matching client profile (tunnel address 10.0.0.2/32, DNS 10.0.0.1). Reuses the packet pump
// from tunnel.rs, so tests exercise the same code path as the real tunnel.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::socket::{tcp, udp};

use crate::conf::{Endpoint, WgConfig};
use crate::tunnel::{make_iface, make_tunn, new_tcp, spawn_engine, Engine, Link, Service, VirtDevice, Wire, WireStats};

/// The peer's address inside the tunnel (echo and DNS live here).
pub const PEER_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
/// The address `client_config` gives the client.
pub const CLIENT_IP: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 2);

/// A running test peer; dropping it stops its worker and closes its socket.
pub struct TestPeer {
    endpoint: SocketAddr,
    client: WgConfig,
    paused: Arc<AtomicBool>,
    cut: Arc<AtomicBool>,
    queries: Arc<Mutex<Vec<String>>>,
    link: Arc<Link<Infallible>>,
    worker: Option<JoinHandle<()>>,
}

impl TestPeer {
    /// Generates fresh keys for both ends, binds 127.0.0.1 on a free port and starts the peer.
    pub fn start() -> TestPeer {
        let server_key: [u8; 32] = rand::random();
        let client_key: [u8; 32] = rand::random();
        let server_pub = PublicKey::from(&StaticSecret::from(server_key)).to_bytes();
        let client_pub = PublicKey::from(&StaticSecret::from(client_key)).to_bytes();

        let sock = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind test peer socket");
        let endpoint = sock.local_addr().unwrap();
        let paused = Arc::new(AtomicBool::new(false));
        let cut = Arc::new(AtomicBool::new(false));
        let wire = Wire::new(make_tunn(server_key, client_pub, None, None), sock, false, None, paused.clone(), cut.clone());

        let mut dev = VirtDevice::default();
        let iface = make_iface(&mut dev, &[(IpAddr::V4(PEER_IP), 24)], false);
        let mut sockets = SocketSet::new(vec![]);
        let udp_sock = |sockets: &mut SocketSet<'static>, port| {
            let buf = || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 256], vec![0u8; 256 * 1024]);
            let mut s = udp::Socket::new(buf(), buf());
            s.bind(port).unwrap();
            sockets.add(s)
        };
        let udp_echo = udp_sock(&mut sockets, 7);
        let dns = udp_sock(&mut sockets, 53);
        let listener = new_listener(&mut sockets);
        let queries = Arc::new(Mutex::new(Vec::new()));
        let svc = PeerSvc { listener, echo: Vec::new(), udp_echo, dns, queries: queries.clone(), tmp: vec![0u8; 64 * 1024] };

        let engine = Engine { wire, dev, iface, sockets, svc, stats: Arc::new(WireStats::default()) };
        let (link, worker) = spawn_engine(engine, "testpeer").expect("start test peer");
        let client = WgConfig {
            private_key: client_key,
            addresses: vec![(IpAddr::V4(CLIENT_IP), 32)],
            dns: vec![IpAddr::V4(PEER_IP)],
            peer_public_key: server_pub,
            preshared_key: None,
            endpoint: Endpoint { host: endpoint.ip().to_string(), port: endpoint.port() },
            keepalive: None,
        };
        TestPeer { endpoint, client, paused, cut, queries, link, worker: Some(worker) }
    }

    /// The peer's UDP address on loopback (pass to `Tunnel::up`).
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    /// A client profile whose peer is this TestPeer.
    pub fn client_config(&self) -> WgConfig {
        self.client.clone()
    }

    /// While paused the peer drops every packet in both directions (simulates a dead VPN).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    /// One-way path failure: while set the peer drops everything the client sends but keeps
    /// sending the client a fresh handshake initiation about once a second. The client receives
    /// initiations whose responses never arrive, so no handshake ever completes.
    pub fn set_inbound_cut(&self, cut: bool) {
        self.cut.store(cut, Ordering::SeqCst);
    }

    /// Every name the DNS responder was asked for, in order.
    pub fn dns_queries(&self) -> Vec<String> {
        self.queries.lock().unwrap().clone()
    }
}

impl Drop for TestPeer {
    /// Stops the worker (which joins its reader) so the socket is closed.
    fn drop(&mut self) {
        self.link.stop();
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// Adds a TCP socket listening on port 7.
fn new_listener(sockets: &mut SocketSet<'static>) -> SocketHandle {
    let mut s = new_tcp();
    s.listen(7).unwrap();
    sockets.add(s)
}

/// The peer's services.
struct PeerSvc {
    /// smoltcp listeners accept one connection each, so a fresh one replaces it on every accept.
    listener: SocketHandle,
    /// Accepted echo connections with the bytes still to send back.
    echo: Vec<(SocketHandle, VecDeque<u8>)>,
    udp_echo: SocketHandle,
    dns: SocketHandle,
    queries: Arc<Mutex<Vec<String>>>,
    tmp: Vec<u8>,
}

impl Service for PeerSvc {
    type Cmd = Infallible;

    /// The peer takes no commands.
    fn command(&mut self, cmd: Infallible, _: &mut Interface, _: &mut SocketSet<'static>) {
        match cmd {}
    }

    /// Echoes TCP and UDP, answers DNS.
    fn service(&mut self, _: &mut Interface, sockets: &mut SocketSet<'static>) -> bool {
        let mut moved = false;
        if !sockets.get::<tcp::Socket>(self.listener).is_listening() {
            self.echo.push((self.listener, VecDeque::new()));
            self.listener = new_listener(sockets);
        }
        let tmp = &mut self.tmp;
        self.echo.retain_mut(|(h, pending)| {
            let s = sockets.get_mut::<tcp::Socket>(*h);
            // Always drain the receive buffer into `pending` so the peer's window never closes:
            // smoltcp 0.14 at zero window discards the other side's window updates when their
            // seq is past rcv_nxt (window-scale rounding), and two smoltcp ends at zero window
            // deadlock. shortcut: `pending` is unbounded, fine for a test fixture whose client reads.
            while let Ok(n) = s.recv_slice(tmp) {
                if n == 0 {
                    break;
                }
                pending.extend(&tmp[..n]);
                moved = true;
            }
            while !pending.is_empty() && s.can_send() {
                let (a, _) = pending.as_slices();
                let n = s.send_slice(a).unwrap_or(0);
                if n == 0 {
                    break;
                }
                pending.drain(..n);
                moved = true;
            }
            if s.state() == tcp::State::CloseWait && pending.is_empty() {
                s.close(); // client finished: close our side once everything is echoed
            }
            if matches!(s.state(), tcp::State::Closed | tcp::State::TimeWait) {
                sockets.remove(*h);
                return false;
            }
            true
        });
        let s = sockets.get_mut::<udp::Socket>(self.udp_echo);
        while let Ok((n, meta)) = s.recv_slice(tmp) {
            let _ = s.send_slice(&tmp[..n], meta.endpoint);
            moved = true;
        }
        let s = sockets.get_mut::<udp::Socket>(self.dns);
        while let Ok((n, meta)) = s.recv_slice(tmp) {
            if let Some((name, reply)) = dns_answer(&tmp[..n]) {
                self.queries.lock().unwrap().push(name);
                let _ = s.send_slice(&reply, meta.endpoint);
            }
            moved = true;
        }
        moved
    }

    /// Nothing waits on the peer.
    fn stop(&mut self, _: &mut SocketSet<'static>) {}
}

/// Parses a one-question DNS query and builds the reply: an A record for 10.0.0.1 when the
/// question is type A, otherwise an empty answer. Returns the queried name too.
fn dns_answer(q: &[u8]) -> Option<(String, Vec<u8>)> {
    if q.len() < 12 || u16::from_be_bytes([q[4], q[5]]) != 1 {
        return None;
    }
    let mut labels = Vec::new();
    let mut i = 12;
    loop {
        let l = *q.get(i)? as usize;
        i += 1;
        if l == 0 {
            break;
        }
        if l > 63 {
            return None;
        }
        labels.push(String::from_utf8_lossy(q.get(i..i + l)?).into_owned());
        i += l;
    }
    let question_end = i + 4;
    let qtype = u16::from_be_bytes(q.get(i..i + 2)?.try_into().ok()?);
    let is_a = qtype == 1;
    let mut r = Vec::with_capacity(question_end + 16);
    r.extend(&q[..2]); // transaction id
    r.extend([0x81, 0x80]); // response, recursion desired + available, no error
    r.extend([0, 1, 0, is_a as u8, 0, 0, 0, 0]); // 1 question, 0/1 answer
    r.extend(q.get(12..question_end)?);
    if is_a {
        // Name pointer to the question, type A, class IN, TTL 60, 4 bytes of address.
        r.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        r.extend(PEER_IP.octets());
    }
    Some((labels.join("."), r))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A query for echo.test gets one A record pointing at the peer.
    #[test]
    fn dns_answer_builds_an_a_record() {
        let mut q = vec![0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        q.extend(b"\x04echo\x04test\x00");
        q.extend([0, 1, 0, 1]);
        let (name, r) = dns_answer(&q).unwrap();
        assert_eq!(name, "echo.test");
        assert_eq!(&r[..2], &[0x12, 0x34]);
        assert_eq!(&r[6..8], &[0, 1]);
        assert!(r.ends_with(&[10, 0, 0, 1]));
    }
}
