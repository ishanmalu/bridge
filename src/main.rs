#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod autostart;
mod clipboard;
mod config;
mod engine;
mod geometry;
mod install;
mod keymap;
mod net;
mod pair;
mod platform;
mod selftest;
mod proto;
mod ui;
mod updater;
mod wol;

use config::Config;
use std::io::Write;
use std::sync::Mutex;

pub fn machine_name() -> String {
    hostname::get()
        .ok()
        .map(|h| h.to_string_lossy().trim_end_matches(".local").to_string())
        .unwrap_or_else(|| "Bridge".into())
}

struct Logger {
    file: Mutex<Option<std::fs::File>>,
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        // Our own messages, plus warnings from libraries.
        m.level() <= if m.target().starts_with("bridge") { log::Level::Info } else { log::Level::Warn }
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let line = format!("{secs} {:5} {}\n", r.level(), r.args());
        eprint!("{line}");
        if let Some(f) = self.file.lock().unwrap().as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }
    fn flush(&self) {}
}

pub fn log_path() -> std::path::PathBuf {
    config::dir().join("bridge.log")
}

fn init_logging() {
    let _ = std::fs::create_dir_all(config::dir());
    let p = log_path();
    if p.metadata().is_ok_and(|m| m.len() > 2_000_000) {
        let _ = std::fs::rename(&p, p.with_extension("old.log"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(p).ok();
    let _ = log::set_boxed_logger(Box::new(Logger { file: Mutex::new(file) }));
    log::set_max_level(log::LevelFilter::Debug);
}

const HELP: &str = "Bridge: one keyboard and mouse for a Mac and a PC.

  bridge                       run (menu bar / tray app)
  bridge pair                  show a pairing code and wait for the other machine
  bridge pair CODE [ADDRESS]   join the machine showing CODE (found automatically, or at ADDRESS)
  bridge status                show pairing and settings
  bridge autostart on|off      start at login
  bridge selftest              check this machine: screens, pointer, clipboard, network
  bridge update                install the latest version
  bridge install               Mac: move to Applications · Windows: install into Program Files
";

fn main() {
    #[cfg(windows)]
    unsafe {
        // Release builds have no console of their own; borrow the terminal's when run from one.
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match args.as_slice() {
        [] => ui::run(),
        ["pair"] => {
            let mut cfg = Config::load();
            let code = pair::new_code();
            let ip = pair::local_ip().map(|i| i.to_string()).unwrap_or_else(|| "?".into());
            println!("Pairing code: {code}   (this machine's address: {ip})\nOn the other machine run:  bridge pair {code}\nor, if it can't find this one:  bridge pair {code} {ip}\n(or use Pair → Enter a pairing code in its menu). Waiting…");
            match pair::host(&cfg, &code, || {}) {
                Ok(p) => {
                    println!("Paired with {}.", p.name);
                    pair::save(&mut cfg, p);
                }
                Err(e) => fail(&e),
            }
        }
        ["pair", code] | ["pair", code, _] => {
            let mut cfg = Config::load();
            let addr = args.get(2).copied();
            match pair::join(&cfg, code, addr) {
                Ok(p) => {
                    println!("Paired with {}.", p.name);
                    pair::save(&mut cfg, p);
                }
                Err(e) => fail(&e),
            }
        }
        ["status"] => {
            let cfg = Config::load();
            println!("This machine: {} ({:?})", machine_name(), proto::Os::current());
            match &cfg.peer {
                Some(p) => println!("Paired with:  {} ({:?}), last seen at {}", p.name, p.os, p.addr.as_deref().unwrap_or("?")),
                None => println!("Not paired. Run `bridge pair` on one machine."),
            }
            println!("Other machine is to the {:?}. Config: {}", cfg.peer_side, config::path().display());
            println!("Start at login: {}", if autostart::enabled() { "on" } else { "off" });
        }
        ["autostart", v @ ("on" | "off")] => match autostart::set(*v == "on") {
            Ok(()) => println!("Start at login: {v}"),
            Err(e) => fail(&e),
        },
        ["install"] => {
            if let Err(e) = install::install() {
                ui::notify_cli(&format!("Couldn't install Bridge: {e}"));
                std::process::exit(1);
            }
        }
        #[cfg(windows)]
        ["uninstall"] => {
            let _ = install::uninstall();
        }
        ["update"] => match updater::check() {
            Ok(Some(r)) => {
                println!("Installing Bridge {}…", r.version);
                if let Err(e) = updater::install(&r, |_| {}) {
                    fail(&e);
                }
            }
            Ok(None) => println!("Bridge {} is up to date.", env!("CARGO_PKG_VERSION")),
            Err(e) => fail(&e),
        },
        ["uitest"] => std::process::exit(ui::ui_selftest()),
        ["selftest"] => std::process::exit(selftest::run()),
        ["--version" | "-V"] => println!("bridge {}", env!("CARGO_PKG_VERSION")),
        _ => print!("{HELP}"),
    }
}

fn fail(e: &str) -> ! {
    eprintln!("error: {e}");
    std::process::exit(1)
}
