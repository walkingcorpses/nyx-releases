// tunnel.rs: the userspace WireGuard tunnel. One worker thread owns the boringtun `Tunn`, the single
// UDP socket connected to the VPN endpoint and a smoltcp TCP/IP stack (Medium::Ip) whose packets are
// encrypted by `Tunn`. A local SOCKS5 server (socks.rs) is the only entrance: its `Backend` here turns
// CONNECT, UDP ASSOCIATE and domain names into smoltcp sockets inside the tunnel. No admin rights,
// no kernel driver.
//
// Threads: `nyx-wg` (worker, owns all protocol state), `nyx-wg-rx` (blocks on the UDP socket and
// forwards datagrams to the worker), plus the SOCKS threads. SOCKS threads exchange bytes with the
// worker through per-connection buffers guarded by a mutex + condvar, and wake the worker through
// its bounded event channel (coalesced by an atomic flag), so nothing polls on a short timer.
//
// Privacy rules: key material is never logged or printed; names are only resolved by a smoltcp DNS
// socket talking to the profile's DNS servers through the tunnel; the only socket that reaches the
// outside world is the UDP socket connected to the endpoint; a target of an IP family the tunnel has
// no address for is refused, never routed elsewhere.
//
// The packet pump (`VirtDevice`, `Wire`, `Link`, `spawn_engine`) is shared with testpeer.rs.

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use boringtun::noise::errors::WireGuardError;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::{dns, tcp, udp};
use smoltcp::wire::{DnsQueryType, HardwareAddress, IpAddress, IpCidr};

use crate::conf::WgConfig;
use crate::socks::{Backend, Credentials, SocksServer, Stream, Target, UdpPort};

/// Inner MTU of the tunnel (WireGuard's usual value: 1500 minus IPv6 + UDP + WireGuard overhead).
pub(crate) const MTU: usize = 1420;
/// How often boringtun's timers run (retransmits, keepalives, rekeys).
const TICK: Duration = Duration::from_millis(250);
/// Re-key once our last completed handshake is this old, so a healthy tunnel never looks stale to
/// the lock (spec 4.2).
const REKEY_AGE: Duration = Duration::from_secs(120);
/// Bounded event queue; a full queue makes the UDP reader wait, which is the backpressure we want.
const EVENT_QUEUE: usize = 1024;
/// Most extra events handled per worker pass before pumping the stack.
const EVENT_BATCH: usize = 256;
/// Upper bound on stack/socket pump rounds per pass, so one pass cannot run forever.
const PUMP_ROUNDS: usize = 32;
/// smoltcp TCP socket buffer size per direction (sets the TCP window, hence throughput).
// shortcut: smoltcp 0.14 zero-window ACK rejection. Buffers over 64 KiB use window scaling; the
// right window edge can then shrink by rounding, and at zero window smoltcp rejects ACKs whose
// seq is not exactly rcv_nxt, so two smoltcp ends at zero window can deadlock (seen with the
// TestPeer, see testpeer.rs). Guarded by `slow_reader_does_not_stall_the_tunnel`; if it ever
// stalls, fall back to buffers <= 64 KiB (no window scaling) and re-check the 10 MB/s throughput.
const TCP_BUF: usize = 128 * 1024;
/// Concurrent tunnel TCP connections; further CONNECTs are refused (ConnectionRefused, SOCKS
/// reply 5). Bounds memory at about 256 x (2 x TCP_BUF + 2 x APP_BUF).
const MAX_TCP_CONNS: usize = 256;
/// Random picks `alloc_port` tries before giving up.
const PORT_TRIES: usize = 64;
/// Buffer between a SOCKS thread and the worker, per direction per connection.
const APP_BUF: usize = 256 * 1024;
/// smoltcp UDP socket: payload bytes and datagram slots per direction.
const UDP_BUF: usize = 256 * 1024;
const UDP_SLOTS: usize = 256;
/// Datagrams kept for a SOCKS UDP association that is not reading; extra ones are dropped.
const UDP_QUEUE: usize = 1024;
/// How long CONNECT waits for the remote end (SOCKS reply 4 after that).
const CONNECT_WAIT: Duration = Duration::from_secs(30);
/// smoltcp gives up on a connection whose data stays unacknowledged this long.
const TCP_TIMEOUT: u64 = 60;
/// How long a SOCKS thread waits for a DNS answer from inside the tunnel.
const DNS_WAIT: Duration = Duration::from_secs(15);
/// Concurrent DNS lookups allowed; more fail at once instead of queueing without bound.
const DNS_PENDING_MAX: usize = 64;

// ---------------------------------------------------------------------------------------------
// Shared packet pump (also used by testpeer.rs)
// ---------------------------------------------------------------------------------------------

/// smoltcp device backed by two packet queues: `rx` holds decrypted packets from WireGuard, `tx`
/// collects packets smoltcp wants to send, which the worker encrypts right after each poll.
#[derive(Default)]
pub(crate) struct VirtDevice {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}

/// One received packet handed to smoltcp.
pub(crate) struct VRx(Vec<u8>);
/// Permission to append one packet to the device's transmit queue.
pub(crate) struct VTx<'a>(&'a mut VecDeque<Vec<u8>>);

impl RxToken for VRx {
    /// Lets smoltcp read the packet in place.
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for VTx<'_> {
    /// Lets smoltcp fill a fresh packet of `len` bytes, then queues it for encryption.
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut p = vec![0u8; len];
        let r = f(&mut p);
        self.0.push_back(p);
        r
    }
}

impl Device for VirtDevice {
    type RxToken<'a> = VRx;
    type TxToken<'a> = VTx<'a>;

    /// Hands smoltcp the next decrypted packet, if any.
    fn receive(&mut self, _: smoltcp::time::Instant) -> Option<(VRx, VTx<'_>)> {
        let p = self.rx.pop_front()?;
        Some((VRx(p), VTx(&mut self.tx)))
    }

    /// Transmitting never blocks: packets queue until the worker encrypts them.
    fn transmit(&mut self, _: smoltcp::time::Instant) -> Option<VTx<'_>> {
        Some(VTx(&mut self.tx))
    }

    /// Raw IP packets (no Ethernet), tunnel MTU.
    fn capabilities(&self) -> DeviceCapabilities {
        let mut c = DeviceCapabilities::default();
        c.medium = Medium::Ip;
        c.max_transmission_unit = MTU;
        c
    }
}

/// Builds a smoltcp interface with the given addresses. With `default_routes` every off-link
/// destination goes out of the (only) interface, which is the tunnel.
pub(crate) fn make_iface(dev: &mut VirtDevice, addrs: &[(IpAddr, u8)], default_routes: bool) -> Interface {
    let mut c = Config::new(HardwareAddress::Ip);
    c.random_seed = rand::random();
    let mut iface = Interface::new(c, dev, smoltcp::time::Instant::now());
    iface.update_ip_addrs(|a| {
        for (ip, prefix) in addrs {
            let _ = a.push(IpCidr::new(IpAddress::from(*ip), *prefix));
        }
    });
    if default_routes {
        // With Medium::Ip the gateway is never looked up; our own address is a valid placeholder.
        for (ip, _) in addrs {
            let _ = match ip {
                IpAddr::V4(v4) => iface.routes_mut().add_default_ipv4_route(*v4).map(|_| ()),
                IpAddr::V6(v6) => iface.routes_mut().add_default_ipv6_route(*v6).map(|_| ()),
            };
        }
    }
    iface
}

/// A TCP socket tuned for bulk transfer: `TCP_BUF` windows, no Nagle, an ACK for every segment
/// so the sender's window keeps moving.
pub(crate) fn new_tcp() -> tcp::Socket<'static> {
    let buf = || tcp::SocketBuffer::new(vec![0u8; TCP_BUF]);
    let mut s = tcp::Socket::new(buf(), buf());
    s.set_nagle_enabled(false);
    s.set_ack_delay(None);
    s
}

/// Last-contact time and byte counters published by the worker for `Tunnel::stats`.
#[derive(Default)]
pub(crate) struct WireStats {
    /// When the peer last gave confirmed contact (see `Wire::handshake_at`). Only moves forward.
    contact_at: Mutex<Option<Instant>>,
    rx: AtomicU64,
    tx: AtomicU64,
    /// The worker thread has ended (stopped or panicked): the tunnel is down for good.
    dead: AtomicBool,
}

impl WireStats {
    /// True once the first confirmed contact (always a completed handshake) happened and the
    /// worker is still running; otherwise the backend refuses everything.
    fn handshaked(&self) -> bool {
        !self.dead.load(Ordering::SeqCst) && self.contact_at.lock().unwrap().is_some()
    }
}

/// Runs when the worker thread ends, also by panic (it lives on the worker's stack): marks the
/// tunnel dead so stats and the backend report it down, and stops the UDP reader thread, so a
/// dead worker never leaves a tunnel that looks alive or a socket that stays open.
struct WorkerExit {
    stats: Arc<WireStats>,
    reader_stop: Arc<AtomicBool>,
}

impl Drop for WorkerExit {
    /// Sets the dead flag and the reader's stop flag (the reader exits within its read timeout).
    fn drop(&mut self) {
        self.stats.dead.store(true, Ordering::SeqCst);
        self.reader_stop.store(true, Ordering::SeqCst);
    }
}

/// The WireGuard side of an endpoint: the `Tunn` and its UDP socket.
pub(crate) struct Wire {
    tunn: Tunn,
    sock: UdpSocket,
    /// Client: the socket is connected to the endpoint (use `send`). Test peer: unconnected, the
    /// destination is learned from the last valid datagram.
    connected: bool,
    dest: Option<SocketAddr>,
    /// The client initiates (and re-initiates) handshakes, re-keying once the handshake is this
    /// old; `None` for the test peer, which only responds.
    rekey: Option<Duration>,
    /// Test hook: drop every packet in both directions while set.
    paused: Arc<AtomicBool>,
    /// Test hook (one-way path failure): drop every inbound datagram while set, and keep sending
    /// our own handshake initiations about once a second, like a server whose replies never arrive.
    cut: Arc<AtomicBool>,
    /// When the `cut` hook last forced an initiation.
    forced_at: Option<Instant>,
    /// Confirmed contact, what the network lock calls "alive": when a handshake WE initiated last
    /// completed (the peer accepted our initiation and its response reached us), the only packet
    /// that proves both directions work right now. Not contact (lab finding F1):
    /// - a received initiation or cookie reply: our answer may never arrive, yet boringtun's own
    ///   `time_since_last_handshake` resets on one;
    /// - received transport data or keepalives: the peer keeps sending on its old keypair (up to
    ///   180 s) while our packets are dropped, e.g. TCP retransmits from the far end.
    ///
    /// Also drives the client's re-key.
    handshake_at: Option<Instant>,
    buf: Vec<u8>,
}

/// Sends one encrypted datagram unless paused. Errors are ignored: UDP is lossy anyway and
/// WireGuard/TCP retransmit.
fn send_net(sock: &UdpSocket, connected: bool, dest: Option<SocketAddr>, paused: &AtomicBool, p: &[u8]) {
    if paused.load(Ordering::Relaxed) {
        return;
    }
    let _ = match (connected, dest) {
        (true, _) => sock.send(p),
        (false, Some(d)) => sock.send_to(p, d),
        (false, None) => return,
    };
}

/// Builds a `Tunn` from raw keys. `keepalive` 0 means disabled (boringtun wants `None`).
pub(crate) fn make_tunn(private: [u8; 32], peer_public: [u8; 32], psk: Option<[u8; 32]>, keepalive: Option<u16>) -> Tunn {
    let index = rand::random::<u32>() >> 8; // boringtun shifts the index left by 8
    Tunn::new(
        StaticSecret::from(private),
        PublicKey::from(peer_public),
        psk,
        keepalive.filter(|k| *k != 0),
        index,
        None,
    )
}

impl Wire {
    /// Wraps a `Tunn` and its socket; see the field docs for `connected` and `rekey`.
    pub(crate) fn new(tunn: Tunn, sock: UdpSocket, connected: bool, rekey: Option<Duration>, paused: Arc<AtomicBool>, cut: Arc<AtomicBool>) -> Wire {
        Wire { tunn, sock, connected, dest: None, rekey, paused, cut, forced_at: None, handshake_at: None, buf: vec![0u8; 65536] }
    }

    /// Sends a handshake initiation now (the client calls this at start so the tunnel is up within
    /// one round trip instead of waiting for the first packet or timer).
    fn initiate(&mut self) {
        let Wire { tunn, sock, connected, dest, paused, buf, .. } = self;
        if let TunnResult::WriteToNetwork(p) = tunn.format_handshake_initiation(buf, false) {
            send_net(sock, *connected, *dest, paused, p);
        }
    }

    /// Decrypts one datagram from the network. Handshake replies go straight back out; data
    /// packets are queued on the device for smoltcp.
    fn input(&mut self, d: &[u8], from: SocketAddr, dev: &mut VirtDevice) {
        if self.paused.load(Ordering::Relaxed) || self.cut.load(Ordering::Relaxed) {
            return;
        }
        let Wire { tunn, sock, connected, dest, paused, buf, handshake_at, .. } = self;
        let r = tunn.decapsulate(Some(from.ip()), d, buf);
        if matches!(r, TunnResult::Err(_)) {
            return;
        }
        if completes_our_handshake(d, &r) {
            *handshake_at = Some(Instant::now());
        }
        if !*connected {
            *dest = Some(from);
        }
        match r {
            TunnResult::WriteToNetwork(p) => {
                send_net(sock, *connected, *dest, paused, p);
                // boringtun contract: repeat with an empty datagram to flush packets queued
                // behind the handshake.
                while let TunnResult::WriteToNetwork(p) = tunn.decapsulate(None, &[], buf) {
                    send_net(sock, *connected, *dest, paused, p);
                }
            }
            TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => dev.rx.push_back(p.to_vec()),
            TunnResult::Done | TunnResult::Err(_) => {}
        }
    }

    /// Encrypts and sends everything smoltcp queued. Without a session boringtun queues the
    /// packet and returns a handshake initiation instead.
    fn output(&mut self, dev: &mut VirtDevice) {
        let Wire { tunn, sock, connected, dest, paused, buf, .. } = self;
        while let Some(p) = dev.tx.pop_front() {
            if let TunnResult::WriteToNetwork(out) = tunn.encapsulate(&p, buf) {
                send_net(sock, *connected, *dest, paused, out);
            }
        }
    }

    /// Runs boringtun's timers. The client also restarts handshakes after boringtun gives up
    /// (so a VPN that was down comes back by itself) and re-keys at its `rekey` age even when idle.
    ///
    /// The re-key age counts from our own last completed handshake (`handshake_at`), never from
    /// boringtun's session age: an initiation received from the peer resets that age and also
    /// cancels our in-flight initiation (boringtun moves it to its `previous` slot), so under a
    /// one-way failure we would otherwise stop asking for a handshake. Contact on every healthy
    /// tunnel, idle or busy, comes only from this re-key (data does not count, see `handshake_at`;
    /// the client's persistent keepalive gets no reply anyway, a WireGuard peer answers data, not
    /// keepalives): the completed handshake every `rekey` (120 s in production, 2/3 of the lock's
    /// 180 s stale threshold; boringtun retries a lost one every 5 s) keeps the contact age under
    /// the threshold. Before the first completed handshake a new one is due whenever none is in
    /// flight.
    fn tick(&mut self) {
        self.force_initiation_if_cut();
        let Wire { tunn, sock, connected, dest, paused, buf, rekey, handshake_at, .. } = self;
        match tunn.update_timers(buf) {
            TunnResult::WriteToNetwork(p) => send_net(sock, *connected, *dest, paused, p),
            TunnResult::Err(WireGuardError::ConnectionExpired) if rekey.is_some() => {
                if let TunnResult::WriteToNetwork(p) = tunn.format_handshake_initiation(buf, false) {
                    send_net(sock, *connected, *dest, paused, p);
                }
            }
            _ => {
                // Re-key check only in this arm: when a timer packet or a restart was sent this
                // tick, the check waits for the next tick (<= one 250 ms TICK of delay), harmless.
                if rekey.is_some_and(|r| handshake_at.is_none_or(|t| t.elapsed() >= r)) {
                    if let TunnResult::WriteToNetwork(p) = tunn.format_handshake_initiation(buf, false) {
                        send_net(sock, *connected, *dest, paused, p);
                    }
                }
            }
        }
    }

    /// Test hook: while `cut` is set, sends a fresh handshake initiation about once a second
    /// (forced, even with one in flight), so the other end keeps RECEIVING initiations that it
    /// can never complete.
    fn force_initiation_if_cut(&mut self) {
        if !self.cut.load(Ordering::Relaxed) || self.forced_at.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.forced_at = Some(Instant::now());
        let Wire { tunn, sock, connected, dest, paused, buf, .. } = self;
        if let TunnResult::WriteToNetwork(p) = tunn.format_handshake_initiation(buf, true) {
            send_net(sock, *connected, *dest, paused, p);
        }
    }

    /// Copies the confirmed-contact time and byte counters into the shared stats.
    fn publish(&self, stats: &WireStats) {
        let (_, tx, rx, _, _) = self.tunn.stats();
        if self.handshake_at.is_some() {
            *stats.contact_at.lock().unwrap() = self.handshake_at;
        }
        stats.tx.store(tx as u64, Ordering::Relaxed);
        stats.rx.store(rx as u64, Ordering::Relaxed);
    }
}

/// True when datagram `d` was a handshake response (WireGuard message type 2, little-endian u32
/// at offset 0) to our initiation that boringtun accepted. Accepted means boringtun answers with
/// its confirmation keepalive (a type-4 packet); under load it may instead return a cookie reply
/// (type 3) without processing the response at all.
fn completes_our_handshake(d: &[u8], r: &TunnResult) -> bool {
    let kind = |p: &[u8]| p.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    matches!(r, TunnResult::WriteToNetwork(p) if kind(d) == Some(2) && kind(p) == Some(4))
}

/// Messages to the worker.
pub(crate) enum Event<C> {
    /// A datagram read from the UDP socket, with its sender.
    Packet(Vec<u8>, SocketAddr),
    /// Some application buffer changed; run a pass.
    Wake,
    Cmd(C),
    Stop,
}

/// Sender side of the worker's event queue, with a coalescing wake flag so a busy SOCKS thread
/// does not flood the queue with one wake per write.
pub(crate) struct Link<C> {
    pending: AtomicBool,
    tx: SyncSender<Event<C>>,
}

impl<C> Link<C> {
    /// Asks the worker for a pass. If the queue is full the worker is about to run anyway.
    fn wake(&self) {
        if !self.pending.swap(true, Ordering::SeqCst) {
            let _ = self.tx.try_send(Event::Wake);
        }
    }

    /// Sends a command; fails with NetworkUnreachable once the worker has stopped.
    fn send(&self, c: C) -> io::Result<()> {
        self.tx.send(Event::Cmd(c)).map_err(|_| io::Error::new(io::ErrorKind::NetworkUnreachable, "tunnel is down"))
    }

    /// Tells the worker to stop (no-op if it already has).
    pub(crate) fn stop(&self) {
        let _ = self.tx.send(Event::Stop);
    }
}

/// What a worker does with its smoltcp sockets: the client tunnel serves SOCKS connections, the
/// test peer runs echo and DNS services.
pub(crate) trait Service: Send + 'static {
    type Cmd: Send + 'static;
    /// Handles one command from the application side.
    fn command(&mut self, cmd: Self::Cmd, iface: &mut Interface, sockets: &mut SocketSet<'static>);
    /// Moves data between application buffers and smoltcp sockets; true if anything moved.
    fn service(&mut self, iface: &mut Interface, sockets: &mut SocketSet<'static>) -> bool;
    /// The worker is stopping: release everyone waiting on it.
    fn stop(&mut self, sockets: &mut SocketSet<'static>);
}

/// Everything a worker owns.
pub(crate) struct Engine<S> {
    pub(crate) wire: Wire,
    pub(crate) dev: VirtDevice,
    pub(crate) iface: Interface,
    pub(crate) sockets: SocketSet<'static>,
    pub(crate) svc: S,
    pub(crate) stats: Arc<WireStats>,
}

/// A started worker: the link to it and its thread handle.
pub(crate) type Running<C> = (Arc<Link<C>>, JoinHandle<()>);

/// Starts the worker and its UDP reader thread. Returns the link to the worker and the worker's
/// handle; joining it also joins the reader, so the UDP socket is closed once it returns.
pub(crate) fn spawn_engine<S: Service>(e: Engine<S>, name: &str) -> io::Result<Running<S::Cmd>> {
    let (tx, rx) = mpsc::sync_channel(EVENT_QUEUE);
    let link = Arc::new(Link { pending: AtomicBool::new(false), tx });
    let rsock = e.wire.sock.try_clone()?;
    // Short read timeout only so the reader notices shutdown; data never waits on it.
    rsock.set_read_timeout(Some(Duration::from_millis(100)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let reader = thread::Builder::new().name(format!("{name}-rx")).spawn({
        let (tx, stop) = (link.tx.clone(), stop.clone());
        move || reader_loop(rsock, tx, stop)
    })?;
    let wl = link.clone();
    let exit = WorkerExit { stats: e.stats.clone(), reader_stop: stop.clone() };
    let worker = thread::Builder::new().name(name.to_string()).spawn(move || {
        let guard = exit; // dropped on return AND on panic: dead flag + reader stop
        run(e, rx, &wl);
        drop(guard);
        let _ = reader.join();
    });
    if worker.is_err() {
        stop.store(true, Ordering::SeqCst); // no worker: let the reader exit and close the socket
    }
    Ok((link, worker?))
}

/// Blocks on the UDP socket and forwards each datagram to the worker until told to stop.
fn reader_loop<C>(sock: UdpSocket, tx: SyncSender<Event<C>>, stop: Arc<AtomicBool>) {
    let mut buf = vec![0u8; 65536];
    while !stop.load(Ordering::SeqCst) {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if tx.send(Event::Packet(buf[..n].to_vec(), from)).is_err() {
                    return; // worker gone
                }
            }
            // Timeouts, and Windows' ICMP-unreachable-as-reset, are not fatal.
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset) => {}
            Err(_) => thread::sleep(Duration::from_millis(20)), // unexpected: avoid a busy loop
        }
    }
}

/// smoltcp's clock from std's.
fn sm_now() -> smoltcp::time::Instant {
    smoltcp::time::Instant::now()
}

/// The worker loop: wait for an event, the next timer tick or smoltcp's own deadline; handle a
/// batch of events; pump sockets <-> stack <-> WireGuard until quiet; run timers; publish stats.
fn run<S: Service>(mut e: Engine<S>, rx: Receiver<Event<S::Cmd>>, link: &Link<S::Cmd>) {
    e.wire.initiate();
    let mut next_tick = Instant::now() + TICK;
    'outer: loop {
        let mut wait = next_tick.saturating_duration_since(Instant::now());
        if let Some(d) = e.iface.poll_delay(sm_now(), &e.sockets) {
            wait = wait.min(Duration::from_micros(d.total_micros()));
        }
        let first = match rx.recv_timeout(wait) {
            Ok(ev) => Some(ev),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        // Clear before servicing: a wake raised after this point triggers another pass.
        link.pending.store(false, Ordering::SeqCst);
        for ev in first.into_iter().chain(std::iter::from_fn(|| rx.try_recv().ok()).take(EVENT_BATCH)) {
            match ev {
                Event::Packet(d, from) => e.wire.input(&d, from, &mut e.dev),
                Event::Wake => {}
                Event::Cmd(c) => e.svc.command(c, &mut e.iface, &mut e.sockets),
                Event::Stop => break 'outer,
            }
        }
        for _ in 0..PUMP_ROUNDS {
            let moved = e.svc.service(&mut e.iface, &mut e.sockets);
            let changed = e.iface.poll(sm_now(), &mut e.dev, &mut e.sockets);
            e.wire.output(&mut e.dev);
            if !moved && matches!(changed, PollResult::None) {
                break;
            }
        }
        if Instant::now() >= next_tick {
            e.wire.tick();
            next_tick = Instant::now() + TICK;
        }
        e.wire.publish(&e.stats);
    }
    e.svc.stop(&mut e.sockets);
}

// ---------------------------------------------------------------------------------------------
// Client tunnel: SOCKS backend
// ---------------------------------------------------------------------------------------------

/// Per-connection buffers shared by the SOCKS threads and the worker.
#[derive(Default)]
struct Conn {
    m: Mutex<ConnBuf>,
    cv: Condvar,
}

#[derive(Default)]
struct ConnBuf {
    /// Remote -> app bytes not yet read.
    rx: VecDeque<u8>,
    /// App -> remote bytes not yet handed to smoltcp.
    tx: VecDeque<u8>,
    /// The TCP handshake completed.
    open: bool,
    /// The connection attempt failed with this kind.
    failed: Option<io::ErrorKind>,
    /// No more bytes will arrive (remote closed, reset or tunnel stopped).
    eof: bool,
    /// No more bytes can be sent.
    broken: bool,
    /// The app called `shutdown`: reads end, the worker flushes `tx` then closes.
    closed: bool,
}

/// Commands from SOCKS threads to the worker.
enum Cmd {
    Connect(SocketAddr, Arc<Conn>),
    Resolve(String, SyncSender<io::Result<IpAddr>>),
    UdpOpen(Arc<UdpConn>),
    UdpSend(Arc<UdpConn>, Vec<u8>, SocketAddr),
    /// Test hook: makes the worker panic, to prove a dead worker reports the tunnel down.
    #[cfg(feature = "test-support")]
    Panic,
}

/// Datagrams received for one UDP association, waiting for the SOCKS relay.
#[derive(Default)]
struct UdpConn {
    m: Mutex<UdpBuf>,
    cv: Condvar,
}

#[derive(Default)]
struct UdpBuf {
    rx: VecDeque<(Vec<u8>, SocketAddr)>,
    /// The worker has stopped.
    dead: bool,
}

/// A TCP connection as the worker sees it.
struct TcpEntry {
    h: SocketHandle,
    port: u16,
    conn: Arc<Conn>,
    /// `close()` was already called on the smoltcp socket.
    closing: bool,
}

/// A UDP association as the worker sees it.
struct UdpEntry {
    h: SocketHandle,
    port: u16,
    conn: Arc<UdpConn>,
}

/// The client worker's service: SOCKS connections, UDP associations and DNS lookups.
struct ClientSvc {
    tcp: Vec<TcpEntry>,
    udp: Vec<UdpEntry>,
    dns: Option<SocketHandle>,
    qtype: DnsQueryType,
    pending_dns: Vec<(dns::QueryHandle, SyncSender<io::Result<IpAddr>>)>,
    /// Local ports in use by our TCP and UDP sockets.
    ports: HashSet<u16>,
    /// Concurrent TCP connections allowed (`MAX_TCP_CONNS`; tests lower it).
    max_tcp: usize,
}

/// Picks a free ephemeral port for a new smoltcp socket, or `None` after `PORT_TRIES` random
/// picks all hit a used port (bounded, so a full port range can never hang the worker). With at
/// most `MAX_TCP_CONNS` TCP sockets plus the UDP associations in 16384 ports, a miss is rare.
fn alloc_port(ports: &mut HashSet<u16>) -> Option<u16> {
    (0..PORT_TRIES).map(|_| rand::random_range(49152..=65535u16)).find(|p| ports.insert(*p))
}

/// Moves up to `dst.len()` bytes from the front of `q` into `dst`; returns how many.
fn drain_into(q: &mut VecDeque<u8>, dst: &mut [u8]) -> usize {
    let n = dst.len().min(q.len());
    let (a, b) = q.as_slices();
    let na = n.min(a.len());
    dst[..na].copy_from_slice(&a[..na]);
    dst[na..n].copy_from_slice(&b[..n - na]);
    q.drain(..n);
    n
}

/// Services one TCP connection; returns (bytes or state moved, connection finished).
fn pump_tcp(e: &mut TcpEntry, s: &mut tcp::Socket) -> (bool, bool) {
    let abandoned = Arc::strong_count(&e.conn) == 1; // every app handle dropped
    let mut b = e.conn.m.lock().unwrap();
    let mut moved = false;
    if !b.open {
        if b.failed.is_some() || b.closed || abandoned {
            s.abort();
            return (true, true);
        }
        if s.may_send() {
            b.open = true;
            moved = true;
        } else if s.state() == tcp::State::Closed {
            b.failed = Some(io::ErrorKind::ConnectionRefused);
            e.conn.cv.notify_all();
            return (true, true);
        } else {
            return (false, false);
        }
    }
    // App -> network.
    while !b.tx.is_empty() && s.can_send() {
        let n = s.send(|dst| {
            let n = drain_into(&mut b.tx, dst);
            (n, n)
        });
        match n {
            Ok(n) if n > 0 => moved = true,
            _ => break,
        }
    }
    // Network -> app. After `shutdown` nobody reads, so incoming data is discarded.
    let gone = b.closed || abandoned;
    while s.can_recv() && (gone || b.rx.len() < APP_BUF) {
        let n = s.recv(|src| {
            let n = if gone { src.len() } else { src.len().min(APP_BUF - b.rx.len()) };
            if !gone {
                b.rx.extend(&src[..n]);
            }
            (n, n)
        });
        match n {
            Ok(n) if n > 0 => moved = true,
            _ => break,
        }
    }
    if !b.eof && !s.may_recv() && s.recv_queue() == 0 {
        b.eof = true;
        moved = true;
    }
    if !b.broken && !s.may_send() && !e.closing {
        b.broken = true;
        moved = true;
    }
    if gone && b.tx.is_empty() && !e.closing {
        s.close();
        e.closing = true;
    }
    let done = matches!(s.state(), tcp::State::Closed | tcp::State::TimeWait);
    if done {
        b.eof = true;
        b.broken = true;
    }
    if moved || done {
        e.conn.cv.notify_all();
    }
    (moved, done)
}

impl ClientSvc {
    /// Creates the service; the DNS socket exists only if the profile has DNS servers usable
    /// with the tunnel's address families.
    fn new(cfg: &WgConfig, sockets: &mut SocketSet<'static>, v4: bool, v6: bool) -> ClientSvc {
        let servers: Vec<IpAddress> = cfg
            .dns
            .iter()
            .filter(|ip| (ip.is_ipv4() && v4) || (ip.is_ipv6() && v6))
            .map(|ip| IpAddress::from(*ip))
            .collect();
        let dns = (!servers.is_empty()).then(|| sockets.add(dns::Socket::new(&servers, vec![])));
        ClientSvc {
            tcp: Vec::new(),
            udp: Vec::new(),
            dns,
            qtype: if v4 { DnsQueryType::A } else { DnsQueryType::Aaaa },
            pending_dns: Vec::new(),
            ports: HashSet::new(),
            max_tcp: MAX_TCP_CONNS,
        }
    }
}

impl Service for ClientSvc {
    type Cmd = Cmd;

    /// Opens sockets and starts DNS queries for the SOCKS threads.
    fn command(&mut self, cmd: Cmd, iface: &mut Interface, sockets: &mut SocketSet<'static>) {
        match cmd {
            Cmd::Connect(to, conn) => {
                // The cap bounds memory (two TCP_BUF + two APP_BUF per connection); a refused
                // CONNECT is SOCKS reply 5, which libtorrent treats like any refused peer.
                let port = if self.tcp.len() >= self.max_tcp { Err(io::ErrorKind::ConnectionRefused) } else {
                    alloc_port(&mut self.ports).ok_or(io::ErrorKind::AddrNotAvailable)
                };
                let port = match port {
                    Ok(p) => p,
                    Err(k) => {
                        conn.m.lock().unwrap().failed = Some(k);
                        conn.cv.notify_all();
                        return;
                    }
                };
                let mut s = new_tcp();
                s.set_timeout(Some(smoltcp::time::Duration::from_secs(TCP_TIMEOUT)));
                if s.connect(iface.context(), to, port).is_err() {
                    self.ports.remove(&port);
                    conn.m.lock().unwrap().failed = Some(io::ErrorKind::InvalidInput);
                    conn.cv.notify_all();
                    return;
                }
                let h = sockets.add(s);
                self.tcp.push(TcpEntry { h, port, conn, closing: false });
            }
            Cmd::Resolve(name, reply) => {
                let Some(h) = self.dns else {
                    let _ = reply.send(Err(io::Error::other("no DNS server configured for this tunnel")));
                    return;
                };
                if self.pending_dns.len() >= DNS_PENDING_MAX {
                    let _ = reply.send(Err(io::Error::other("too many DNS lookups in flight")));
                    return;
                }
                match sockets.get_mut::<dns::Socket>(h).start_query(iface.context(), &name, self.qtype) {
                    Ok(q) => self.pending_dns.push((q, reply)),
                    Err(_) => {
                        let _ = reply.send(Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid DNS name")));
                    }
                }
            }
            Cmd::UdpOpen(conn) => {
                let Some(port) = alloc_port(&mut self.ports) else {
                    conn.m.lock().unwrap().dead = true; // no free port: the association fails
                    conn.cv.notify_all();
                    return;
                };
                let buf = || udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; UDP_SLOTS], vec![0u8; UDP_BUF]);
                let mut s = udp::Socket::new(buf(), buf());
                if s.bind(port).is_err() {
                    self.ports.remove(&port);
                    conn.m.lock().unwrap().dead = true;
                    conn.cv.notify_all();
                    return;
                }
                let h = sockets.add(s);
                self.udp.push(UdpEntry { h, port, conn });
            }
            Cmd::UdpSend(conn, payload, to) => {
                if let Some(e) = self.udp.iter().find(|e| Arc::ptr_eq(&e.conn, &conn)) {
                    // A full send buffer drops the datagram, as UDP may.
                    let _ = sockets.get_mut::<udp::Socket>(e.h).send_slice(&payload, to);
                }
            }
            #[cfg(feature = "test-support")]
            Cmd::Panic => panic!("test-support: worker panic requested"),
        }
    }

    /// Pumps every TCP connection, delivers UDP datagrams and finished DNS answers.
    fn service(&mut self, _iface: &mut Interface, sockets: &mut SocketSet<'static>) -> bool {
        let mut moved = false;
        let ClientSvc { tcp: tcps, udp: udps, dns: dns_h, pending_dns, ports, .. } = self;
        tcps.retain_mut(|e| {
            let (m, done) = pump_tcp(e, sockets.get_mut::<tcp::Socket>(e.h));
            moved |= m;
            if done {
                sockets.remove(e.h);
                ports.remove(&e.port);
            }
            !done
        });
        udps.retain(|e| {
            if Arc::strong_count(&e.conn) == 1 {
                sockets.remove(e.h); // association closed
                ports.remove(&e.port);
                return false;
            }
            let s = sockets.get_mut::<udp::Socket>(e.h);
            let mut got = false;
            let mut b = e.conn.m.lock().unwrap();
            while let Ok((d, meta)) = s.recv() {
                got = true;
                if b.rx.len() < UDP_QUEUE {
                    b.rx.push_back((d.to_vec(), SocketAddr::from(meta.endpoint)));
                }
            }
            if got {
                e.conn.cv.notify_all();
                moved = true;
            }
            true
        });
        if let Some(h) = *dns_h {
            let s = sockets.get_mut::<dns::Socket>(h);
            pending_dns.retain(|(q, reply)| match s.get_query_result(*q) {
                Err(dns::GetQueryResultError::Pending) => true,
                Ok(addrs) => {
                    let r = addrs.first().map(|a| IpAddr::from(*a)).ok_or_else(|| io::Error::other("name has no address"));
                    let _ = reply.send(r);
                    false
                }
                Err(_) => {
                    let _ = reply.send(Err(io::Error::other("DNS lookup inside the tunnel failed")));
                    false
                }
            });
        }
        moved
    }

    /// Ends every connection and association so SOCKS relays finish instead of hanging.
    fn stop(&mut self, _sockets: &mut SocketSet<'static>) {
        for e in &self.tcp {
            let mut b = e.conn.m.lock().unwrap();
            if !b.open {
                b.failed = Some(io::ErrorKind::NetworkUnreachable);
            }
            b.eof = true;
            b.broken = true;
            e.conn.cv.notify_all();
        }
        for e in &self.udp {
            e.conn.m.lock().unwrap().dead = true;
            e.conn.cv.notify_all();
        }
        for (_, reply) in self.pending_dns.drain(..) {
            let _ = reply.send(Err(io::Error::new(io::ErrorKind::NetworkUnreachable, "tunnel is down")));
        }
    }
}

/// App-side handle to a tunnelled TCP connection (one per relay direction, via `try_clone_box`).
struct TunnelStream {
    conn: Arc<Conn>,
    link: Arc<Link<Cmd>>,
}

impl TunnelStream {
    /// Locks this connection's buffers.
    fn lock(&self) -> MutexGuard<'_, ConnBuf> {
        self.conn.m.lock().unwrap()
    }
}

impl Read for TunnelStream {
    /// Blocks until bytes arrive; returns 0 at end of stream or after `shutdown`.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut b = self.lock();
        loop {
            if !b.rx.is_empty() {
                let n = drain_into(&mut b.rx, buf);
                drop(b);
                self.link.wake(); // room freed: the worker may pull more from smoltcp
                return Ok(n);
            }
            if b.eof || b.closed {
                return Ok(0);
            }
            b = self.conn.cv.wait(b).unwrap();
        }
    }
}

impl Write for TunnelStream {
    /// Blocks while the send buffer is full; fails once the connection can no longer send.
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut b = self.lock();
        loop {
            if b.closed || b.broken {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if b.tx.len() < APP_BUF {
                let n = buf.len().min(APP_BUF - b.tx.len());
                b.tx.extend(&buf[..n]);
                drop(b);
                self.link.wake();
                return Ok(n);
            }
            b = self.conn.cv.wait(b).unwrap();
        }
    }

    /// Nothing to flush: written bytes are already queued for the worker.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Stream for TunnelStream {
    /// Another handle to the same connection.
    fn try_clone_box(&self) -> io::Result<Box<dyn Stream>> {
        Ok(Box::new(TunnelStream { conn: self.conn.clone(), link: self.link.clone() }))
    }

    /// Ends reads on every handle at once; queued bytes are still sent, then the connection closes.
    fn shutdown(&self) -> io::Result<()> {
        self.lock().closed = true;
        self.conn.cv.notify_all();
        self.link.wake();
        Ok(())
    }
}

impl Drop for TunnelStream {
    /// Lets the worker notice when the last handle is gone. The wake runs just before the `Arc`
    /// is released, so a pass may still see the handle; the worker re-checks on every timer pass
    /// (at most `TICK` later), which bounds that delay.
    fn drop(&mut self) {
        self.link.wake();
    }
}

/// App-side handle to a UDP association inside the tunnel.
struct TunnelUdp {
    conn: Arc<UdpConn>,
    backend: Arc<TunnelBackend>,
}

impl UdpPort for TunnelUdp {
    /// Resolves the target inside the tunnel if needed and queues the datagram.
    fn send_to(&self, payload: &[u8], to: &Target) -> io::Result<()> {
        let addr = self.backend.resolve(to)?;
        self.backend.link.send(Cmd::UdpSend(self.conn.clone(), payload.to_vec(), addr))
    }

    /// Waits up to `timeout` for a datagram; `Duration::ZERO` never blocks. Oversized datagrams
    /// are truncated to `buf`.
    fn recv(&self, buf: &mut [u8], timeout: Duration) -> io::Result<Option<(usize, SocketAddr)>> {
        let mut b = self.conn.m.lock().unwrap();
        if b.rx.is_empty() && !b.dead && !timeout.is_zero() {
            b = self.conn.cv.wait_timeout_while(b, timeout, |b| b.rx.is_empty() && !b.dead).unwrap().0;
        }
        match b.rx.pop_front() {
            Some((d, from)) => {
                let n = d.len().min(buf.len());
                buf[..n].copy_from_slice(&d[..n]);
                Ok(Some((n, from)))
            }
            None if b.dead => Err(io::Error::new(io::ErrorKind::NetworkUnreachable, "tunnel is down")),
            None => Ok(None),
        }
    }
}

impl Drop for TunnelUdp {
    /// Lets the worker close the smoltcp socket (same `TICK`-bounded race as `TunnelStream`).
    fn drop(&mut self) {
        self.backend.link.wake();
    }
}

/// The SOCKS backend of a live tunnel.
struct TunnelBackend {
    link: Arc<Link<Cmd>>,
    stats: Arc<WireStats>,
    v4: bool,
    v6: bool,
}

/// Error for "this cannot go through the tunnel".
fn unreachable(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::NetworkUnreachable, msg)
}

impl TunnelBackend {
    /// Refuses everything until the first WireGuard handshake has completed, and for good once
    /// the worker has died.
    fn ready(&self) -> io::Result<()> {
        if self.stats.dead.load(Ordering::SeqCst) {
            Err(unreachable("tunnel is down (worker stopped)"))
        } else if self.stats.handshaked() {
            Ok(())
        } else {
            Err(unreachable("no WireGuard handshake yet"))
        }
    }

    /// Turns a target into an address inside the tunnel: domains are looked up by the worker's
    /// DNS socket (through the tunnel); a family the tunnel has no address for is refused.
    fn resolve(&self, t: &Target) -> io::Result<SocketAddr> {
        let addr = match t {
            Target::Addr(a) => *a,
            Target::Domain(name, port) => match name.parse::<IpAddr>() {
                Ok(ip) => SocketAddr::new(ip, *port),
                Err(_) => {
                    let (tx, rx) = mpsc::sync_channel(1);
                    self.link.send(Cmd::Resolve(name.clone(), tx))?;
                    let ip = rx.recv_timeout(DNS_WAIT).map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
                    SocketAddr::new(ip, *port)
                }
            },
        };
        match addr.ip() {
            IpAddr::V4(_) if !self.v4 => Err(unreachable("IPv4 target but the tunnel has no IPv4 address")),
            IpAddr::V6(_) if !self.v6 => Err(unreachable("IPv6 target but the tunnel has no IPv6 address")),
            _ => Ok(addr),
        }
    }
}

/// `Backend` lives on an `Arc<TunnelBackend>` so UDP ports can keep the backend for DNS.
struct BackendHandle(Arc<TunnelBackend>);

impl Backend for BackendHandle {
    /// Opens a TCP connection inside the tunnel and waits for it to be established.
    fn connect_tcp(&self, t: &Target) -> io::Result<Box<dyn Stream>> {
        let be = &self.0;
        be.ready()?;
        let addr = be.resolve(t)?;
        let conn = Arc::new(Conn::default());
        be.link.send(Cmd::Connect(addr, conn.clone()))?;
        let stream = TunnelStream { conn, link: be.link.clone() };
        let deadline = Instant::now() + CONNECT_WAIT;
        let mut b = stream.lock();
        loop {
            if b.open {
                drop(b);
                return Ok(Box::new(stream));
            }
            if let Some(k) = b.failed {
                return Err(k.into());
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                b.closed = true; // the worker aborts the attempt
                return Err(io::ErrorKind::TimedOut.into());
            };
            b = stream.conn.cv.wait_timeout(b, left).unwrap().0;
        }
    }

    /// Opens a UDP socket inside the tunnel for one SOCKS association.
    fn udp_open(&self) -> io::Result<Box<dyn UdpPort>> {
        self.0.ready()?;
        let conn = Arc::new(UdpConn::default());
        self.0.link.send(Cmd::UdpOpen(conn.clone()))?;
        Ok(Box::new(TunnelUdp { conn, backend: self.0.clone() }))
    }
}

/// Backend used when no usable profile exists: nothing ever leaves.
struct BlockedBackend;

impl Backend for BlockedBackend {
    /// Always network-unreachable (SOCKS reply 3).
    fn connect_tcp(&self, _: &Target) -> io::Result<Box<dyn Stream>> {
        Err(unreachable("no VPN profile is active"))
    }
    /// Always network-unreachable (SOCKS reply 3).
    fn udp_open(&self) -> io::Result<Box<dyn UdpPort>> {
        Err(unreachable("no VPN profile is active"))
    }
}

// ---------------------------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------------------------

/// Snapshot of the tunnel's health for the network lock and the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TunnelStats {
    /// Time since the last CONFIRMED contact with the peer: a handshake we initiated that the
    /// peer completed (its response arrived). Received initiations and received data do not
    /// count: neither proves our packets reach the peer (lab finding F1). The client re-keys
    /// every `rekey_age` (120 s by default), so a healthy tunnel stays under that plus retries.
    /// `None` before the first completed handshake.
    pub handshake_age: Option<Duration>,
    /// Decrypted payload bytes received / plaintext bytes sent through WireGuard.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Why a tunnel could not be started.
#[derive(Debug)]
pub enum TunnelError {
    /// The profile has no interface address.
    NoAddress,
    Io(io::Error),
}

impl fmt::Display for TunnelError {
    /// One-line message for the UI.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TunnelError::NoAddress => write!(f, "The WireGuard profile has no interface Address."),
            TunnelError::Io(e) => write!(f, "Could not start the WireGuard tunnel: {e}"),
        }
    }
}

impl std::error::Error for TunnelError {}

impl From<io::Error> for TunnelError {
    /// Wraps socket and thread errors.
    fn from(e: io::Error) -> Self {
        TunnelError::Io(e)
    }
}

/// A running tunnel (or a blocked stand-in) with its local SOCKS5 port. Dropping it stops the
/// SOCKS server, ends every proxied connection, joins the worker and closes the UDP socket.
pub struct Tunnel {
    socks: Option<SocksServer>,
    stats: Arc<WireStats>,
    link: Option<Arc<Link<Cmd>>>,
    worker: Option<JoinHandle<()>>,
}

impl Tunnel {
    /// Brings the tunnel up to `endpoint` (already resolved by the caller) and starts the SOCKS
    /// server. Returns at once; the handshake completes in the background (see `stats`).
    pub fn up(cfg: &WgConfig, endpoint: SocketAddr) -> Result<Tunnel, TunnelError> {
        Tunnel::up_with_rekey(cfg, endpoint, REKEY_AGE)
    }

    /// Like `up`, but re-keys once OUR last completed handshake (the confirmed contact, see
    /// `TunnelStats::handshake_age`) is `rekey_age` old instead of `REKEY_AGE`. The
    /// app keeps it below the lock's stale threshold (tests shorten both), so a healthy tunnel
    /// never looks stale and a dead one re-handshakes as soon as the server is back.
    pub fn up_with_rekey(cfg: &WgConfig, endpoint: SocketAddr, rekey_age: Duration) -> Result<Tunnel, TunnelError> {
        let v4 = cfg.addresses.iter().any(|(ip, _)| ip.is_ipv4());
        let v6 = cfg.addresses.iter().any(|(ip, _)| ip.is_ipv6());
        if !v4 && !v6 {
            return Err(TunnelError::NoAddress);
        }
        let sock = UdpSocket::bind(if endpoint.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
        sock.connect(endpoint)?;
        let tunn = make_tunn(cfg.private_key, cfg.peer_public_key, cfg.preshared_key, cfg.keepalive);
        let wire = Wire::new(tunn, sock, true, Some(rekey_age), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let mut dev = VirtDevice::default();
        let iface = make_iface(&mut dev, &cfg.addresses, true);
        let mut sockets = SocketSet::new(vec![]);
        let svc = ClientSvc::new(cfg, &mut sockets, v4, v6);
        let stats = Arc::new(WireStats::default());
        let engine = Engine { wire, dev, iface, sockets, svc, stats: stats.clone() };
        let (link, worker) = spawn_engine(engine, "nyx-wg")?;
        let backend = Arc::new(TunnelBackend { link: link.clone(), stats: stats.clone(), v4, v6 });
        let mut t = Tunnel { socks: None, stats, link: Some(link), worker: Some(worker) };
        t.socks = Some(SocksServer::start(Arc::new(BackendHandle(backend)))?); // on error, Drop stops the worker
        Ok(t)
    }

    /// A SOCKS server whose backend refuses everything (network unreachable). Used when no
    /// usable profile exists, so clients fail fast instead of finding another way out.
    pub fn blocked() -> io::Result<Tunnel> {
        Ok(Tunnel {
            socks: Some(SocksServer::start(Arc::new(BlockedBackend))?),
            stats: Arc::new(WireStats::default()),
            link: None,
            worker: None,
        })
    }

    /// The local SOCKS5 address (127.0.0.1, random port).
    pub fn socks_addr(&self) -> SocketAddr {
        self.socks.as_ref().map(|s| s.addr()).expect("SOCKS server runs for the tunnel's lifetime")
    }

    /// The SOCKS5 username and password for this tunnel.
    pub fn credentials(&self) -> Credentials {
        self.socks.as_ref().map(|s| s.credentials()).expect("SOCKS server runs for the tunnel's lifetime")
    }

    /// Test hook: makes the worker thread panic (see `worker_panic_reports_tunnel_down`).
    #[cfg(feature = "test-support")]
    pub fn panic_worker_for_test(&self) {
        if let Some(l) = &self.link {
            let _ = l.send(Cmd::Panic);
        }
    }

    /// Contact age (see `TunnelStats::handshake_age`) and byte counters. Once the worker has died
    /// the tunnel is down: no contact (`None`) and zeroed counters, so nothing reads it as alive.
    pub fn stats(&self) -> TunnelStats {
        if self.stats.dead.load(Ordering::SeqCst) {
            return TunnelStats { handshake_age: None, rx_bytes: 0, tx_bytes: 0 };
        }
        TunnelStats {
            handshake_age: self.stats.contact_at.lock().unwrap().map(|t| t.elapsed()),
            rx_bytes: self.stats.rx.load(Ordering::Relaxed),
            tx_bytes: self.stats.tx.load(Ordering::Relaxed),
        }
    }
}

impl Drop for Tunnel {
    /// Stops accepting first, then stops the worker (which ends every connection) and joins it.
    fn drop(&mut self) {
        drop(self.socks.take());
        if let Some(l) = self.link.take() {
            l.stop();
        }
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conf::Endpoint;

    /// With every ephemeral port taken, `alloc_port` gives up instead of looping forever.
    #[test]
    fn alloc_port_fails_when_no_port_is_free() {
        let mut ports: HashSet<u16> = (49152..=65535u16).collect();
        assert_eq!(alloc_port(&mut ports), None);
        let mut free = HashSet::new();
        assert!(alloc_port(&mut free).is_some_and(|p| p >= 49152 && free.contains(&p)));
    }

    /// Past the connection cap a CONNECT is refused (ConnectionRefused, SOCKS reply 5) without
    /// opening a smoltcp socket; the cap is lowered to 2 here so the test stays cheap.
    #[test]
    fn connect_past_the_cap_is_refused() {
        let cfg = WgConfig {
            private_key: [1; 32],
            addresses: vec![(IpAddr::from([10, 0, 0, 2]), 32)],
            dns: vec![],
            peer_public_key: [2; 32],
            preshared_key: None,
            endpoint: Endpoint { host: "127.0.0.1".into(), port: 1 },
            keepalive: None,
        };
        let mut dev = VirtDevice::default();
        let mut iface = make_iface(&mut dev, &cfg.addresses, true);
        let mut sockets = SocketSet::new(vec![]);
        let mut svc = ClientSvc::new(&cfg, &mut sockets, true, false);
        svc.max_tcp = 2;
        let to: SocketAddr = "10.0.0.1:7".parse().unwrap();
        let conns: Vec<Arc<Conn>> = (0..3).map(|_| Arc::new(Conn::default())).collect();
        for c in &conns {
            svc.command(Cmd::Connect(to, c.clone()), &mut iface, &mut sockets);
        }
        assert_eq!(svc.tcp.len(), 2);
        assert_eq!(conns[0].m.lock().unwrap().failed, None);
        assert_eq!(conns[2].m.lock().unwrap().failed, Some(io::ErrorKind::ConnectionRefused));
    }
}
