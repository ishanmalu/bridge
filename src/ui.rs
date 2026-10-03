//! Menu bar (Mac) / tray (Windows) app: status, switch, pause, wake, pairing, start at login.

use crate::config::Config;
use crate::engine::{Engine, Event, Mode, Status, Ui};
use crate::{autostart, pair};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, RwLock};
use tao::event::Event as TaoEvent;
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIconBuilder};

enum UserEvent {
    Status(Status),
    Menu(MenuEvent),
}

/// The app icon in miniature: two screens with a pointer crossing between them.
fn icon(connected: bool) -> Icon {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4];
    let (r, g, b) = if cfg!(target_os = "macos") { (0, 0, 0) } else if connected { (0xFF, 0x5A, 0x36) } else { (0xB0, 0xB0, 0xB0) };
    let alpha = if connected || !cfg!(target_os = "macos") { 255 } else { 110 };
    let mut put = |x: usize, y: usize| {
        let i = (y * S + x) * 4;
        px[i..i + 4].copy_from_slice(&[r, g, b, alpha]);
    };
    for (x0, x1) in [(1usize, 12usize), (20, 31)] {
        for y in 7..25 {
            for x in x0..x1 {
                let edge = y < 9 || y > 22 || x < x0 + 2 || x >= x1 - 2;
                if edge {
                    put(x, y);
                }
            }
        }
    }
    // Arrow pointer, tip at (13, 10).
    for dy in 0..13usize {
        for dx in 0..=(dy * 7 / 12) {
            put(13 + dx, 10 + dy);
        }
    }
    Icon::from_rgba(px, S as u32, S as u32).unwrap()
}

pub fn notify(msg: &str) {
    log::info!("{msg}");
    #[cfg(target_os = "macos")]
    {
        let script = format!("display notification {:?} with title \"Bridge\"", msg);
        let _ = std::process::Command::new("osascript").args(["-e", &script]).spawn();
    }
    #[cfg(windows)]
    {
        let m: Vec<u16> = msg.encode_utf16().chain(Some(0)).collect();
        let t: Vec<u16> = "Bridge".encode_utf16().chain(Some(0)).collect();
        std::thread::spawn(move || unsafe {
            use windows::core::PCWSTR;
            use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_OK, MB_SETFOREGROUND};
            MessageBoxW(None, PCWSTR(m.as_ptr()), PCWSTR(t.as_ptr()), MB_OK | MB_SETFOREGROUND);
        });
    }
}

fn ask_code() -> Option<(String, Option<String>)> {
    #[cfg(target_os = "macos")]
    let out = std::process::Command::new("osascript")
        .args([
            "-e",
            r#"text returned of (display dialog "Enter the six-digit code the other machine shows (add its address after the code if it isn't found):" default answer "" with title "Bridge" buttons {"Cancel", "Pair"} default button "Pair")"#,
        ])
        .output()
        .ok()?;
    #[cfg(windows)]
    let out = {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.Interaction]::InputBox('Enter the six-digit code the other machine shows (add its address after the code if it is not found):', 'Bridge')",
            ])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?
    };
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        return None; // cancelled
    }
    let parsed = pair::parse_entry(&text);
    if parsed.is_none() {
        notify("That isn't a six-digit code.");
    }
    parsed
}

fn open(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

pub fn run() {
    let cfg = Arc::new(RwLock::new(Config::load()));
    let (tx, rx) = channel::<Event>();

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = event_loop.create_proxy();
    let p2 = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = p2.send_event(UserEvent::Menu(e));
    }));

    let net = match crate::net::Net::start(cfg.clone(), tx.clone()) {
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            log::warn!("port in use: Bridge is already running");
            notify("Bridge is already running. Look for it in the menu bar / tray.");
            std::thread::sleep(std::time::Duration::from_secs(1));
            std::process::exit(0);
        }
        Err(e) => {
            notify(&format!("Bridge can't listen for the other machine: {e}"));
            std::process::exit(1);
        }
    };
    let plat = crate::platform::start(tx.clone());
    crate::platform::install_panic_guard(plat.clone());
    let engine = Engine::new(
        cfg.clone(),
        net,
        plat,
        Box::new(move |s| {
            let _ = proxy.send_event(UserEvent::Status(s));
        }),
    );
    std::thread::Builder::new().name("engine".into()).spawn(move || engine.run(rx)).unwrap();

    let status = MenuItem::new("Starting…", false, None);
    let switch = MenuItem::new("Switch now", true, None);
    let pause = CheckMenuItem::new("Pause crossing", true, cfg.read().unwrap().paused, None);
    let wake = MenuItem::new("Wake the other machine", true, None);
    let show_code = MenuItem::new("Show a pairing code", true, None);
    let enter_code = MenuItem::new("Enter a pairing code…", true, None);
    let pair_menu = Submenu::with_items("Pair", true, &[&show_code, &enter_code]).unwrap();
    let login = CheckMenuItem::new("Start at login", true, autostart::enabled(), None);
    let settings = MenuItem::new("Open settings file", true, None);
    let logs = MenuItem::new("Open log", true, None);
    let quit = MenuItem::new("Quit Bridge", true, None);
    let menu = Menu::with_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &switch,
        &pause,
        &wake,
        &PredefinedMenuItem::separator(),
        &pair_menu,
        &login,
        &settings,
        &logs,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .unwrap();

    let mut tray = None;
    let mut last_connected = None;
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        match event {
            TaoEvent::NewEvents(tao::event::StartCause::Init) => {
                #[allow(unused_mut)]
                let mut b = TrayIconBuilder::new().with_menu(Box::new(menu.clone())).with_tooltip("Bridge").with_icon(icon(false));
                #[cfg(target_os = "macos")]
                {
                    b = b.with_icon_as_template(true);
                }
                tray = b.build().ok();
            }
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
                if let Some(t) = &tray {
                    let _ = t.set_tooltip(Some(format!("Bridge: {text}")));
                    if last_connected != Some(s.connected) {
                        let _ = t.set_icon(Some(icon(s.connected)));
                        #[cfg(target_os = "macos")]
                        t.set_icon_as_template(true);
                        last_connected = Some(s.connected);
                    }
                }
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
                } else if id == *show_code.id() {
                    start_pairing(cfg.clone(), tx.clone(), None);
                } else if id == *enter_code.id() {
                    let (cfg, tx) = (cfg.clone(), tx.clone());
                    std::thread::spawn(move || {
                        if let Some(entry) = ask_code() {
                            start_pairing(cfg, tx, Some(entry));
                        }
                    });
                } else if id == *login.id() {
                    let want = login.is_checked();
                    if let Err(e) = autostart::set(want) {
                        notify(&format!("Couldn't change start at login: {e}"));
                    }
                    login.set_checked(autostart::enabled());
                } else if id == *settings.id() {
                    open(&crate::config::path());
                } else if id == *logs.id() {
                    open(&crate::log_path());
                } else if id == *quit.id() {
                    // The engine releases everything, then exits.
                    let _ = tx.send(Event::Ui(Ui::Quit));
                }
            }
            _ => {}
        }
    });
}

fn start_pairing(cfg: Arc<RwLock<Config>>, tx: Sender<Event>, join: Option<(String, Option<String>)>) {
    std::thread::spawn(move || {
        let snapshot = cfg.read().unwrap().clone();
        let result = match join {
            Some((code, addr)) => pair::join(&snapshot, &code, addr.as_deref()),
            None => {
                let code = pair::new_code();
                pair::host(&snapshot, &code, || show_code(&code))
            }
        };
        match result {
            Ok(peer) => {
                let name = peer.name.clone();
                pair::save(&mut cfg.write().unwrap(), peer);
                let _ = tx.send(Event::Ui(Ui::Repaired));
                notify(&format!("Paired with {name}."));
            }
            Err(e) if e == pair::REPLACED => {}
            Err(e) => notify(&format!("Pairing failed: {e}")),
        }
    });
}

fn show_code(code: &str) {
    let spaced = format!("{} {}", &code[..3], &code[3..]);
    let addr = pair::local_ip().map(|ip| format!("\\n(This machine's address: {ip})")).unwrap_or_default();
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display dialog \"Pairing code:\\n\\n{spaced}\\n\\nOn the other machine choose Pair → Enter a pairing code.{addr}\" with title \"Bridge\" buttons {{\"OK\"}} default button \"OK\""
        );
        let _ = std::process::Command::new("osascript").args(["-e", &script]).spawn();
    }
    #[cfg(windows)]
    notify(&format!("Pairing code: {spaced}\n\nOn the other machine choose Pair → Enter a pairing code.{}", addr.replace("\\n", "\n")));
}
