//! Wire messages. Everything between the two machines is one of these, postcard-encoded,
//! inside an encrypted Noise stream (see net.rs).

use serde::{Deserialize, Serialize};

pub const PORT: u16 = 24800;
pub const PAIR_PORT: u16 = 24801;
pub const SERVICE: &str = "_bridge._tcp.local.";
pub const PAIR_SERVICE: &str = "_bridge-pair._tcp.local.";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Windows,
    Other,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Other
        }
    }
}

/// Which edge of *this* machine's screens faces the other machine.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Media {
    PlayPause,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    Mute,
}

/// Buttons: 0 left, 1 right, 2 middle, 3 back, 4 forward.
pub type Button = u8;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Clip {
    Text(String),
    /// PNG-encoded image.
    Image(Vec<u8>),
    /// (file name, contents) pairs. Directories are skipped.
    Files(Vec<(String, Vec<u8>)>),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Msg {
    Hello { name: String, os: Os, macs: Vec<[u8; 6]>, version: String },
    Ping,
    /// The sender is handing input over. `frac` is the 0..1 position along the shared edge,
    /// or None to land in the middle of the main screen (hotkey switch).
    Enter { frac: Option<f64> },
    /// The receiver of input pushed off its edge (or pressed the hotkey) and gives control back.
    Leave { frac: Option<f64> },
    MouseMove { dx: f64, dy: f64 },
    MouseButton { button: Button, down: bool },
    /// Pixels; positive dy = content moves down (scroll up), positive dx = content moves right.
    Scroll { dx: f64, dy: f64 },
    /// USB HID usage (keyboard page 0x07).
    Key { hid: u16, down: bool },
    Media(Media),
    Clipboard(Clip),
}
