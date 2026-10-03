//! Who has the keyboard and mouse right now, and what happens when that changes.
//!
//! Local:       input stays here; pushing into the shared edge (or the hotkey) hands it over.
//! Controlling: our input is swallowed and sent to the other machine.
//! Controlled:  the other machine's input is injected here; pushing back out returns it.

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::geometry::{self, Step};
use crate::keymap;
use crate::net::Net;
use crate::platform::{Input, Platform, Rect};
use crate::proto::{Msg, Os};
use std::collections::HashSet;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

pub enum Event {
    Input(Input),
    Net(Msg),
    Connected,
    Disconnected,
    Ui(Ui),
}

pub enum Ui {
    TogglePause,
    Switch,
    Wake,
    Repaired,
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Local,
    Controlling,
    Controlled,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub peer: Option<String>,
    pub connected: bool,
    pub mode: Mode,
    pub paused: bool,
}

pub struct Engine {
    cfg: Arc<RwLock<Config>>,
    net: Arc<Net>,
    plat: Arc<dyn Platform>,
    on_status: Box<dyn Fn(Status) + Send>,
    mode: Mode,
    connected: bool,
    peer_os: Option<Os>,
    displays: Vec<Rect>,
    displays_at: Instant,
    push: f64,
    /// Where the pointer is while the other machine drives it.
    pos: (f64, f64),
    /// Local movement while being driven; enough of it takes the pointer back.
    reclaim: f64,
    keys_down: HashSet<u16>,
    buttons_down: HashSet<u8>,
    hotkey_pressed_at: Option<Instant>,
    hotkey_last_tap: Option<Instant>,
    clipboard: Clipboard,
    last_wake: Option<Instant>,
}

impl Engine {
    pub fn new(
        cfg: Arc<RwLock<Config>>,
        net: Arc<Net>,
        plat: Arc<dyn Platform>,
        on_status: Box<dyn Fn(Status) + Send>,
    ) -> Engine {
        let peer_os = cfg.read().unwrap().peer.as_ref().map(|p| p.os);
        Engine {
            cfg,
            net,
            plat,
            on_status,
            mode: Mode::Local,
            connected: false,
            peer_os,
            displays: vec![],
            displays_at: Instant::now() - Duration::from_secs(60),
            push: 0.0,
            pos: (0.0, 0.0),
            reclaim: 0.0,
            keys_down: HashSet::new(),
            buttons_down: HashSet::new(),
            hotkey_pressed_at: None,
            hotkey_last_tap: None,
            clipboard: Clipboard::new(),
            last_wake: None,
        }
    }

    pub fn run(mut self, rx: Receiver<Event>) {
        self.report();
        while let Ok(ev) = rx.recv() {
            match ev {
                Event::Input(i) => self.on_input(i),
                Event::Net(m) => self.on_msg(m),
                Event::Connected => {
                    self.go_local();
                    self.connected = true;
                    let name = crate::machine_name();
                    self.net.send(&Msg::Hello {
                        name,
                        os: Os::current(),
                        macs: crate::wol::own_macs(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    });
                    self.report();
                }
                Event::Disconnected => {
                    self.connected = false;
                    self.go_local();
                    self.report();
                }
                Event::Ui(u) => self.on_ui(u),
            }
        }
    }

    fn report(&self) {
        let c = self.cfg.read().unwrap();
        (self.on_status)(Status {
            peer: c.peer.as_ref().map(|p| p.name.clone()),
            connected: self.connected,
            mode: self.mode,
            paused: c.paused,
        });
    }

    fn displays(&mut self) -> &[Rect] {
        if self.displays_at.elapsed() > Duration::from_secs(2) || self.displays.is_empty() {
            self.displays = self.plat.displays();
            self.displays_at = Instant::now();
        }
        &self.displays
    }

    fn on_ui(&mut self, u: Ui) {
        match u {
            Ui::TogglePause => {
                let mut c = self.cfg.write().unwrap();
                c.paused = !c.paused;
                c.save();
                drop(c);
                self.report();
            }
            Ui::Switch => self.switch(),
            Ui::Wake => {
                self.last_wake = None;
                self.wake();
            }
            Ui::Quit => {
                if self.mode == Mode::Controlling {
                    self.net.send(&Msg::Leave { frac: None });
                }
                self.go_local();
                std::process::exit(0);
            }
            Ui::Repaired => {
                self.peer_os = self.cfg.read().unwrap().peer.as_ref().map(|p| p.os);
                self.net.reset();
                self.report();
            }
        }
    }

    fn wake(&mut self) {
        // Pushing at the edge of a sleeping machine shouldn't flood the network.
        if self.last_wake.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
            return;
        }
        self.last_wake = Some(Instant::now());
        let macs = self.cfg.read().unwrap().peer.as_ref().map(|p| p.macs.clone()).unwrap_or_default();
        if macs.is_empty() {
            log::info!("can't wake the other machine: its network address isn't known yet (connect once first)");
        }
        crate::wol::wake(&macs);
    }

    /// The hotkey: hand over, or take back.
    fn switch(&mut self) {
        match self.mode {
            Mode::Local => self.hand_over(None),
            Mode::Controlling => {
                self.net.send(&Msg::Leave { frac: None });
                self.go_local();
                self.report();
            }
            Mode::Controlled => self.give_back(None),
        }
    }

    fn hotkey(&mut self, hid: u16, down: bool) -> bool {
        let key = self.cfg.read().unwrap().hotkey;
        if hid != key {
            if down {
                self.hotkey_last_tap = None;
                self.hotkey_pressed_at = None;
            }
            return false;
        }
        let now = Instant::now();
        if down {
            self.hotkey_pressed_at = Some(now);
            return false;
        }
        let quick = self.hotkey_pressed_at.take().is_some_and(|t| now - t < Duration::from_millis(300));
        if !quick {
            self.hotkey_last_tap = None;
            return false;
        }
        if self.hotkey_last_tap.is_some_and(|t| now - t < Duration::from_millis(400)) {
            self.hotkey_last_tap = None;
            return true;
        }
        self.hotkey_last_tap = Some(now);
        false
    }

    fn on_input(&mut self, i: Input) {
        if let Input::Key { hid, down } = i {
            if self.hotkey(hid, down) {
                self.switch();
                return;
            }
        }
        match self.mode {
            Mode::Local => {
                if let Input::Move { x, y, dx, dy } = i {
                    self.watch_edge(x, y, dx, dy);
                }
            }
            Mode::Controlling => {
                let m = match i {
                    Input::Move { dx, dy, .. } => Msg::MouseMove { dx, dy },
                    Input::Button { button, down } => Msg::MouseButton { button, down },
                    Input::Scroll { dx, dy } => Msg::Scroll { dx, dy },
                    Input::Key { hid, down } => Msg::Key { hid, down },
                    Input::Media(m) => Msg::Media(m),
                };
                self.net.send(&m);
            }
            Mode::Controlled => {
                // Someone grabbed this machine's own mouse: give it back to them.
                if let Input::Move { dx, dy, .. } = i {
                    self.reclaim += dx.abs() + dy.abs();
                    if self.reclaim > 60.0 {
                        self.give_back(None);
                    }
                }
            }
        }
    }

    fn watch_edge(&mut self, x: f64, y: f64, dx: f64, dy: f64) {
        let (side, resistance, paused) = {
            let c = self.cfg.read().unwrap();
            (c.peer_side, c.edge_resistance, c.paused)
        };
        if paused || self.cfg.read().unwrap().peer.is_none() {
            return;
        }
        let ds = self.displays().to_vec();
        let out = geometry::outward(side, dx, dy);
        if !geometry::on_edge(&ds, x, y, side) || out < 0.0 {
            self.push = 0.0;
            return;
        }
        // Windows clamps the reported position at the edge, so pushing into it reads as no movement.
        let stalled = dx == 0.0 && dy == 0.0;
        if out == 0.0 && !stalled {
            return;
        }
        self.push += out.max(1.0);
        if self.push < resistance {
            return;
        }
        self.push = 0.0;
        if self.cfg.read().unwrap().block_in_fullscreen && self.plat.fullscreen_app() {
            return;
        }
        self.hand_over(Some(geometry::frac_along(&ds, x, y, side)));
    }

    fn send_clipboard(&mut self) {
        let (share, max_mb) = {
            let c = self.cfg.read().unwrap();
            (c.share_clipboard, c.max_clipboard_files_mb)
        };
        if share {
            if let Some(clip) = self.clipboard.take_new(max_mb * 1_000_000) {
                self.net.send(&Msg::Clipboard(clip));
            }
        }
    }

    fn hand_over(&mut self, frac: Option<f64>) {
        if !self.connected {
            self.wake();
            return;
        }
        self.send_clipboard();
        self.net.send(&Msg::Enter { frac });
        self.mode = Mode::Controlling;
        self.plat.set_capturing(true);
        log::info!("now driving the other machine");
        self.report();
    }

    /// We were being driven; stop, and tell the other side.
    fn give_back(&mut self, frac: Option<f64>) {
        self.release_all();
        self.send_clipboard();
        self.net.send(&Msg::Leave { frac });
        self.mode = Mode::Local;
        self.report();
    }

    fn go_local(&mut self) {
        match self.mode {
            Mode::Controlling => self.plat.set_capturing(false),
            Mode::Controlled => self.release_all(),
            Mode::Local => {}
        }
        self.mode = Mode::Local;
    }

    fn release_all(&mut self) {
        for k in std::mem::take(&mut self.keys_down) {
            self.plat.key(k, false);
        }
        for b in std::mem::take(&mut self.buttons_down) {
            self.plat.button(b, false);
        }
    }

    fn on_msg(&mut self, m: Msg) {
        match m {
            Msg::Hello { name, os, macs, version } => {
                log::info!("other side: {name} ({os:?}), Bridge {version}");
                self.peer_os = Some(os);
                let mut c = self.cfg.write().unwrap();
                if let Some(p) = c.peer.as_mut() {
                    p.name = name;
                    p.os = os;
                    if !macs.is_empty() {
                        p.macs = macs;
                    }
                }
                c.save();
                drop(c);
                self.report();
            }
            Msg::Ping => {}
            Msg::Clipboard(clip) => self.clipboard.apply(clip),
            Msg::Enter { frac } => {
                if self.mode == Mode::Controlling {
                    // Both pushed at once; the newcomer wins.
                    self.plat.set_capturing(false);
                }
                let side = self.cfg.read().unwrap().peer_side;
                let ds = self.displays().to_vec();
                let p = match frac {
                    Some(f) => geometry::entry_point(&ds, side, f),
                    None => geometry::center(&ds),
                };
                self.pos = p;
                self.reclaim = 0.0;
                self.mode = Mode::Controlled;
                self.plat.move_to(p.0, p.1);
                self.report();
            }
            Msg::Leave { frac } => {
                let was = self.mode;
                self.go_local();
                if was == Mode::Controlling {
                    if let Some(f) = frac {
                        let side = self.cfg.read().unwrap().peer_side;
                        let ds = self.displays().to_vec();
                        let (x, y) = geometry::entry_point(&ds, side, f);
                        self.plat.warp(x, y);
                    }
                } else if was == Mode::Controlled {
                    // The other side took its input back; hand it our clipboard.
                    self.send_clipboard();
                }
                self.report();
            }
            _ if self.mode != Mode::Controlled => {}
            Msg::MouseMove { dx, dy } => {
                let (scale, side) = {
                    let c = self.cfg.read().unwrap();
                    (c.incoming_pointer_scale, c.peer_side)
                };
                let ds = self.displays().to_vec();
                match geometry::step(&ds, self.pos.0, self.pos.1, dx * scale, dy * scale, side) {
                    Step::Move(x, y) => {
                        self.pos = (x, y);
                        self.reclaim = 0.0;
                        self.plat.move_to(x, y);
                    }
                    Step::Exit(f) => {
                        if self.buttons_down.is_empty() {
                            self.give_back(Some(f));
                        }
                    }
                }
            }
            Msg::MouseButton { button, down } => {
                if down {
                    self.buttons_down.insert(button);
                } else {
                    self.buttons_down.remove(&button);
                }
                self.plat.button(button, down);
            }
            Msg::Scroll { dx, dy } => {
                let c = self.cfg.read().unwrap();
                let s = c.incoming_scroll_scale * if c.invert_incoming_scroll { -1.0 } else { 1.0 };
                drop(c);
                self.plat.scroll(dx * s, dy * s);
            }
            Msg::Key { hid, down } => {
                let swap = self.cfg.read().unwrap().swap_cmd_ctrl && self.peer_os.is_some_and(|o| o != Os::current());
                let hid = if swap { keymap::swap_cmd_ctrl(hid) } else { hid };
                if down {
                    self.keys_down.insert(hid);
                } else {
                    self.keys_down.remove(&hid);
                }
                self.plat.key(hid, down);
            }
            Msg::Media(m) => self.plat.media(m),
        }
    }
}

/// Two complete Bridges (engine + encrypted link) on localhost, with fake screens that record
/// what would have been injected.
#[cfg(test)]
mod e2e {
    use super::*;
    use crate::config::Peer;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        log: Mutex<Vec<String>>,
    }

    impl Platform for Fake {
        fn displays(&self) -> Vec<Rect> {
            vec![Rect { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 }]
        }
        fn cursor(&self) -> (f64, f64) {
            (0.0, 0.0)
        }
        fn set_capturing(&self, on: bool) {
            self.log.lock().unwrap().push(format!("capture {on}"));
        }
        fn warp(&self, x: f64, y: f64) {
            self.log.lock().unwrap().push(format!("warp {x:.0},{y:.0}"));
        }
        fn move_to(&self, x: f64, y: f64) {
            self.log.lock().unwrap().push(format!("move {x:.0},{y:.0}"));
        }
        fn button(&self, b: u8, down: bool) {
            self.log.lock().unwrap().push(format!("button {b} {down}"));
        }
        fn scroll(&self, dx: f64, dy: f64) {
            self.log.lock().unwrap().push(format!("scroll {dx:.0},{dy:.0}"));
        }
        fn key(&self, hid: u16, down: bool) {
            self.log.lock().unwrap().push(format!("key {hid:#x} {down}"));
        }
        fn media(&self, m: crate::proto::Media) {
            self.log.lock().unwrap().push(format!("media {m:?}"));
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    struct Side {
        tx: Sender<Event>,
        fake: Arc<Fake>,
    }

    impl Side {
        fn input(&self, i: Input) {
            self.tx.send(Event::Input(i)).unwrap();
        }
        fn has(&self, needle: &str) -> bool {
            self.fake.log.lock().unwrap().iter().any(|l| l == needle)
        }
        fn wait_for(&self, needle: &str) {
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(10) {
                if self.has(needle) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            panic!("never saw {needle:?}; log: {:?}", self.fake.log.lock().unwrap());
        }
    }

    fn pair_up() -> (Side, Side) {
        let gen = || snow::Builder::new(crate::net::NOISE.parse().unwrap()).generate_keypair().unwrap();
        let (ka, kb) = (gen(), gen());
        let (pa, pb) = (free_port(), free_port());
        let mk = |me: &snow::Keypair, them: &snow::Keypair, my_port, their_port, side| Config {
            private_key: hex::encode(&me.private),
            public_key: hex::encode(&me.public),
            peer: Some(Peer {
                name: "other".into(),
                os: Os::current(),
                public_key: hex::encode(&them.public),
                addr: Some("127.0.0.1".into()),
                macs: vec![],
                port: their_port,
            }),
            peer_side: side,
            port: my_port,
            discovery: false,
            share_clipboard: false,
            ephemeral: true,
            ..Default::default()
        };
        let start = |cfg: Config| {
            let cfg = Arc::new(RwLock::new(cfg));
            let (tx, rx) = channel();
            let net = Net::start(cfg.clone(), tx.clone()).unwrap();
            let fake = Arc::new(Fake::default());
            let engine = Engine::new(cfg, net, fake.clone(), Box::new(|_| {}));
            std::thread::spawn(move || engine.run(rx));
            Side { tx, fake }
        };
        // A (think: the Mac) has the PC to its right; B has the Mac to its left.
        let a = start(mk(&ka, &kb, pa, pb, crate::proto::Side::Right));
        let b = start(mk(&kb, &ka, pb, pa, crate::proto::Side::Left));
        (a, b)
    }

    fn connected(a: &Side, b: &Side) {
        // Hello carries the version; give the dialer a few seconds to find its peer.
        let t = Instant::now();
        loop {
            a.input(Input::Move { x: 999.0, y: 400.0, dx: 40.0, dy: 0.0 });
            std::thread::sleep(Duration::from_millis(150));
            if b.fake.log.lock().unwrap().iter().any(|l| l.starts_with("move ")) {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "never connected");
        }
    }

    #[test]
    fn full_round_trip() {
        let (a, b) = pair_up();

        // Push into A's right edge: B's pointer appears at its left edge, same height.
        connected(&a, &b);
        a.wait_for("capture true");
        b.wait_for("move 2,400");

        // A's input now drives B.
        a.input(Input::Move { x: 0.0, y: 0.0, dx: 100.0, dy: 50.0 });
        b.wait_for("move 102,450");
        a.input(Input::Button { button: 0, down: true });
        a.input(Input::Button { button: 0, down: false });
        b.wait_for("button 0 true");
        b.wait_for("button 0 false");
        a.input(Input::Key { hid: 0x06, down: true });
        a.input(Input::Key { hid: 0x06, down: false });
        b.wait_for("key 0x6 true");
        b.wait_for("key 0x6 false");
        a.input(Input::Scroll { dx: 0.0, dy: 40.0 });
        b.wait_for("scroll 0,40");
        a.input(Input::Media(crate::proto::Media::PlayPause));
        b.wait_for("media PlayPause");

        // A key held when control comes back is released, not left stuck.
        a.input(Input::Key { hid: 0xE1, down: true });
        b.wait_for("key 0xe1 true");

        // Pushing off B's left edge hands control back to A, at the same height.
        a.input(Input::Move { x: 0.0, y: 0.0, dx: -500.0, dy: 0.0 });
        b.wait_for("key 0xe1 false");
        a.wait_for("capture false");
        a.wait_for("warp 997,450");

        // Back home, A's input stays on A.
        let before = b.fake.log.lock().unwrap().len();
        a.input(Input::Key { hid: 0x04, down: true });
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(b.fake.log.lock().unwrap().len(), before, "input leaked to the other machine");
    }

    #[test]
    fn hotkey_and_reclaim() {
        let (a, b) = pair_up();
        connected(&a, &b);
        a.wait_for("capture true");

        // Double-tap the hotkey on A (Right Option on a Mac, Right Ctrl on a PC) takes control back.
        let key = Config::default().hotkey;
        let tap = |s: &Side| {
            s.input(Input::Key { hid: key, down: true });
            s.input(Input::Key { hid: key, down: false });
        };
        tap(&a);
        tap(&a);
        a.wait_for("capture false");

        // Double-tap again: switch over without touching the edge, landing mid-screen.
        tap(&a);
        tap(&a);
        b.wait_for("move 500,400");
        let captures = |s: &Side| s.fake.log.lock().unwrap().iter().filter(|l| *l == "capture true").count();
        assert_eq!(captures(&a), 2);

        // Someone moves B's own mouse: B takes itself back and A stops capturing.
        b.input(Input::Move { x: 500.0, y: 400.0, dx: 80.0, dy: 0.0 });
        let t = Instant::now();
        while a.fake.log.lock().unwrap().iter().filter(|l| *l == "capture false").count() < 2 {
            assert!(t.elapsed() < Duration::from_secs(5), "A never released: {:?}", a.fake.log.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
