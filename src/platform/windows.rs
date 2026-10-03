//! Windows: low-level keyboard and mouse hooks see (and, while driving the Mac, swallow) all input;
//! SendInput plays the Mac's input back here. Run elevated to type into admin windows too.

use super::{Input, Platform, Rect};
use crate::engine::Event;
use crate::keymap;
use crate::proto::{Button, Media};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

struct Shared {
    events: Sender<Event>,
    capturing: AtomicBool,
    center: Mutex<POINT>,
    last: Mutex<Option<POINT>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

fn send(i: Input) {
    if let Some(sh) = SHARED.get() {
        let _ = sh.events.send(Event::Input(i));
    }
}

const WHEEL_TO_PX: f64 = 40.0 / 120.0;

unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(sh) = SHARED.get() else { return CallNextHookEx(None, code, wp, lp) };
    if code < 0 {
        return CallNextHookEx(None, code, wp, lp);
    }
    let ms = &*(lp.0 as *const MSLLHOOKSTRUCT);
    if ms.flags & LLMHF_INJECTED != 0 {
        // Our own injected moves: remember where they put the pointer so real movement
        // afterwards is measured from there, not from a stale position.
        if wp.0 as u32 == WM_MOUSEMOVE {
            if let Ok(mut last) = sh.last.lock() {
                *last = Some(ms.pt);
            }
        }
        return CallNextHookEx(None, code, wp, lp);
    }
    let capturing = sh.capturing.load(Ordering::Relaxed);
    let hi = (ms.mouseData >> 16) as i16;
    match wp.0 as u32 {
        WM_MOUSEMOVE => {
            let pt = ms.pt;
            if capturing {
                let c = *sh.center.lock().unwrap();
                let (dx, dy) = (pt.x - c.x, pt.y - c.y);
                if dx != 0 || dy != 0 {
                    send(Input::Move { x: c.x as f64, y: c.y as f64, dx: dx as f64, dy: dy as f64 });
                }
            } else {
                let mut last = sh.last.lock().unwrap();
                let (dx, dy) = last.map(|l| (pt.x - l.x, pt.y - l.y)).unwrap_or((0, 0));
                *last = Some(pt);
                send(Input::Move { x: pt.x as f64, y: pt.y as f64, dx: dx as f64, dy: dy as f64 });
            }
        }
        WM_LBUTTONDOWN => send(Input::Button { button: 0, down: true }),
        WM_LBUTTONUP => send(Input::Button { button: 0, down: false }),
        WM_RBUTTONDOWN => send(Input::Button { button: 1, down: true }),
        WM_RBUTTONUP => send(Input::Button { button: 1, down: false }),
        WM_MBUTTONDOWN => send(Input::Button { button: 2, down: true }),
        WM_MBUTTONUP => send(Input::Button { button: 2, down: false }),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let b = if hi as u16 == 1 { 3 } else { 4 };
            send(Input::Button { button: b, down: wp.0 as u32 == WM_XBUTTONDOWN });
        }
        WM_MOUSEWHEEL => send(Input::Scroll { dx: 0.0, dy: hi as f64 * WHEEL_TO_PX }),
        WM_MOUSEHWHEEL => send(Input::Scroll { dx: -(hi as f64) * WHEEL_TO_PX, dy: 0.0 }),
        _ => {}
    }
    if capturing {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wp, lp)
    }
}

fn media_from_vk(vk: u32) -> Option<Media> {
    Some(match VIRTUAL_KEY(vk as u16) {
        VK_MEDIA_PLAY_PAUSE => Media::PlayPause,
        VK_MEDIA_NEXT_TRACK => Media::Next,
        VK_MEDIA_PREV_TRACK => Media::Previous,
        VK_VOLUME_UP => Media::VolumeUp,
        VK_VOLUME_DOWN => Media::VolumeDown,
        VK_VOLUME_MUTE => Media::Mute,
        _ => return None,
    })
}

unsafe extern "system" fn kbd_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(sh) = SHARED.get() else { return CallNextHookEx(None, code, wp, lp) };
    if code < 0 {
        return CallNextHookEx(None, code, wp, lp);
    }
    let kb = &*(lp.0 as *const KBDLLHOOKSTRUCT);
    if kb.flags.0 & LLKHF_INJECTED.0 != 0 {
        return CallNextHookEx(None, code, wp, lp);
    }
    let capturing = sh.capturing.load(Ordering::Relaxed);
    let down = kb.flags.0 & LLKHF_UP.0 == 0;
    if let Some(m) = media_from_vk(kb.vkCode) {
        if down {
            send(Input::Media(m));
        }
    } else {
        let hid = match VIRTUAL_KEY(kb.vkCode as u16) {
            VK_NUMLOCK => Some(0x53),
            VK_PAUSE => Some(0x48),
            _ => {
                let ext = if kb.flags.0 & LLKHF_EXTENDED.0 != 0 { 0xE000 } else { 0 };
                keymap::hid_from_win(kb.scanCode as u16 | ext)
            }
        };
        if let Some(hid) = hid {
            send(Input::Key { hid, down });
        }
    }
    if capturing {
        LRESULT(1)
    } else {
        CallNextHookEx(None, code, wp, lp)
    }
}

const SYSTEM_CURSORS: [u32; 13] = [32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650];

fn hide_cursor() {
    unsafe {
        let and = [0xFFu8; 128];
        let xor = [0u8; 128];
        let Ok(blank) = CreateCursor(None, 0, 0, 32, 32, and.as_ptr() as _, xor.as_ptr() as _) else { return };
        for id in SYSTEM_CURSORS {
            if let Ok(copy) = CopyIcon(HICON(blank.0)) {
                let _ = SetSystemCursor(HCURSOR(copy.0), SYSTEM_CURSOR_ID(id));
            }
        }
        let _ = DestroyCursor(blank);
    }
}

fn restore_cursor() {
    unsafe {
        let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
}

/// Windows Firewall blocks incoming connections to new apps by default, which would stop the Mac
/// reaching this PC. When running as administrator, allow Bridge on private networks only.
fn allow_through_firewall() {
    use std::os::windows::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return };
    let program = format!("program={}", exe.display());
    let run = |args: &[&str]| {
        std::process::Command::new("netsh")
            .args(args)
            .creation_flags(0x0800_0000)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    // Replace any rule from an older location of the exe.
    run(&["advfirewall", "firewall", "delete", "rule", "name=Bridge"]);
    let ok = run(&[
        "advfirewall", "firewall", "add", "rule", "name=Bridge", "dir=in", "action=allow",
        &program, "protocol=TCP", "localport=24800-24801", "profile=private,domain",
    ]);
    if ok {
        log::info!("firewall rule for private networks in place");
    } else {
        log::warn!("couldn't add a firewall rule (not running as administrator); Windows may ask instead");
    }
}

struct Inject {
    scroll_rem: (f64, f64),
}

struct Win {
    inject: Mutex<Inject>,
}

pub fn start(events: Sender<Event>) -> Arc<dyn Platform> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // A previous run may have died with the pointer hidden.
    restore_cursor();
    allow_through_firewall();
    let _ = SHARED.set(Shared {
        events,
        capturing: AtomicBool::new(false),
        center: Mutex::new(POINT::default()),
        last: Mutex::new(None),
    });
    std::thread::Builder::new()
        .name("hooks".into())
        .spawn(|| unsafe {
            let m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0);
            let k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kbd_proc), None, 0);
            if m.is_err() || k.is_err() {
                log::error!("could not install input hooks: {:?} {:?}", m.err(), k.err());
            } else {
                log::info!("input hooks running");
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .unwrap();
    Arc::new(Win { inject: Mutex::new(Inject { scroll_rem: (0.0, 0.0) }) })
}

fn send_inputs(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn keyboard(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

impl Platform for Win {
    fn displays(&self) -> Vec<Rect> {
        unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            let out = &mut *(data.0 as *mut Vec<Rect>);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(m, &mut mi).as_bool() {
                let r = mi.rcMonitor;
                out.push(Rect {
                    x: r.left as f64,
                    y: r.top as f64,
                    w: (r.right - r.left) as f64,
                    h: (r.bottom - r.top) as f64,
                });
            }
            BOOL(1)
        }
        let mut out: Vec<Rect> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut _ as isize));
        }
        // Primary display first (it holds the origin).
        out.sort_by_key(|r| !(r.x == 0.0 && r.y == 0.0));
        out
    }


    fn cursor(&self) -> (f64, f64) {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        (p.x as f64, p.y as f64)
    }

    fn set_capturing(&self, on: bool) {
        let sh = SHARED.get().unwrap();
        if on {
            // Park the pointer mid-screen so movement in every direction registers.
            let (cx, cy) = crate::geometry::center(&self.displays());
            if let Ok(mut c) = sh.center.lock() {
                *c = POINT { x: cx as i32, y: cy as i32 };
            }
            unsafe {
                let _ = SetCursorPos(cx as i32, cy as i32);
            }
            hide_cursor();
        } else {
            restore_cursor();
            if let Ok(mut l) = sh.last.lock() {
                *l = None;
            }
        }
        sh.capturing.store(on, Ordering::SeqCst);
    }

    fn warp(&self, x: f64, y: f64) {
        unsafe {
            let _ = SetCursorPos(x as i32, y as i32);
        }
        *SHARED.get().unwrap().last.lock().unwrap() = Some(POINT { x: x as i32, y: y as i32 });
    }

    fn move_to(&self, x: f64, y: f64) {
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN) as f64,
                GetSystemMetrics(SM_YVIRTUALSCREEN) as f64,
                GetSystemMetrics(SM_CXVIRTUALSCREEN) as f64,
                GetSystemMetrics(SM_CYVIRTUALSCREEN) as f64,
            )
        };
        let nx = ((x - vx) * 65535.0 / (vw - 1.0).max(1.0)).round() as i32;
        let ny = ((y - vy) * 65535.0 / (vh - 1.0).max(1.0)).round() as i32;
        send_inputs(&[mouse(nx, ny, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)]);
    }

    fn button(&self, button: Button, down: bool) {
        let (flags, data) = match (button, down) {
            (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (0, false) => (MOUSEEVENTF_LEFTUP, 0),
            (1, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (1, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (b, d) => (if d { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, if b == 3 { 1 } else { 2 }),
        };
        send_inputs(&[mouse(0, 0, data, flags)]);
    }

    fn scroll(&self, dx: f64, dy: f64) {
        let mut st = self.inject.lock().unwrap();
        let wy = dy / WHEEL_TO_PX + st.scroll_rem.1;
        let wx = -dx / WHEEL_TO_PX + st.scroll_rem.0;
        let (iy, ix) = (wy.trunc() as i32, wx.trunc() as i32);
        st.scroll_rem = (wx - ix as f64, wy - iy as f64);
        let mut v = Vec::new();
        if iy != 0 {
            v.push(mouse(0, 0, iy, MOUSEEVENTF_WHEEL));
        }
        if ix != 0 {
            v.push(mouse(0, 0, ix, MOUSEEVENTF_HWHEEL));
        }
        if !v.is_empty() {
            send_inputs(&v);
        }
    }

    fn key(&self, hid: u16, down: bool) {
        // Num Lock and Pause share a scancode; send those by virtual key.
        let vk_only = match hid {
            0x53 => Some(VK_NUMLOCK),
            0x48 => Some(VK_PAUSE),
            _ => None,
        };
        let up = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
        if let Some(vk) = vk_only {
            send_inputs(&[keyboard(vk.0, 0, up)]);
            return;
        }
        let Some(scan) = keymap::win_from_hid(hid) else { return };
        let mut flags = KEYEVENTF_SCANCODE | up;
        if scan & 0xE000 == 0xE000 {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        send_inputs(&[keyboard(0, scan & 0xFF, flags)]);
    }

    fn media(&self, m: Media) {
        let vk = match m {
            Media::PlayPause => VK_MEDIA_PLAY_PAUSE,
            Media::Next => VK_MEDIA_NEXT_TRACK,
            Media::Previous => VK_MEDIA_PREV_TRACK,
            Media::VolumeUp => VK_VOLUME_UP,
            Media::VolumeDown => VK_VOLUME_DOWN,
            Media::Mute => VK_VOLUME_MUTE,
        };
        send_inputs(&[
            keyboard(vk.0, 0, KEYEVENTF_EXTENDEDKEY),
            keyboard(vk.0, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
        ]);
    }

    fn fullscreen_app(&self) -> bool {
        unsafe {
            let w = GetForegroundWindow();
            if w.0.is_null() || w == GetShellWindow() || w == GetDesktopWindow() {
                return false;
            }
            let mut cls = [0u16; 64];
            let n = GetClassNameW(w, &mut cls);
            let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            if cls == "Progman" || cls == "WorkerW" {
                return false;
            }
            let mut r = RECT::default();
            if GetWindowRect(w, &mut r).is_err() {
                return false;
            }
            let mon = MonitorFromWindow(w, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(mon, &mut mi).as_bool() {
                return false;
            }
            let m = mi.rcMonitor;
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
        }
    }
}
