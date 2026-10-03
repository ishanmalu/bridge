//! The app around the engine: menu bar / tray icon, the Welcome, Pair, Settings and About
//! windows (one HTML file in a native webview), notification banners, and updates.

use crate::config::Config;
use crate::engine::{Event, Mode, Status, Ui};
use crate::{autostart, pair, updater};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event as TaoEvent, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use wry::{WebView, WebViewBuilder};

const APP_HTML: &str = include_str!("../assets/ui/app.html");
const APP_ICON: &[u8] = include_bytes!("../assets/icon-256.png");

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Page {
    Welcome,
    Pair,
    Settings,
    About,
    Banner,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Welcome => "welcome",
            Page::Pair => "pair",
            Page::Settings => "settings",
            Page::About => "about",
            Page::Banner => "banner",
        }
    }
    fn from_name(s: &str) -> Option<Page> {
        Some(match s {
            "welcome" => Page::Welcome,
            "pair" => Page::Pair,
            "settings" => Page::Settings,
            "about" => Page::About,
            _ => return None,
        })
    }
    fn size(self) -> (f64, f64) {
        match self {
            Page::Welcome => (520.0, 520.0),
            Page::Pair => (460.0, 520.0),
            Page::Settings => (560.0, 680.0),
            Page::About => (400.0, 520.0),
            Page::Banner => (380.0, 92.0),
        }
    }
    fn title(self) -> &'static str {
        match self {
            Page::Welcome => "Welcome to Bridge",
            Page::Pair => "Pair",
            Page::Settings => "Bridge Settings",
            Page::About => "About Bridge",
            Page::Banner => "Bridge",
        }
    }
}

pub enum UserEvent {
    Status(Status),
    Menu(MenuEvent),
    Ipc(Page, String),
    Push(Page, Value),
    Notify { title: String, body: String, action: Option<Page> },
}

static PROXY: OnceLock<Mutex<EventLoopProxy<UserEvent>>> = OnceLock::new();
static PENDING_BANNER: Mutex<Option<Value>> = Mutex::new(None);

fn post(e: UserEvent) {
    if let Some(p) = PROXY.get() {
        let _ = p.lock().unwrap().send_event(e);
    }
}

fn push(page: Page, v: Value) {
    post(UserEvent::Push(page, v));
}

/// A Bridge banner (falls back to the terminal when there's no app running).
pub fn notify(msg: &str) {
    log::info!("{msg}");
    if PROXY.get().is_some() {
        post(UserEvent::Notify { title: "Bridge".into(), body: msg.into(), action: None });
    } else {
        println!("{msg}");
    }
}

/// For command-line runs that have no window or console (the Windows uninstaller).
pub fn notify_cli(msg: &str) {
    println!("{msg}");
    #[cfg(windows)]
    unsafe {
        use windows::core::PCWSTR;
        use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK};
        let m: Vec<u16> = msg.encode_utf16().chain(Some(0)).collect();
        let t: Vec<u16> = "Bridge".encode_utf16().chain(Some(0)).collect();
        MessageBoxW(None, PCWSTR(m.as_ptr()), PCWSTR(t.as_ptr()), MB_OK);
    }
}

fn decode_png(bytes: &[u8]) -> (Vec<u8>, u32, u32) {
    let dec = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (buf, info.width, info.height)
}

#[derive(Clone, Copy, PartialEq)]
enum TrayState {
    Off,
    On,
    Driving,
}

fn tray_icon(state: TrayState) -> Icon {
    #[cfg(target_os = "macos")]
    let bytes: &[u8] = match state {
        TrayState::Off => include_bytes!("../assets/tray/mac-off.png"),
        TrayState::On => include_bytes!("../assets/tray/mac.png"),
        TrayState::Driving => include_bytes!("../assets/tray/mac-driving.png"),
    };
    #[cfg(not(target_os = "macos"))]
    let bytes: &[u8] = match state {
        TrayState::Off => include_bytes!("../assets/tray/win-off.png"),
        TrayState::On => include_bytes!("../assets/tray/win.png"),
        TrayState::Driving => include_bytes!("../assets/tray/win-driving.png"),
    };
    let (rgba, w, h) = decode_png(bytes);
    Icon::from_rgba(rgba, w, h).unwrap()
}

fn os_name() -> &'static str {
    if cfg!(target_os = "macos") { "mac" } else { "win" }
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).creation_flags(0x0800_0000).spawn();
    }
}

fn open_path(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("notepad").arg(path).spawn();
}

/// Plain-language versions of what can go wrong while pairing.
fn friendly(e: &str) -> String {
    if e == "wrong code" || e == "pairing failed" {
        "That code didn't match. Check the digits on the other machine and try again.".into()
    } else if e.contains("too many wrong codes") {
        "Too many wrong tries. A new code is showing; use that one.".into()
    } else if e.contains("couldn't find") {
        "Couldn't find the other machine. Make sure it's showing a code, or enter its address.".into()
    } else if e.contains("could not reach") || e.contains("refused") || e.contains("timed out") {
        "Couldn't reach that machine. Check it's showing a code and both are on the same network.".into()
    } else {
        e.to_string()
    }
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

struct Panel {
    window: Window,
    webview: WebView,
}

struct App {
    cfg: Arc<RwLock<Config>>,
    tx: Sender<Event>,
    panels: HashMap<Page, Panel>,
    by_window: HashMap<WindowId, Page>,
    status: Option<Status>,
    banner_until: Option<Instant>,
    banner_closing: Option<Instant>,
    banner_action: Option<Page>,
    release: Arc<Mutex<Option<updater::Release>>>,
    next_poll: Instant,
}

impl App {
    fn open(&mut self, target: &EventLoopWindowTarget<UserEvent>, page: Page) {
        if let Some(p) = self.panels.get(&page) {
            if page != Page::Banner {
                p.window.set_visible(true);
                p.window.set_focus();
                crate::platform::activate_app();
            }
            return;
        }
        let (w, h) = page.size();
        let banner = page == Page::Banner;
        let mut wb = WindowBuilder::new()
            .with_title(page.title())
            .with_inner_size(LogicalSize::new(w, h))
            .with_resizable(page == Page::Settings)
            .with_minimizable(!banner)
            .with_maximizable(false)
            .with_visible(false);
        if page == Page::Settings {
            wb = wb.with_min_inner_size(LogicalSize::new(480.0, 420.0));
        }
        if banner {
            wb = wb.with_decorations(false).with_transparent(true).with_always_on_top(true).with_focused(false);
            if let Some(m) = target.primary_monitor() {
                let scale = m.scale_factor();
                let pos = m.position().to_logical::<f64>(scale);
                let size = m.size().to_logical::<f64>(scale);
                let x = pos.x + size.width - w - 12.0;
                // Mac: under the menu bar, like system notifications. Windows: above the taskbar.
                let y = if cfg!(target_os = "macos") { pos.y + 34.0 } else { pos.y + size.height - h - 60.0 };
                wb = wb.with_position(LogicalPosition::new(x, y));
            }
        }
        #[cfg(target_os = "macos")]
        {
            use tao::platform::macos::WindowBuilderExtMacOS;
            if !banner {
                wb = wb.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true);
            }
        }
        #[cfg(windows)]
        {
            use tao::platform::windows::WindowBuilderExtWindows;
            let (rgba, iw, ih) = decode_png(include_bytes!("../assets/tray/win.png"));
            wb = wb.with_window_icon(tao::window::Icon::from_rgba(rgba, iw, ih).ok());
            if banner {
                wb = wb.with_skip_taskbar(true);
            }
        }
        let Ok(window) = wb.build(target) else { return };
        if !banner {
            if let Some(m) = window.current_monitor() {
                let scale = m.scale_factor();
                let ms = m.size().to_logical::<f64>(scale);
                let mp = m.position().to_logical::<f64>(scale);
                window.set_outer_position(LogicalPosition::new(mp.x + (ms.width - w) / 2.0, mp.y + (ms.height - h) / 2.5));
            }
        }
        let init = json!({
            "page": page.name(),
            "os": os_name(),
            "icon": format!("data:image/png;base64,{}", base64_encode(APP_ICON)),
            "autoCheck": page == Page::About,
        });
        let proxy = PROXY.get().unwrap().lock().unwrap().clone();
        let built = WebViewBuilder::new()
            .with_html(APP_HTML)
            .with_initialization_script(format!("window.BRIDGE = {init};"))
            .with_transparent(banner)
            .with_devtools(cfg!(debug_assertions))
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(UserEvent::Ipc(page, req.body().clone()));
            })
            .build(&window);
        let webview = match built {
            Ok(w) => w,
            Err(e) => {
                log::error!("couldn't open the {} window: {e}", page.name());
                return;
            }
        };
        window.set_visible(true);
        if !banner {
            window.set_focus();
            crate::platform::activate_app();
        }
        self.by_window.insert(window.id(), page);
        self.panels.insert(page, Panel { window, webview });
    }

    fn close(&mut self, page: Page) {
        if let Some(p) = self.panels.remove(&page) {
            self.by_window.remove(&p.window.id());
            if page == Page::Pair {
                pair::cancel();
            }
            if page == Page::Banner {
                self.banner_until = None;
                self.banner_closing = None;
            }
        }
    }

    fn send(&self, page: Page, v: &Value) {
        if let Some(p) = self.panels.get(&page) {
            let _ = p.webview.evaluate_script(&format!("window.bridge && window.bridge.recv({v})"));
        }
    }

    fn welcome_state(&self) -> Value {
        json!({
            "type": "welcome",
            "installed": crate::install::installed(),
            "trusted": crate::platform::accessibility_trusted(),
            "paired": self.cfg.read().unwrap().peer.is_some(),
        })
    }

    fn settings_state(&self) -> Value {
        let c = self.cfg.read().unwrap();
        let mut cfg = serde_json::to_value(&*c).unwrap_or(json!({}));
        if let Some(o) = cfg.as_object_mut() {
            o.remove("private_key");
            o.remove("public_key");
            o.remove("peer");
        }
        json!({
            "type": "settings",
            "cfg": cfg,
            "version": env!("CARGO_PKG_VERSION"),
            "autostart": autostart::enabled(),
            "peer": c.peer.as_ref().map(|p| json!({"name": p.name, "os": p.os})),
            "connected": self.status.as_ref().is_some_and(|s| s.connected),
        })
    }

    fn refresh(&self, page: Page) {
        match page {
            Page::Welcome => self.send(page, &self.welcome_state()),
            Page::Settings => self.send(page, &self.settings_state()),
            Page::About => self.send(page, &json!({"type": "about", "version": env!("CARGO_PKG_VERSION")})),
            Page::Pair => self.send(
                page,
                &json!({"type": "me", "name": crate::machine_name(), "ip": pair::local_ip().map(|i| i.to_string())}),
            ),
            Page::Banner => {}
        }
    }

    fn banner(&mut self, target: &EventLoopWindowTarget<UserEvent>, title: &str, body: &str, action: Option<Page>) {
        let fresh = !self.panels.contains_key(&Page::Banner);
        let msg = json!({"type": "banner", "title": title, "body": body});
        if fresh {
            // The page isn't loaded yet; it asks for this once it is.
            *PENDING_BANNER.lock().unwrap() = Some(msg.clone());
        }
        self.open(target, Page::Banner);
        self.banner_action = action;
        self.banner_until = Some(Instant::now() + Duration::from_millis(if action.is_some() { 9000 } else { 4800 }));
        self.banner_closing = None;
        if !fresh {
            self.send(Page::Banner, &msg);
        }
    }

    fn set(&mut self, key: &str, value: Value) {
        // Keys and the pairing are never writable from the UI.
        if matches!(key, "private_key" | "public_key" | "peer" | "ephemeral") {
            return;
        }
        let mut c = self.cfg.write().unwrap();
        let mut v = serde_json::to_value(&*c).unwrap();
        v[key] = value;
        match serde_json::from_value::<Config>(v) {
            Ok(mut next) => {
                next.ephemeral = c.ephemeral;
                *c = next;
                c.save();
            }
            Err(e) => log::warn!("rejected setting {key}: {e}"),
        }
        drop(c);
        if key == "paused" {
            let _ = self.tx.send(Event::Ui(Ui::Refresh));
        }
    }

    fn ipc(&mut self, target: &EventLoopWindowTarget<UserEvent>, page: Page, body: &str) {
        let Ok(m) = serde_json::from_str::<Value>(body) else { return };
        match m["cmd"].as_str().unwrap_or("") {
            "ready" => {
                if page == Page::Banner {
                    if let Some(msg) = PENDING_BANNER.lock().unwrap().take() {
                        self.send(Page::Banner, &msg);
                    }
                }
                self.refresh(page);
            }
            "close" => self.close(page),
            "drag" => {
                if let Some(p) = self.panels.get(&page) {
                    let _ = p.window.drag_window();
                }
            }
            "open" => {
                if let Some(p) = m["page"].as_str().and_then(Page::from_name) {
                    self.open(target, p);
                }
            }
            "url" => {
                if let Some(u) = m["url"].as_str().filter(|u| u.starts_with("https://")) {
                    open_url(u);
                }
            }
            "openLog" => open_path(&crate::log_path()),
            "openAccessibility" => open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"),
            "set" => {
                if let Some(k) = m["key"].as_str() {
                    self.set(k, m["value"].clone());
                }
            }
            "autostart" => {
                if let Err(e) = autostart::set(m["value"].as_bool().unwrap_or(false)) {
                    notify(&format!("Couldn't change start at login: {e}"));
                }
                self.refresh(Page::Settings);
            }
            "unpair" => {
                let mut c = self.cfg.write().unwrap();
                c.peer = None;
                c.save();
                drop(c);
                let _ = self.tx.send(Event::Ui(Ui::Repaired));
                self.refresh(Page::Settings);
            }
            "host" => start_host(self.cfg.clone(), self.tx.clone()),
            "cancelHost" => pair::cancel(),
            "join" => {
                let code = m["code"].as_str().unwrap_or("").to_string();
                let addr = m["addr"].as_str().map(str::to_string);
                start_join(self.cfg.clone(), self.tx.clone(), code, addr);
            }
            "install" => {
                std::thread::spawn(|| {
                    #[cfg(windows)]
                    let r = crate::install::elevate("install").map(|_| {
                        // The elevated copy installs and starts; this one makes way.
                        std::thread::sleep(Duration::from_millis(300));
                        std::process::exit(0)
                    });
                    #[cfg(not(windows))]
                    let r = crate::install::install();
                    if let Err(e) = r {
                        notify(&format!("Couldn't install: {e}"));
                    }
                });
            }
            #[cfg(windows)]
            "uninstall" => {
                if crate::install::elevate("uninstall").is_ok() {
                    std::process::exit(0);
                }
            }
            "check" => {
                let slot = self.release.clone();
                std::thread::spawn(move || {
                    let msg = match updater::check() {
                        Ok(Some(r)) => {
                            let v = json!({"type": "update", "available": true, "version": r.version, "notes": r.notes});
                            *slot.lock().unwrap() = Some(r);
                            v
                        }
                        Ok(None) => json!({"type": "update", "available": false}),
                        Err(e) => json!({"type": "update", "error": format!("Couldn't check: {e}")}),
                    };
                    push(Page::About, msg);
                });
            }
            "installUpdate" => {
                let slot = self.release.clone();
                std::thread::spawn(move || {
                    let Some(r) = slot.lock().unwrap().clone() else { return };
                    let res = updater::install(&r, |p| push(Page::About, json!({"type": "progress", "p": p})));
                    if let Err(e) = res {
                        push(Page::About, json!({"type": "update", "error": e}));
                    }
                });
            }
            "bannerClick" => {
                let action = self.banner_action.take();
                self.close(Page::Banner);
                if let Some(p) = action {
                    self.open(target, p);
                }
            }
            _ => {}
        }
    }
}

fn paired(cfg: &Arc<RwLock<Config>>, tx: &Sender<Event>, peer: crate::config::Peer) {
    let name = peer.name.clone();
    pair::save(&mut cfg.write().unwrap(), peer);
    let _ = tx.send(Event::Ui(Ui::Repaired));
    push(Page::Pair, json!({"type": "paired", "name": name}));
    push(Page::Settings, json!({"type": "refresh"}));
    push(Page::Welcome, json!({"type": "refresh"}));
    notify(&format!("Paired with {name}. Push the pointer off the edge to cross."));
}

fn start_host(cfg: Arc<RwLock<Config>>, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let snapshot = cfg.read().unwrap().clone();
        let code = pair::new_code();
        let shown = code.clone();
        let res = pair::host(&snapshot, &code, move || {
            push(Page::Pair, json!({"type": "code", "code": shown, "ip": pair::local_ip().map(|i| i.to_string())}))
        });
        match res {
            Ok(peer) => paired(&cfg, &tx, peer),
            Err(e) if e == pair::REPLACED => {}
            Err(e) if e.contains("too many") => {
                push(Page::Pair, json!({"type": "pairError", "msg": friendly(&e)}));
                start_host(cfg, tx); // a fresh code straight away
            }
            Err(e) => push(Page::Pair, json!({"type": "pairError", "msg": friendly(&e)})),
        }
    });
}

fn start_join(cfg: Arc<RwLock<Config>>, tx: Sender<Event>, code: String, addr: Option<String>) {
    std::thread::spawn(move || {
        // We're the one typing, so stop showing a code of our own.
        pair::cancel();
        let snapshot = cfg.read().unwrap().clone();
        match pair::join(&snapshot, &code, addr.as_deref()) {
            Ok(peer) => paired(&cfg, &tx, peer),
            Err(e) => push(Page::Pair, json!({"type": "pairError", "msg": friendly(&e)})),
        }
    });
}

fn first_run_needed(cfg: &Config) -> bool {
    cfg.peer.is_none() || !crate::install::installed() || !crate::platform::accessibility_trusted()
}

pub fn run() {
    let cfg = Arc::new(RwLock::new(Config::load()));
    let (tx, rx) = channel::<Event>();
    updater::clean_up();

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let _ = PROXY.set(Mutex::new(event_loop.create_proxy()));
    MenuEvent::set_event_handler(Some(move |e| post(UserEvent::Menu(e))));

    // Another copy may still be shutting down (after an update or install): give it a moment.
    let started = Instant::now();
    let net = loop {
        match crate::net::Net::start(cfg.clone(), tx.clone()) {
            Ok(n) => break n,
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && started.elapsed() < Duration::from_secs(5) => {
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                log::warn!("port in use: Bridge is already running");
                notify_cli("Bridge is already running. Look for it in the menu bar or tray.");
                std::process::exit(0);
            }
            Err(e) => {
                notify_cli(&format!("Bridge can't listen for the other machine: {e}"));
                std::process::exit(1);
            }
        }
    };
    let plat = crate::platform::start(tx.clone());
    crate::platform::install_panic_guard(plat.clone());
    let engine = crate::engine::Engine::new(cfg.clone(), net, plat, Box::new(|s| post(UserEvent::Status(s))));
    std::thread::Builder::new().name("engine".into()).spawn(move || engine.run(rx)).unwrap();

    // Update check shortly after launch, then daily.
    let release = Arc::new(Mutex::new(None));
    {
        let (cfg, slot) = (cfg.clone(), release.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(20));
            if cfg.read().unwrap().check_updates {
                if let Ok(Some(r)) = updater::check() {
                    let v = r.version.clone();
                    *slot.lock().unwrap() = Some(r);
                    post(UserEvent::Notify {
                        title: format!("Bridge {v} is available"),
                        body: "Click to see what's new and install it.".into(),
                        action: Some(Page::About),
                    });
                }
            }
            std::thread::sleep(Duration::from_secs(24 * 3600));
        });
    }

    let status = MenuItem::new("Starting…", false, None);
    let switch = MenuItem::new("Switch now", true, None);
    let pause = CheckMenuItem::new("Pause crossing", true, cfg.read().unwrap().paused, None);
    let wake = MenuItem::new("Wake the other machine", true, None);
    let pair_item = MenuItem::new("Pair…", true, None);
    let settings = MenuItem::new("Settings…", true, None);
    let updates = MenuItem::new("Check for updates…", true, None);
    let quit = MenuItem::new("Quit Bridge", true, None);
    let menu = Menu::with_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &switch,
        &pause,
        &wake,
        &PredefinedMenuItem::separator(),
        &pair_item,
        &settings,
        &updates,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .unwrap();

    let mut app = App {
        cfg: cfg.clone(),
        tx: tx.clone(),
        panels: HashMap::new(),
        by_window: HashMap::new(),
        status: None,
        banner_until: None,
        banner_closing: None,
        banner_action: None,
        release,
        next_poll: Instant::now(),
    };
    let mut tray = None;
    let mut tray_state = None;

    event_loop.run(move |event, target, flow| {
        match event {
            TaoEvent::NewEvents(StartCause::Init) => {
                #[allow(unused_mut)]
                let mut b = TrayIconBuilder::new()
                    .with_menu(Box::new(menu.clone()))
                    .with_tooltip("Bridge")
                    .with_icon(tray_icon(TrayState::Off));
                #[cfg(target_os = "macos")]
                {
                    b = b.with_icon_as_template(true);
                }
                tray = b.build().ok();
                if first_run_needed(&cfg.read().unwrap()) {
                    app.open(target, Page::Welcome);
                }
            }
            TaoEvent::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                let now = Instant::now();
                if app.banner_closing.is_some_and(|t| now >= t) {
                    app.close(Page::Banner);
                } else if app.banner_until.is_some_and(|t| now >= t) {
                    app.send(Page::Banner, &json!({"type": "bannerOut"}));
                    app.banner_until = None;
                    app.banner_closing = Some(now + Duration::from_millis(350));
                }
                if now >= app.next_poll {
                    app.next_poll = now + Duration::from_secs(1);
                    app.refresh(Page::Welcome);
                }
            }
            TaoEvent::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                if let Some(page) = app.by_window.get(&window_id).copied() {
                    app.close(page);
                }
            }
            TaoEvent::UserEvent(UserEvent::Ipc(page, body)) => app.ipc(target, page, &body),
            TaoEvent::UserEvent(UserEvent::Push(page, v)) => {
                if v["type"] == "refresh" {
                    app.refresh(page);
                } else {
                    app.send(page, &v);
                }
            }
            TaoEvent::UserEvent(UserEvent::Notify { title, body, action }) => app.banner(target, &title, &body, action),
            TaoEvent::UserEvent(UserEvent::Status(s)) => {
                let peer = s.peer.clone().unwrap_or_else(|| "the other machine".into());
                let text = match (&s.peer, s.connected, s.mode) {
                    (None, _, _) => "Not paired yet".to_string(),
                    (_, false, _) => format!("Looking for {peer}…"),
                    (_, true, Mode::Controlling) => format!("Driving {peer}"),
                    (_, true, Mode::Controlled) => format!("{peer} is driving this machine"),
                    (_, true, Mode::Local) if s.paused => format!("Connected to {peer} · paused"),
                    (_, true, Mode::Local) => format!("Connected to {peer}"),
                };
                status.set_text(&text);
                switch.set_text(match s.mode {
                    Mode::Local => format!("Switch to {peer}"),
                    _ => "Switch back".to_string(),
                });
                switch.set_enabled(s.connected);
                wake.set_text(format!("Wake {peer}"));
                wake.set_enabled(s.peer.is_some() && !s.connected);
                pause.set_checked(s.paused);
                let was_connected = app.status.as_ref().map(|o| o.connected);
                if was_connected == Some(false) && s.connected {
                    notify(&format!("Connected to {peer}."));
                } else if was_connected == Some(true) && !s.connected && s.peer.is_some() {
                    notify(&format!("Lost the connection to {peer}. Reconnecting…"));
                }
                let state = match (s.connected, s.mode) {
                    (false, _) => TrayState::Off,
                    (true, Mode::Controlling) => TrayState::Driving,
                    (true, _) => TrayState::On,
                };
                if let Some(t) = &tray {
                    let _ = t.set_tooltip(Some(format!("Bridge: {text}")));
                    if tray_state != Some(state) {
                        let _ = t.set_icon(Some(tray_icon(state)));
                        #[cfg(target_os = "macos")]
                        t.set_icon_as_template(true);
                        tray_state = Some(state);
                    }
                }
                app.status = Some(s);
                app.refresh(Page::Settings);
            }
            TaoEvent::UserEvent(UserEvent::Menu(e)) => {
                let id = e.id;
                let ui = |u: Ui| {
                    let _ = tx.send(Event::Ui(u));
                };
                if id == *switch.id() {
                    ui(Ui::Switch);
                } else if id == *pause.id() {
                    ui(Ui::TogglePause);
                } else if id == *wake.id() {
                    ui(Ui::Wake);
                } else if id == *pair_item.id() {
                    app.open(target, Page::Pair);
                } else if id == *settings.id() {
                    app.open(target, Page::Settings);
                } else if id == *updates.id() {
                    if app.panels.contains_key(&Page::About) {
                        app.open(target, Page::About);
                        if let Some(p) = app.panels.get(&Page::About) {
                            let _ = p.webview.evaluate_script("document.getElementById('upd-check').click()");
                        }
                    } else {
                        app.open(target, Page::About);
                    }
                } else if id == *quit.id() {
                    ui(Ui::Quit);
                }
            }
            _ => {}
        }

        // Wake up for banner timing and permission polling only when something needs it.
        let mut wake_at = app.banner_closing.or(app.banner_until);
        if app.panels.contains_key(&Page::Welcome) {
            wake_at = Some(wake_at.map_or(app.next_poll, |t| t.min(app.next_poll)));
        }
        *flow = wake_at.map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
    });
}
