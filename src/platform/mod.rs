use crate::engine::Event;
use crate::proto::{Button, Media};
use std::sync::mpsc::Sender;
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// Physical input seen on this machine. Events the app injects itself never show up here.
#[derive(Debug, Clone, Copy)]
pub enum Input {
    /// Pointer position (screen coordinates) and the movement that produced it.
    Move { x: f64, y: f64, dx: f64, dy: f64 },
    Button { button: Button, down: bool },
    Scroll { dx: f64, dy: f64 },
    Key { hid: u16, down: bool },
    Media(Media),
}

pub trait Platform: Send + Sync {
    /// Every active display, in the global coordinate space the pointer moves in.
    fn displays(&self) -> Vec<Rect>;
    /// Current pointer position (used by `bridge selftest`).
    fn cursor(&self) -> (f64, f64);
    /// On: local input is swallowed (still reported as Input) and the pointer is frozen and hidden.
    fn set_capturing(&self, on: bool);
    /// Move the pointer without it counting as user input.
    fn warp(&self, x: f64, y: f64);
    /// Injected input, as if it came from a real device here.
    fn move_to(&self, x: f64, y: f64);
    fn button(&self, button: Button, down: bool);
    fn scroll(&self, dx: f64, dy: f64);
    fn key(&self, hid: u16, down: bool);
    fn media(&self, m: Media);
    /// A full-screen app (game, film) is in front.
    fn fullscreen_app(&self) -> bool {
        false
    }
}

/// Any panic means a thread is gone and the app can't be trusted to hand input back,
/// so give the pointer back, log it and exit (the login item restarts Bridge).
pub fn install_panic_guard(plat: Arc<dyn Platform>) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("crashed: {info}");
        plat.set_capturing(false);
        default(info);
        std::process::exit(1);
    }));
}

pub fn start(events: Sender<Event>) -> Arc<dyn Platform> {
    #[cfg(target_os = "macos")]
    return macos::start(events);
    #[cfg(windows)]
    return windows::start(events);
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = events;
        panic!("Bridge runs on macOS and Windows");
    }
}
