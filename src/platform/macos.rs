//! macOS: a CoreGraphics event tap sees (and, while driving the PC, swallows) all input;
//! events posted at the HID level play the PC's input back here.
//! Needs Accessibility permission (System Settings → Privacy & Security → Accessibility).

#![allow(non_upper_case_globals, non_snake_case)]

use super::{Input, Platform, Rect};
use crate::engine::Event;
use crate::keymap;
use crate::proto::{Button, Media};
use std::ffi::{c_char, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

type Ref = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct CGPoint {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGSize {
    w: f64,
    h: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

type TapCallback = extern "C" fn(proxy: Ref, ty: u32, event: Ref, user: Ref) -> Ref;

#[link(name = "CoreGraphics", kind = "framework")]
#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(tap: u32, place: u32, options: u32, mask: u64, cb: TapCallback, user: Ref) -> Ref;
    fn CGEventTapEnable(tap: Ref, enable: bool);
    fn CFMachPortCreateRunLoopSource(alloc: Ref, port: Ref, order: isize) -> Ref;
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFRunLoopAddSource(rl: Ref, src: Ref, mode: Ref);
    fn CFRunLoopRun();
    static kCFRunLoopCommonModes: Ref;
    static kCFBooleanTrue: Ref;
    fn CFRelease(p: Ref);
    fn CFStringCreateWithCString(alloc: Ref, s: *const c_char, enc: u32) -> Ref;
    fn CFDictionaryCreate(alloc: Ref, keys: *const Ref, vals: *const Ref, n: isize, kcb: Ref, vcb: Ref) -> Ref;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    static kAXTrustedCheckOptionPrompt: Ref;
    fn AXIsProcessTrustedWithOptions(opts: Ref) -> bool;

    fn CGEventCreate(src: Ref) -> Ref;
    fn CGEventGetLocation(e: Ref) -> CGPoint;
    fn CGEventGetIntegerValueField(e: Ref, f: u32) -> i64;
    fn CGEventGetDoubleValueField(e: Ref, f: u32) -> f64;
    fn CGEventGetFlags(e: Ref) -> u64;
    fn CGEventSetIntegerValueField(e: Ref, f: u32, v: i64);
    fn CGEventSetDoubleValueField(e: Ref, f: u32, v: f64);
    fn CGEventSetFlags(e: Ref, flags: u64);
    fn CGEventSetType(e: Ref, ty: u32);
    fn CGEventCreateMouseEvent(src: Ref, ty: u32, pos: CGPoint, button: u32) -> Ref;
    fn CGEventCreateKeyboardEvent(src: Ref, vk: u16, down: bool) -> Ref;
    fn CGEventCreateScrollWheelEvent2(src: Ref, units: u32, count: u32, w1: i32, w2: i32, w3: i32) -> Ref;
    fn CGEventPost(tap: u32, e: Ref);
    fn CGEventSourceCreate(state: i32) -> Ref;
    fn CGEventSourceSetLocalEventsSuppressionInterval(src: Ref, secs: f64);
    fn CGWarpMouseCursorPosition(p: CGPoint) -> i32;
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
    fn CGGetActiveDisplayList(max: u32, ds: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayBounds(d: u32) -> CGRect;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayHideCursor(d: u32) -> i32;
    fn CGDisplayShowCursor(d: u32) -> i32;
    fn _CGSDefaultConnection() -> i32;
    fn CGSSetConnectionProperty(cid: i32, target: i32, key: Ref, value: Ref) -> i32;
}

// Event types.
const LDOWN: u32 = 1;
const LUP: u32 = 2;
const RDOWN: u32 = 3;
const RUP: u32 = 4;
const MOVED: u32 = 5;
const LDRAG: u32 = 6;
const RDRAG: u32 = 7;
const KEYDOWN: u32 = 10;
const KEYUP: u32 = 11;
const FLAGS: u32 = 12;
const SYSDEFINED: u32 = 14;
const SCROLL: u32 = 22;
const ODOWN: u32 = 25;
const OUP: u32 = 26;
const ODRAG: u32 = 27;
const TAP_DISABLED_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_USER: u32 = 0xFFFF_FFFF;

// Event fields.
const F_CLICK_STATE: u32 = 1;
const F_BUTTON: u32 = 3;
const F_DX: u32 = 4;
const F_DY: u32 = 5;
const F_KEYCODE: u32 = 9;
const F_SCROLL_LINE_Y: u32 = 11;
const F_SCROLL_LINE_X: u32 = 12;
const F_USERDATA: u32 = 42;
const F_SCROLL_CONTINUOUS: u32 = 88;
const F_SCROLL_PX_Y: u32 = 96;
const F_SCROLL_PX_X: u32 = 97;

/// Stamped on everything we post, so the tap can tell our events from the user's.
const MAGIC: i64 = 0x0B21_D6E;

// Modifier flags: generic, then the device-specific left/right bits.
const SHIFT: u64 = 0x20000;
const CTRL: u64 = 0x40000;
const ALT: u64 = 0x80000;
const CMD: u64 = 0x100000;
const NUMPAD: u64 = 0x200000;
const FN: u64 = 0x800000;
const CAPS: u64 = 0x10000;

fn modifier_bits(hid: u16) -> Option<(u64, u64)> {
    Some(match hid {
        keymap::LCTRL => (CTRL, 0x1),
        keymap::LSHIFT => (SHIFT, 0x2),
        keymap::RSHIFT => (SHIFT, 0x4),
        keymap::LGUI => (CMD, 0x8),
        keymap::RGUI => (CMD, 0x10),
        keymap::LALT => (ALT, 0x20),
        keymap::RALT => (ALT, 0x40),
        keymap::RCTRL => (CTRL, 0x2000),
        _ => return None,
    })
}

struct Shared {
    events: Sender<Event>,
    capturing: AtomicBool,
    tap: AtomicPtr<c_void>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

extern "C" fn tap_cb(_proxy: Ref, ty: u32, event: Ref, _user: Ref) -> Ref {
    let Some(sh) = SHARED.get() else { return event };
    if ty == TAP_DISABLED_TIMEOUT || ty == TAP_DISABLED_USER {
        let tap = sh.tap.load(Ordering::SeqCst);
        if !tap.is_null() {
            unsafe { CGEventTapEnable(tap, true) };
        }
        return event;
    }
    unsafe {
        if CGEventGetIntegerValueField(event, F_USERDATA) == MAGIC {
            return event;
        }
    }
    let capturing = sh.capturing.load(Ordering::Relaxed);
    let send = |i: Input| {
        let _ = sh.events.send(Event::Input(i));
    };
    unsafe {
        match ty {
            MOVED | LDRAG | RDRAG | ODRAG => {
                let p = CGEventGetLocation(event);
                let dx = CGEventGetDoubleValueField(event, F_DX);
                let dy = CGEventGetDoubleValueField(event, F_DY);
                send(Input::Move { x: p.x, y: p.y, dx, dy });
            }
            LDOWN | LUP => send(Input::Button { button: 0, down: ty == LDOWN }),
            RDOWN | RUP => send(Input::Button { button: 1, down: ty == RDOWN }),
            ODOWN | OUP => {
                let b = CGEventGetIntegerValueField(event, F_BUTTON).clamp(2, 4) as u8;
                send(Input::Button { button: b, down: ty == ODOWN });
            }
            SCROLL => {
                let continuous = CGEventGetIntegerValueField(event, F_SCROLL_CONTINUOUS) != 0;
                let (dx, dy) = if continuous {
                    (CGEventGetDoubleValueField(event, F_SCROLL_PX_X), CGEventGetDoubleValueField(event, F_SCROLL_PX_Y))
                } else {
                    // A wheel notch: ~40 px, roughly what Windows scrolls for one notch.
                    (
                        CGEventGetIntegerValueField(event, F_SCROLL_LINE_X) as f64 * 40.0,
                        CGEventGetIntegerValueField(event, F_SCROLL_LINE_Y) as f64 * 40.0,
                    )
                };
                send(Input::Scroll { dx, dy });
            }
            KEYDOWN | KEYUP => {
                let vk = CGEventGetIntegerValueField(event, F_KEYCODE) as u16;
                if let Some(hid) = keymap::hid_from_mac(vk) {
                    send(Input::Key { hid, down: ty == KEYDOWN });
                }
            }
            FLAGS => {
                let vk = CGEventGetIntegerValueField(event, F_KEYCODE) as u16;
                let flags = CGEventGetFlags(event);
                if vk == 0x39 {
                    // Caps Lock only reports toggles; pass one press through.
                    send(Input::Key { hid: 0x39, down: true });
                    send(Input::Key { hid: 0x39, down: false });
                } else if let Some(hid) = keymap::hid_from_mac(vk) {
                    if let Some((_, dev)) = modifier_bits(hid) {
                        send(Input::Key { hid, down: flags & dev != 0 });
                    }
                }
            }
            SYSDEFINED => {
                match media_from_event(event) {
                    Some((m, down)) => {
                        if down {
                            send(Input::Media(m));
                        }
                    }
                    None => return event, // brightness and friends stay on this Mac
                }
            }
            _ => {}
        }
    }
    if capturing {
        null_mut()
    } else {
        event
    }
}

const NX_SOUND_UP: i64 = 0;
const NX_SOUND_DOWN: i64 = 1;
const NX_MUTE: i64 = 7;
const NX_PLAY: i64 = 16;
const NX_NEXT: i64 = 17;
const NX_PREVIOUS: i64 = 18;
const NX_FAST: i64 = 19;
const NX_REWIND: i64 = 20;

fn media_code(m: Media) -> i64 {
    match m {
        Media::PlayPause => NX_PLAY,
        Media::Next => NX_NEXT,
        Media::Previous => NX_PREVIOUS,
        Media::VolumeUp => NX_SOUND_UP,
        Media::VolumeDown => NX_SOUND_DOWN,
        Media::Mute => NX_MUTE,
    }
}

unsafe fn media_from_event(event: Ref) -> Option<(Media, bool)> {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    objc2::rc::autoreleasepool(|_| {
        let cls = AnyClass::get(c"NSEvent")?;
        let ns: *mut AnyObject = msg_send![cls, eventWithCGEvent: event];
        if ns.is_null() {
            return None;
        }
        let subtype: i16 = msg_send![ns, subtype];
        if subtype != 8 {
            return None;
        }
        let data1: isize = msg_send![ns, data1];
        let key = ((data1 as i64) & 0xFFFF_0000) >> 16;
        let down = ((data1 as i64) & 0xFF00) >> 8 == 0xA;
        let m = match key {
            NX_PLAY => Media::PlayPause,
            NX_NEXT | NX_FAST => Media::Next,
            NX_PREVIOUS | NX_REWIND => Media::Previous,
            NX_SOUND_UP => Media::VolumeUp,
            NX_SOUND_DOWN => Media::VolumeDown,
            NX_MUTE => Media::Mute,
            _ => return None,
        };
        Some((m, down))
    })
}

fn post_media(code: i64, down: bool) {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::msg_send;
    use objc2_foundation::NSPoint;
    unsafe {
        objc2::rc::autoreleasepool(|_| {
            let Some(cls) = AnyClass::get(c"NSEvent") else { return };
            let data1 = (code << 16) | (if down { 0xA } else { 0xB } << 8);
            let flags: usize = if down { 0xA00 } else { 0xB00 };
            let ns: *mut AnyObject = msg_send![cls,
                otherEventWithType: SYSDEFINED as usize,
                location: NSPoint::new(0.0, 0.0),
                modifierFlags: flags,
                timestamp: 0.0f64,
                windowNumber: 0isize,
                context: null_mut::<AnyObject>(),
                subtype: 8i16,
                data1: data1 as isize,
                data2: -1isize];
            if ns.is_null() {
                return;
            }
            let cg: Ref = msg_send![ns, CGEvent];
            if !cg.is_null() {
                CGEventSetIntegerValueField(cg, F_USERDATA, MAGIC);
                CGEventPost(0, cg);
            }
        })
    }
}

struct Inject {
    flags: u64,
    dev_flags: u64,
    buttons: [bool; 5],
    last_click: Option<(Button, Instant, CGPoint, i64)>,
    scroll_rem: (f64, f64),
    pos: CGPoint,
}

struct Mac {
    shared: Arc<Shared>,
    src: Ref,
    inject: Mutex<Inject>,
}

unsafe impl Send for Mac {}
unsafe impl Sync for Mac {}

fn ask_for_accessibility() -> bool {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let vals = [kCFBooleanTrue];
        let d = CFDictionaryCreate(
            null_mut(),
            keys.as_ptr(),
            vals.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const u8 as Ref,
            &kCFTypeDictionaryValueCallBacks as *const u8 as Ref,
        );
        let ok = AXIsProcessTrustedWithOptions(d);
        CFRelease(d);
        ok
    }
}

pub fn start(events: Sender<Event>) -> Arc<dyn Platform> {
    let shared = Arc::new(Shared { events, capturing: AtomicBool::new(false), tap: AtomicPtr::new(null_mut()) });
    let _ = SHARED.set(shared.clone());
    let src = unsafe {
        let s = CGEventSourceCreate(1); // HID system state
        CGEventSourceSetLocalEventsSuppressionInterval(s, 0.0);
        // Lets a background app hide the pointer while it drives the PC.
        let key = CFStringCreateWithCString(null_mut(), c"SetsCursorInBackground".as_ptr(), 0x0800_0100);
        let cid = _CGSDefaultConnection();
        CGSSetConnectionProperty(cid, cid, key, kCFBooleanTrue);
        CFRelease(key);
        s
    };
    std::thread::Builder::new()
        .name("event-tap".into())
        .spawn(move || run_tap())
        .unwrap();
    let pos = unsafe {
        let e = CGEventCreate(null_mut());
        let p = CGEventGetLocation(e);
        CFRelease(e);
        p
    };
    Arc::new(Mac {
        shared,
        src,
        inject: Mutex::new(Inject {
            flags: 0,
            dev_flags: 0,
            buttons: [false; 5],
            last_click: None,
            scroll_rem: (0.0, 0.0),
            pos,
        }),
    })
}

fn run_tap() {
    let mask: u64 = [LDOWN, LUP, RDOWN, RUP, MOVED, LDRAG, RDRAG, KEYDOWN, KEYUP, FLAGS, SYSDEFINED, SCROLL, ODOWN, OUP, ODRAG]
        .iter()
        .fold(0, |m, t| m | (1u64 << t));
    let mut asked = false;
    let tap = loop {
        let tap = unsafe { CGEventTapCreate(1, 0, 0, mask, tap_cb, null_mut()) };
        if !tap.is_null() {
            break tap;
        }
        if !asked {
            log::warn!("no Accessibility permission yet; asking");
            ask_for_accessibility();
            asked = true;
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    log::info!("event tap running");
    SHARED.get().unwrap().tap.store(tap, Ordering::SeqCst);
    unsafe {
        let src = CFMachPortCreateRunLoopSource(null_mut(), tap, 0);
        CFRunLoopAddSource(CFRunLoopGetCurrent(), src, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
        CFRunLoopRun();
    }
}

impl Mac {
    fn post(&self, e: Ref, flags: u64) {
        unsafe {
            CGEventSetIntegerValueField(e, F_USERDATA, MAGIC);
            CGEventSetFlags(e, flags);
            CGEventPost(0, e);
            CFRelease(e);
        }
    }
}

impl Platform for Mac {
    fn displays(&self) -> Vec<Rect> {
        let mut ids = [0u32; 16];
        let mut n = 0u32;
        unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) };
        ids[..n as usize]
            .iter()
            .map(|&d| {
                let r = unsafe { CGDisplayBounds(d) };
                Rect { x: r.origin.x, y: r.origin.y, w: r.size.w, h: r.size.h }
            })
            .collect()
    }


    fn cursor(&self) -> (f64, f64) {
        unsafe {
            let e = CGEventCreate(null_mut());
            let p = CGEventGetLocation(e);
            CFRelease(e);
            (p.x, p.y)
        }
    }

    fn set_capturing(&self, on: bool) {
        let was = self.shared.capturing.swap(on, Ordering::SeqCst);
        if was == on {
            return;
        }
        unsafe {
            CGAssociateMouseAndMouseCursorPosition(!on);
            if on {
                CGDisplayHideCursor(CGMainDisplayID());
            } else {
                CGDisplayShowCursor(CGMainDisplayID());
            }
        }
    }

    fn warp(&self, x: f64, y: f64) {
        unsafe { CGWarpMouseCursorPosition(CGPoint { x, y }) };
        self.inject.lock().unwrap().pos = CGPoint { x, y };
    }

    fn move_to(&self, x: f64, y: f64) {
        let mut st = self.inject.lock().unwrap();
        let (ty, btn) = if st.buttons[0] {
            (LDRAG, 0)
        } else if st.buttons[1] {
            (RDRAG, 1)
        } else if st.buttons[2..].iter().any(|b| *b) {
            (ODRAG, 2)
        } else {
            (MOVED, 0)
        };
        let p = CGPoint { x, y };
        let (dx, dy) = (x - st.pos.x, y - st.pos.y);
        st.pos = p;
        unsafe {
            let e = CGEventCreateMouseEvent(self.src, ty, p, btn);
            CGEventSetDoubleValueField(e, F_DX, dx);
            CGEventSetDoubleValueField(e, F_DY, dy);
            self.post(e, st.flags | st.dev_flags);
        }
    }

    fn button(&self, button: Button, down: bool) {
        let mut st = self.inject.lock().unwrap();
        let b = (button as usize).min(4);
        st.buttons[b] = down;
        let ty = match (b, down) {
            (0, true) => LDOWN,
            (0, false) => LUP,
            (1, true) => RDOWN,
            (1, false) => RUP,
            (_, true) => ODOWN,
            (_, false) => OUP,
        };
        let pos = st.pos;
        // macOS needs the click count spelled out, or double-clicks never happen.
        let clicks = if down {
            let n = match st.last_click {
                Some((lb, t, p, n))
                    if lb == button
                        && t.elapsed() < Duration::from_millis(500)
                        && (p.x - pos.x).abs() < 5.0
                        && (p.y - pos.y).abs() < 5.0 =>
                {
                    n + 1
                }
                _ => 1,
            };
            st.last_click = Some((button, Instant::now(), pos, n));
            n
        } else {
            st.last_click.map(|c| c.3).unwrap_or(1)
        };
        unsafe {
            let e = CGEventCreateMouseEvent(self.src, ty, pos, b as u32);
            CGEventSetIntegerValueField(e, F_CLICK_STATE, clicks);
            if b >= 2 {
                CGEventSetIntegerValueField(e, F_BUTTON, b as i64);
            }
            self.post(e, st.flags | st.dev_flags);
        }
    }

    fn scroll(&self, dx: f64, dy: f64) {
        let mut st = self.inject.lock().unwrap();
        let x = dx + st.scroll_rem.0;
        let y = dy + st.scroll_rem.1;
        let (ix, iy) = (x.trunc() as i32, y.trunc() as i32);
        st.scroll_rem = (x - ix as f64, y - iy as f64);
        if ix == 0 && iy == 0 {
            return;
        }
        unsafe {
            let e = CGEventCreateScrollWheelEvent2(self.src, 0, 2, iy, ix, 0);
            self.post(e, st.flags | st.dev_flags);
        }
    }

    fn key(&self, hid: u16, down: bool) {
        let Some(vk) = keymap::mac_from_hid(hid) else { return };
        let mut st = self.inject.lock().unwrap();
        if hid == 0x39 {
            if down {
                st.flags ^= CAPS;
            } else {
                return;
            }
        }
        let modifier = modifier_bits(hid);
        if let Some((_, dev)) = modifier {
            if down {
                st.dev_flags |= dev;
            } else {
                st.dev_flags &= !dev;
            }
            let dev_now = st.dev_flags;
            let any = |bits: &[u64]| bits.iter().any(|b| dev_now & b != 0);
            st.flags &= !(SHIFT | CTRL | ALT | CMD);
            if any(&[0x2, 0x4]) { st.flags |= SHIFT }
            if any(&[0x1, 0x2000]) { st.flags |= CTRL }
            if any(&[0x20, 0x40]) { st.flags |= ALT }
            if any(&[0x8, 0x10]) { st.flags |= CMD }
        }
        let mut flags = st.flags | st.dev_flags;
        if (0x4F..=0x52).contains(&hid) {
            flags |= NUMPAD | FN; // arrows
        } else if (0x3A..=0x45).contains(&hid) || (0x49..=0x4E).contains(&hid) || (0x68..=0x6E).contains(&hid) {
            flags |= FN; // F-keys, Home/End/Page/Forward Delete
        }
        unsafe {
            let e = CGEventCreateKeyboardEvent(self.src, vk, down);
            if modifier.is_some() || hid == 0x39 {
                CGEventSetType(e, FLAGS);
            }
            self.post(e, flags);
        }
    }

    fn media(&self, m: Media) {
        let code = media_code(m);
        post_media(code, true);
        post_media(code, false);
    }
}
