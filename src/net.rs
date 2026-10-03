//! One encrypted TCP link to the paired machine.
//!
//! Noise KK with both static keys pinned at pairing, so nothing unpaired can connect or listen in.
//! Machines find each other with mDNS (falling back to the last address that worked).
//! The machine with the smaller public key dials; the other only listens, so there is never a race.

use crate::config::Config;
use crate::engine::Event;
use crate::proto::{Msg, SERVICE};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use snow::StatelessTransportState;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub const NOISE: &str = "Noise_KK_25519_ChaChaPoly_BLAKE2s";
const CHUNK: usize = 60_000;
/// Largest message accepted, even from the paired machine (copied files are capped well below).
const MAX_MESSAGE: usize = 512 << 20;

pub(crate) fn write_frame(s: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let mut out = Vec::with_capacity(data.len() + 2);
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    s.write_all(&out)
}

pub(crate) fn read_frame(s: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 2];
    s.read_exact(&mut len)?;
    let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
    s.read_exact(&mut buf)?;
    Ok(buf)
}

fn bad(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

fn handshake(s: &mut TcpStream, cfg: &Config, initiator: bool) -> io::Result<StatelessTransportState> {
    let peer = cfg.peer.as_ref().ok_or_else(|| bad("not paired"))?;
    let remote = hex::decode(&peer.public_key).map_err(bad)?;
    let local = cfg.private_key();
    let b = snow::Builder::new(NOISE.parse().unwrap())
        .local_private_key(&local)
        .remote_public_key(&remote);
    let mut hs = if initiator { b.build_initiator() } else { b.build_responder() }.map_err(bad)?;
    let mut buf = [0u8; 256];
    let mut sink = [0u8; 256];
    if initiator {
        let n = hs.write_message(&[], &mut buf).map_err(bad)?;
        write_frame(s, &buf[..n])?;
        hs.read_message(&read_frame(s)?, &mut sink).map_err(bad)?;
    } else {
        hs.read_message(&read_frame(s)?, &mut sink).map_err(bad)?;
        let n = hs.write_message(&[], &mut buf).map_err(bad)?;
        write_frame(s, &buf[..n])?;
    }
    hs.into_stateless_transport_mode().map_err(bad)
}

struct Writer {
    stream: TcpStream,
    ts: Arc<StatelessTransportState>,
    nonce: u64,
}

#[derive(Clone)]
pub struct Link {
    writer: Arc<Mutex<Writer>>,
    generation: u64,
}

impl Link {
    pub fn send(&self, m: &Msg) -> io::Result<()> {
        let body = postcard::to_stdvec(m).map_err(bad)?;
        let mut plain = Vec::with_capacity(body.len() + 4);
        plain.extend_from_slice(&(body.len() as u32).to_be_bytes());
        plain.extend_from_slice(&body);
        let mut w = self.writer.lock().unwrap();
        let mut out = Vec::with_capacity(plain.len() + plain.len() / CHUNK * 18 + 18);
        let mut ct = vec![0u8; CHUNK + 16];
        for chunk in plain.chunks(CHUNK) {
            let n = w.ts.write_message(w.nonce, chunk, &mut ct).map_err(bad)?;
            w.nonce += 1;
            out.extend_from_slice(&(n as u16).to_be_bytes());
            out.extend_from_slice(&ct[..n]);
        }
        w.stream.write_all(&out)
    }
}

fn reader(mut s: TcpStream, ts: Arc<StatelessTransportState>, events: &Sender<Event>) -> io::Result<()> {
    let mut nonce = 0u64;
    let mut plain: Vec<u8> = Vec::new();
    let mut pt = vec![0u8; CHUNK + 16];
    loop {
        let frame = read_frame(&mut s)?;
        let n = ts.read_message(nonce, &frame, &mut pt).map_err(bad)?;
        nonce += 1;
        plain.extend_from_slice(&pt[..n]);
        while plain.len() >= 4 {
            let len = u32::from_be_bytes(plain[..4].try_into().unwrap()) as usize;
            if len > MAX_MESSAGE {
                return Err(bad("message too large"));
            }
            if plain.len() < 4 + len {
                break;
            }
            match postcard::from_bytes::<Msg>(&plain[4..4 + len]) {
                Ok(m) => {
                    if !matches!(m, Msg::Ping) {
                        let _ = events.send(Event::Net(m));
                    }
                }
                Err(e) => log::warn!("undecodable message (newer version on the other side?): {e}"),
            }
            plain.drain(..4 + len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Peer;
    use crate::proto::{Clip, Os};

    fn pair_cfgs() -> (Config, Config) {
        let gen = || snow::Builder::new(NOISE.parse().unwrap()).generate_keypair().unwrap();
        let (a, b) = (gen(), gen());
        let peer = |k: &snow::Keypair| Peer { name: "x".into(), os: Os::Mac, public_key: hex::encode(&k.public), addr: None, macs: vec![], port: 0 };
        let ca = Config { private_key: hex::encode(&a.private), public_key: hex::encode(&a.public), peer: Some(peer(&b)), ..Default::default() };
        let cb = Config { private_key: hex::encode(&b.private), public_key: hex::encode(&b.public), peer: Some(peer(&a)), ..Default::default() };
        (ca, cb)
    }

    #[test]
    fn link_carries_small_and_huge_messages() {
        let (ca, cb) = pair_cfgs();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let ts = Arc::new(handshake(&mut s, &cb, false).unwrap());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || reader(s, ts, &tx));
            (0..2).map(|_| rx.recv().unwrap()).collect::<Vec<_>>()
        });
        let mut s = TcpStream::connect(addr).unwrap();
        let ts = Arc::new(handshake(&mut s, &ca, true).unwrap());
        let link = Link { writer: Arc::new(Mutex::new(Writer { stream: s, ts, nonce: 0 })), generation: 1 };
        let big = vec![7u8; 3_000_000];
        link.send(&Msg::MouseMove { dx: 1.5, dy: -2.0 }).unwrap();
        link.send(&Msg::Clipboard(Clip::Image(big.clone()))).unwrap();
        let got = t.join().unwrap();
        assert!(matches!(got[0], Event::Net(Msg::MouseMove { dx, .. }) if dx == 1.5));
        assert!(matches!(&got[1], Event::Net(Msg::Clipboard(Clip::Image(b))) if *b == big));
    }

    #[test]
    fn stranger_is_rejected() {
        let (ca, _) = pair_cfgs();
        let (_, stranger) = pair_cfgs();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            handshake(&mut s, &ca, false).is_ok()
        });
        let mut s = TcpStream::connect(addr).unwrap();
        let _ = handshake(&mut s, &stranger, true);
        drop(s);
        assert!(!t.join().unwrap());
    }
}

pub struct Net {
    cfg: Arc<RwLock<Config>>,
    events: Sender<Event>,
    link: Mutex<Option<Link>>,
    generation: AtomicU64,
    discovered: Mutex<HashMap<String, Vec<IpAddr>>>,
}

impl Net {
    /// Fails if the port is taken, which also means Bridge is already running.
    pub fn start(cfg: Arc<RwLock<Config>>, events: Sender<Event>) -> io::Result<Arc<Net>> {
        let port = cfg.read().unwrap().port;
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        let discovery = cfg.read().unwrap().discovery;
        let net = Arc::new(Net {
            cfg,
            events,
            link: Mutex::new(None),
            generation: AtomicU64::new(0),
            discovered: Mutex::new(HashMap::new()),
        });
        let n = net.clone();
        std::thread::spawn(move || n.listen(listener));
        if discovery {
            let n = net.clone();
            std::thread::spawn(move || n.discover());
        }
        let n = net.clone();
        std::thread::spawn(move || n.maintain());
        Ok(net)
    }

    pub fn link(&self) -> Option<Link> {
        self.link.lock().unwrap().clone()
    }

    pub fn send(&self, m: &Msg) {
        if let Some(l) = self.link() {
            if let Err(e) = l.send(m) {
                log::warn!("send failed: {e}");
                self.drop_link(l.generation);
            }
        }
    }

    /// Forget the current link (after re-pairing, for instance).
    pub fn reset(&self) {
        let g = self.generation.load(Ordering::SeqCst);
        self.drop_link(g);
    }

    fn drop_link(&self, generation: u64) {
        let mut slot = self.link.lock().unwrap();
        if let Some(l) = slot.as_ref() {
            if l.generation == generation {
                let _ = l.writer.lock().unwrap().stream.shutdown(std::net::Shutdown::Both);
                *slot = None;
                drop(slot);
                let _ = self.events.send(Event::Disconnected);
            }
        }
    }

    fn install(self: &Arc<Self>, mut stream: TcpStream, initiator: bool) {
        let cfg = self.cfg.read().unwrap().clone();
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let ts = match handshake(&mut stream, &cfg, initiator) {
            Ok(ts) => Arc::new(ts),
            Err(e) => {
                log::warn!("handshake with {:?} failed: {e}", stream.peer_addr().ok());
                return;
            }
        };
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(8)));
        let peer_ip = stream.peer_addr().ok().map(|a| a.ip().to_string());
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let Ok(write_half) = stream.try_clone() else { return };
        let link = Link { writer: Arc::new(Mutex::new(Writer { stream: write_half, ts: ts.clone(), nonce: 0 })), generation };
        let replaced = {
            let mut slot = self.link.lock().unwrap();
            let old = slot.replace(link);
            if let Some(old) = &old {
                let _ = old.writer.lock().unwrap().stream.shutdown(std::net::Shutdown::Both);
            }
            old.is_some()
        };
        if let Some(ip) = peer_ip {
            let mut c = self.cfg.write().unwrap();
            if let Some(p) = c.peer.as_mut() {
                if p.addr.as_deref() != Some(ip.as_str()) {
                    p.addr = Some(ip);
                    c.save();
                }
            }
        }
        log::info!("connected ({})", if initiator { "dialed" } else { "accepted" });
        if replaced {
            // The other side restarted: whatever state we had with the old link is gone.
            let _ = self.events.send(Event::Disconnected);
        }
        let _ = self.events.send(Event::Connected);
        let me = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = reader(stream, ts, &me.events) {
                log::info!("link closed: {e}");
            }
            me.drop_link(generation);
        });
    }

    fn listen(self: Arc<Self>, listener: TcpListener) {
        for s in listener.incoming().flatten() {
            let me = self.clone();
            std::thread::spawn(move || me.install(s, false));
        }
    }

    fn discover(self: Arc<Self>) {
        let Ok(daemon) = ServiceDaemon::new() else {
            log::error!("mDNS unavailable; using the saved address only");
            return;
        };
        let (host, id, port) = {
            let c = self.cfg.read().unwrap();
            (crate::machine_name(), c.public_key[..16].to_string(), c.port)
        };
        let host_label = format!("{}.local.", host.replace([' ', '.'], "-"));
        let props = [("id", id.as_str())];
        if let Ok(info) = ServiceInfo::new(SERVICE, &host, &host_label, "", port, &props[..]) {
            let _ = daemon.register(info.enable_addr_auto());
        }
        let Ok(rx) = daemon.browse(SERVICE) else { return };
        while let Ok(ev) = rx.recv() {
            if let ServiceEvent::ServiceResolved(info) = ev {
                if let Some(id) = info.get_property_val_str("id") {
                    let addrs: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
                    self.discovered.lock().unwrap().insert(id.to_string(), addrs);
                }
            }
        }
    }

    /// Pick up pairing done from the command line while the app runs.
    fn reload_if_changed(&self, seen: &mut Option<std::time::SystemTime>) {
        if self.cfg.read().unwrap().ephemeral {
            return;
        }
        let now = crate::config::mtime();
        if now == *seen {
            return;
        }
        *seen = now;
        let fresh = Config::load();
        let mut c = self.cfg.write().unwrap();
        let changed = c.peer.as_ref().map(|p| &p.public_key) != fresh.peer.as_ref().map(|p| &p.public_key)
            || c.public_key != fresh.public_key;
        if changed {
            log::info!("pairing changed on disk; reloading");
            *c = fresh;
            drop(c);
            let _ = self.events.send(Event::Ui(crate::engine::Ui::Repaired));
        }
    }

    fn maintain(self: Arc<Self>) {
        let mut seen = crate::config::mtime();
        loop {
            std::thread::sleep(Duration::from_secs(2));
            self.reload_if_changed(&mut seen);
            if self.link().is_some() {
                self.send(&Msg::Ping);
                continue;
            }
            let cfg = self.cfg.read().unwrap().clone();
            let Some(peer) = cfg.peer.as_ref() else { continue };
            if cfg.public_key >= peer.public_key {
                continue; // the other side dials
            }
            let mut candidates: Vec<IpAddr> = self
                .discovered
                .lock()
                .unwrap()
                .get(&peer.public_key[..16.min(peer.public_key.len())])
                .cloned()
                .unwrap_or_default();
            if let Some(ip) = peer.addr.as_ref().and_then(|a| a.parse().ok()) {
                if !candidates.contains(&ip) {
                    candidates.push(ip);
                }
            }
            // IPv4 first: link-local IPv6 needs scope ids we don't carry.
            candidates.sort_by_key(|ip| ip.is_ipv6());
            for ip in candidates {
                if let Ok(s) = TcpStream::connect_timeout(&SocketAddr::new(ip, peer.port), Duration::from_millis(1500)) {
                    self.install(s, true);
                    if self.link().is_some() {
                        break;
                    }
                }
            }
        }
    }
}
