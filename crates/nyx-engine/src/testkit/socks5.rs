//! Minimal SOCKS5 server for tests (RFC 1928/1929: CONNECT and UDP ASSOCIATE). It counts what it
//! relays, so tests can prove traffic went through it and see where UDP datagrams were sent.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// What the UDP relay saw.
#[derive(Default)]
struct Udp {
    datagrams: AtomicUsize,
    dest_ports: std::sync::Mutex<Vec<u16>>,
    /// Local addresses of the relay sockets: packets arriving from one of these were relayed.
    relays: std::sync::Mutex<Vec<SocketAddr>>,
}

/// A running SOCKS5 stub. Dropping it stops the accept loop (the proxy "goes down").
pub struct Socks5Stub {
    addr: SocketAddr,
    connects: Arc<AtomicUsize>,
    /// Destination ports of every CONNECT seen, in order.
    ports: Arc<std::sync::Mutex<Vec<u16>>>,
    /// UDP relay state (UDP ASSOCIATE).
    udp: Arc<Udp>,
    stop: Arc<AtomicBool>,
}

impl Socks5Stub {
    /// Starts the stub on `bind`. With `creds` set, clients must authenticate with that
    /// user/password pair (method 0x02); otherwise it accepts "no auth".
    pub fn start(bind: SocketAddr, creds: Option<(String, String)>) -> Socks5Stub {
        let listener = TcpListener::bind(bind).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let connects = Arc::new(AtomicUsize::new(0));
        let ports = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let udp = Arc::new(Udp::default());
        let (c, p, s, u) = (connects.clone(), ports.clone(), stop.clone(), udp.clone());
        thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let (c, p, u, creds) = (c.clone(), p.clone(), u.clone(), creds.clone());
                        thread::spawn(move || {
                            let _ = serve(conn, creds, c, p, u);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
            // listener drops here, so new connections are refused
        });
        Socks5Stub { addr, connects, ports, udp, stop }
    }

    /// Address clients should use as the proxy.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// How many CONNECT requests have been served.
    pub fn connect_count(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }

    /// Whether any CONNECT targeted this destination port.
    pub fn saw_port(&self, port: u16) -> bool {
        self.ports.lock().unwrap().contains(&port)
    }

    /// How many UDP datagrams the relay forwarded to destinations.
    pub fn udp_count(&self) -> usize {
        self.udp.datagrams.load(Ordering::SeqCst)
    }

    /// Whether any relayed datagram was addressed to this destination port.
    pub fn saw_udp_port(&self, port: u16) -> bool {
        self.udp.dest_ports.lock().unwrap().contains(&port)
    }

    /// Local addresses of the UDP relay sockets. A destination that receives a datagram whose source
    /// is one of these got it through the proxy; any other source means a direct send.
    pub fn udp_relay_addrs(&self) -> Vec<SocketAddr> {
        self.udp.relays.lock().unwrap().clone()
    }

    /// Stops accepting; existing relays finish on their own.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // give the accept loop a tick to drop the listener
        thread::sleep(Duration::from_millis(100));
    }
}

impl Drop for Socks5Stub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Handles one client: greeting, optional user/pass auth, CONNECT, then relay both ways.
fn serve(
    mut c: TcpStream,
    creds: Option<(String, String)>,
    connects: Arc<AtomicUsize>,
    ports: Arc<std::sync::Mutex<Vec<u16>>>,
    udp: Arc<Udp>,
) -> std::io::Result<()> {
    c.set_nonblocking(false)?;
    let mut b = [0u8; 2];
    c.read_exact(&mut b)?;
    let mut methods = vec![0u8; b[1] as usize];
    c.read_exact(&mut methods)?;
    match &creds {
        None => c.write_all(&[5, 0])?,
        Some((user, pass)) => {
            if !methods.contains(&2) {
                c.write_all(&[5, 0xff])?;
                return Ok(());
            }
            c.write_all(&[5, 2])?;
            let mut h = [0u8; 2];
            c.read_exact(&mut h)?;
            let mut u = vec![0u8; h[1] as usize];
            c.read_exact(&mut u)?;
            let mut pl = [0u8; 1];
            c.read_exact(&mut pl)?;
            let mut p = vec![0u8; pl[0] as usize];
            c.read_exact(&mut p)?;
            let ok = u == user.as_bytes() && p == pass.as_bytes();
            c.write_all(&[1, if ok { 0 } else { 1 }])?;
            if !ok {
                return Ok(());
            }
        }
    }
    let mut req = [0u8; 4];
    c.read_exact(&mut req)?;
    let target = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            c.read_exact(&mut a)?;
            let mut p = [0u8; 2];
            c.read_exact(&mut p)?;
            SocketAddr::from((a, u16::from_be_bytes(p)))
        }
        3 => {
            let mut l = [0u8; 1];
            c.read_exact(&mut l)?;
            let mut name = vec![0u8; l[0] as usize];
            c.read_exact(&mut name)?;
            let mut p = [0u8; 2];
            c.read_exact(&mut p)?;
            let host = String::from_utf8_lossy(&name).into_owned();
            let port = u16::from_be_bytes(p);
            std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))?
                .next()
                .ok_or_else(|| std::io::Error::other("unresolvable"))?
        }
        _ => {
            c.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0])?;
            return Ok(());
        }
    };
    if req[1] == 3 {
        return udp_associate(c, udp);
    }
    if req[1] != 1 {
        c.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])?;
        return Ok(());
    }
    connects.fetch_add(1, Ordering::SeqCst);
    ports.lock().unwrap().push(target.port());
    let upstream = match TcpStream::connect_timeout(&target, Duration::from_secs(3)) {
        Ok(u) => u,
        Err(_) => {
            c.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0])?;
            return Ok(());
        }
    };
    c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])?;
    let (mut c2, mut u2) = (c.try_clone()?, upstream.try_clone()?);
    let t = thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut u2);
        let _ = u2.shutdown(Shutdown::Write);
    });
    let mut u3 = upstream;
    let _ = std::io::copy(&mut u3, &mut c);
    let _ = c.shutdown(Shutdown::Write);
    let _ = t.join();
    Ok(())
}

/// UDP ASSOCIATE: opens a relay socket, tells the client where it is, then forwards datagrams in
/// both directions until the TCP control connection closes.
fn udp_associate(mut c: TcpStream, udp: Arc<Udp>) -> std::io::Result<()> {
    let sock = UdpSocket::bind("127.0.0.1:0")?;
    let local = sock.local_addr()?;
    udp.relays.lock().unwrap().push(local);
    sock.set_read_timeout(Some(Duration::from_millis(100)))?;
    let port = local.port().to_be_bytes();
    c.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, port[0], port[1]])?;
    c.set_read_timeout(Some(Duration::from_millis(50)))?;
    let mut client: Option<SocketAddr> = None;
    let mut buf = vec![0u8; 65_536];
    let mut probe = [0u8; 1];
    loop {
        // The association ends when the TCP connection does.
        match c.read(&mut probe) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        if client.is_none() || client == Some(from) {
            // From the client: strip the SOCKS5 UDP header (RSV RSV FRAG ATYP ADDR PORT) and forward.
            client = Some(from);
            let d = &buf[..n];
            if n < 10 || d[2] != 0 {
                continue;
            }
            let (dest, off) = match d[3] {
                1 => (SocketAddr::from(([d[4], d[5], d[6], d[7]], u16::from_be_bytes([d[8], d[9]]))), 10),
                3 => {
                    let l = d[4] as usize;
                    if n < 5 + l + 2 {
                        continue;
                    }
                    let host = String::from_utf8_lossy(&d[5..5 + l]).into_owned();
                    let p = u16::from_be_bytes([d[5 + l], d[6 + l]]);
                    match std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), p)).ok().and_then(|mut i| i.next()) {
                        Some(a) => (a, 7 + l),
                        None => continue,
                    }
                }
                _ => continue,
            };
            udp.datagrams.fetch_add(1, Ordering::SeqCst);
            udp.dest_ports.lock().unwrap().push(dest.port());
            let _ = sock.send_to(&d[off..], dest);
        } else if let Some(cl) = client {
            // From a destination: wrap it in a SOCKS5 UDP header naming the sender and hand it back.
            if let SocketAddr::V4(v4) = from {
                let mut out = vec![0, 0, 0, 1];
                out.extend_from_slice(&v4.ip().octets());
                out.extend_from_slice(&v4.port().to_be_bytes());
                out.extend_from_slice(&buf[..n]);
                let _ = sock.send_to(&out, cl);
            }
        }
    }
    Ok(())
}
