//! Minimal SOCKS5 server for tests (RFC 1928/1929, CONNECT only). It counts the CONNECT
//! requests it receives and relays bytes, so tests can prove traffic went through it.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// A running SOCKS5 stub. Dropping it stops the accept loop (the proxy "goes down").
pub struct Socks5Stub {
    addr: SocketAddr,
    connects: Arc<AtomicUsize>,
    /// Destination ports of every CONNECT seen, in order.
    ports: Arc<std::sync::Mutex<Vec<u16>>>,
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
        let (c, p, s) = (connects.clone(), ports.clone(), stop.clone());
        thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let (c, p, creds) = (c.clone(), p.clone(), creds.clone());
                        thread::spawn(move || {
                            let _ = serve(conn, creds, c, p);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
            // listener drops here, so new connections are refused
        });
        Socks5Stub { addr, connects, ports, stop }
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
