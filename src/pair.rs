//! One-time pairing. One machine shows a six-digit code, the other types it in.
//! SPAKE2 turns the code into a strong shared key (a wrong guess learns nothing and can't be
//! brute-forced offline), and that key carries each side's Noise public key across.

use crate::config::{Config, Peer};
use crate::net::{read_frame, write_frame};
use crate::proto::{Os, PAIR_PORT, PAIR_SERVICE};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use rand::Rng;
use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct Card {
    name: String,
    os: Os,
    public_key: String,
    port: u16,
}

type Res<T> = Result<T, String>;

fn exchange(s: &mut TcpStream, code: &str, cfg: &Config, host: bool) -> Res<Peer> {
    let _ = s.set_read_timeout(Some(Duration::from_secs(20)));
    let (state, msg) = Spake2::<Ed25519Group>::start_symmetric(
        &Password::new(code.as_bytes()),
        &Identity::new(b"bridge-pair-v1"),
    );
    write_frame(s, &msg).map_err(|e| e.to_string())?;
    let theirs = read_frame(s).map_err(|e| e.to_string())?;
    let key = state.finish(&theirs).map_err(|_| "pairing failed".to_string())?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key[..32]));
    let (mine_n, theirs_n) = if host { ([1u8; 12], [2u8; 12]) } else { ([2u8; 12], [1u8; 12]) };
    let card = Card { name: crate::machine_name(), os: Os::current(), public_key: cfg.public_key.clone(), port: cfg.port };
    let sealed = cipher
        .encrypt(Nonce::from_slice(&mine_n), postcard::to_stdvec(&card).unwrap().as_slice())
        .unwrap();
    write_frame(s, &sealed).map_err(|e| e.to_string())?;
    let got = read_frame(s).map_err(|e| e.to_string())?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&theirs_n), got.as_slice())
        .map_err(|_| "wrong code".to_string())?;
    let card: Card = postcard::from_bytes(&plain).map_err(|e| e.to_string())?;
    Ok(Peer {
        name: card.name,
        os: card.os,
        public_key: card.public_key,
        addr: s.peer_addr().ok().map(|a| a.ip().to_string()),
        macs: vec![],
        port: card.port,
    })
}

pub fn new_code() -> String {
    format!("{:06}", rand::thread_rng().gen_range(0..1_000_000))
}

/// Wrong codes allowed before the code is thrown away. Each guess needs a full network round
/// trip and SPAKE2 gives an attacker exactly one guess per attempt, so 3 tries in a million is safe.
const MAX_ATTEMPTS: u32 = 3;

/// This machine's LAN address, to show next to the code in case mDNS can't find it.
pub fn local_ip() -> Option<std::net::IpAddr> {
    // Connecting a UDP socket sends nothing; it just picks the outgoing interface.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// The latest pairing session. Starting a new one ends any older one, so only one code is
/// ever live and the code on screen is always the one being listened for.
static SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// End whatever pairing session is waiting.
pub fn cancel() {
    SESSION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

pub const REPLACED: &str = "replaced by a newer pairing code";

/// Wait (up to 5 minutes) for the other machine to join with `code`.
/// `show` is called once the code is actually live, never before.
pub fn host(cfg: &Config, code: &str, show: impl FnOnce()) -> Res<Peer> {
    use std::sync::atomic::Ordering;
    let me = SESSION.fetch_add(1, Ordering::SeqCst) + 1;
    // An older session notices within 200 ms and releases the port.
    let bind_deadline = Instant::now() + Duration::from_secs(2);
    let listener = loop {
        match TcpListener::bind(("0.0.0.0", PAIR_PORT)) {
            Ok(l) => break l,
            Err(e) if Instant::now() > bind_deadline => return Err(format!("port {PAIR_PORT}: {e}")),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    show();
    let daemon = ServiceDaemon::new().ok();
    if let Some(d) = &daemon {
        let name = crate::machine_name();
        let label = format!("{}.local.", name.replace([' ', '.'], "-"));
        if let Ok(info) = ServiceInfo::new(PAIR_SERVICE, &name, &label, "", PAIR_PORT, None::<std::collections::HashMap<String, String>>) {
            let _ = d.register(info.enable_addr_auto());
        }
    }
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut failures = 0;
    let result = loop {
        if Instant::now() > deadline {
            break Err("nobody joined within 5 minutes".into());
        }
        if SESSION.load(Ordering::SeqCst) != me {
            break Err(REPLACED.into());
        }
        if failures >= MAX_ATTEMPTS {
            break Err("too many wrong codes; start pairing again for a new one".into());
        }
        match listener.accept() {
            Ok((mut s, from)) => {
                let _ = s.set_nonblocking(false);
                match exchange(&mut s, code, cfg, true) {
                    Ok(p) => break Ok(p),
                    Err(e) => {
                        // Only a real guess counts; a connection that drops early (a port
                        // scan, a network blip) learns nothing and costs nothing.
                        if e == "wrong code" || e == "pairing failed" {
                            failures += 1;
                        }
                        log::warn!("pair attempt from {from} failed: {e}");
                    }
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    if let Some(d) = daemon {
        let _ = d.shutdown();
    }
    result
}

/// Join a machine that is showing `code`. `addr` may be omitted to find it on the network.
pub fn join(cfg: &Config, code: &str, addr: Option<&str>) -> Res<Peer> {
    let targets: Vec<SocketAddr> = match addr {
        Some(a) => {
            use std::net::ToSocketAddrs;
            (a, PAIR_PORT).to_socket_addrs().map_err(|e| format!("{a}: {e}"))?.collect()
        }
        None => find_host()?,
    };
    let mut last = String::from("no address");
    for t in targets {
        match TcpStream::connect_timeout(&t, Duration::from_secs(3)) {
            Ok(mut s) => return exchange(&mut s, code, cfg, false),
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("could not reach the other machine: {last}"))
}

fn find_host() -> Res<Vec<SocketAddr>> {
    let d = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let rx = d.browse(PAIR_SERVICE).map_err(|e| e.to_string())?;
    // Skip this machine: it may be showing a code of its own.
    let me = crate::machine_name();
    let mine: Vec<std::net::IpAddr> = local_ip().into_iter().collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Ok(ev) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if let ServiceEvent::ServiceResolved(info) = ev {
            let own_name = info.get_fullname().split('.').next() == Some(me.as_str());
            if own_name || info.get_addresses().iter().any(|a| mine.contains(a) || a.is_loopback()) {
                continue;
            }
            let mut v: Vec<SocketAddr> =
                info.get_addresses().iter().map(|a| SocketAddr::new(*a, PAIR_PORT)).collect();
            v.sort_by_key(|a| a.is_ipv6());
            let _ = d.shutdown();
            return Ok(v);
        }
    }
    let _ = d.shutdown();
    Err("couldn't find the other machine on the network. Type the code followed by the address it shows, e.g. 123456 192.168.1.20".into())
}

/// Store a freshly paired peer.
pub fn save(cfg: &mut Config, peer: Peer) {
    log::info!("paired with {} ({:?})", peer.name, peer.os);
    cfg.peer = Some(peer);
    cfg.save();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that use the real pairing port take turns.
    static PORT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn cfg() -> Config {
        let kp = snow::Builder::new(crate::net::NOISE.parse().unwrap()).generate_keypair().unwrap();
        Config { private_key: hex::encode(kp.private), public_key: hex::encode(kp.public), ..Default::default() }
    }

    fn run(host_code: &str, join_code: &str) -> (Res<Peer>, Res<Peer>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let (a, b) = (cfg(), cfg());
        let hc = host_code.to_string();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            exchange(&mut s, &hc, &a, true)
        });
        let mut s = TcpStream::connect(addr).unwrap();
        let j = exchange(&mut s, join_code, &b, false);
        drop(s);
        (t.join().unwrap(), j)
    }

    #[test]
    fn host_gives_up_after_three_wrong_codes() {
        let _turn = PORT.lock().unwrap_or_else(|e| e.into_inner());
        let host_cfg = cfg();
        let t = std::thread::spawn(move || host(&host_cfg, "111111", || {}));
        std::thread::sleep(Duration::from_millis(300));
        for _ in 0..3 {
            let _ = join(&cfg(), "222222", Some("127.0.0.1"));
        }
        // Even the right code is refused now.
        let err = t.join().unwrap().unwrap_err();
        assert!(err.contains("too many"), "{err}");
    }

    #[test]
    fn newer_code_replaces_older() {
        let _turn = PORT.lock().unwrap_or_else(|e| e.into_inner());
        let (a, b) = (cfg(), cfg());
        let old = std::thread::spawn(move || host(&a, "111111", || {}));
        std::thread::sleep(Duration::from_millis(300));
        let new = std::thread::spawn(move || host(&b, "222222", || {}));
        assert_eq!(old.join().unwrap().unwrap_err(), REPLACED);
        std::thread::sleep(Duration::from_millis(300));
        // A dropped connection isn't a guess, and the newest code is the one that works.
        drop(TcpStream::connect(("127.0.0.1", PAIR_PORT)));
        assert!(join(&cfg(), "222222", Some("127.0.0.1")).is_ok());
        assert!(new.join().unwrap().is_ok());
    }

    #[test]
    fn right_code_pairs() {
        let (h, j) = run("123456", "123456");
        assert!(h.is_ok() && j.is_ok());
    }

    #[test]
    fn wrong_code_fails() {
        let (h, j) = run("123456", "654321");
        assert!(h.is_err() && j.is_err());
    }
}
