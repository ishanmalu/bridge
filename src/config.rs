use crate::proto::{Os, Side};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Peer {
    pub name: String,
    pub os: Os,
    /// Hex-encoded Noise static public key, pinned at pairing.
    pub public_key: String,
    /// Last address it was reached at; mDNS discovery is tried first.
    pub addr: Option<String>,
    /// For Wake-on-LAN, learned from the peer's Hello.
    #[serde(default)]
    pub macs: Vec<[u8; 6]>,
    #[serde(default = "default_port")]
    pub port: u16,
}

fn default_port() -> u16 {
    crate::proto::PORT
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Config {
    /// Hex-encoded Noise static private key, generated on first run.
    pub private_key: String,
    pub public_key: String,
    pub peer: Option<Peer>,
    /// Which edge of this machine's screens the other machine sits beyond.
    pub peer_side: Side,
    /// How far (in pixels) you have to push into the edge before crossing.
    pub edge_resistance: f64,
    /// Pause edge crossing; the hotkey still works.
    pub paused: bool,
    /// Multiplies pointer movement arriving from the other machine.
    pub incoming_pointer_scale: f64,
    /// Multiplies scrolling arriving from the other machine.
    pub incoming_scroll_scale: f64,
    pub invert_incoming_scroll: bool,
    /// Swap Cmd and Ctrl when the other machine runs a different OS.
    pub swap_cmd_ctrl: bool,
    /// Double-tap this key (HID usage) to switch. Defaults: Right Option on Mac, Right Ctrl on PC.
    pub hotkey: u16,
    /// Clipboard text, images and files travel when you switch.
    pub share_clipboard: bool,
    /// Files bigger than this (MB, total) are left behind.
    pub max_clipboard_files_mb: u64,
    /// Turn crossing off while a full-screen app is in front (games, films).
    pub block_in_fullscreen: bool,
    /// TCP port this machine listens on.
    pub port: u16,
    /// Find the other machine with mDNS (otherwise only its saved address is tried).
    pub discovery: bool,
    /// Look for a new version once a day.
    pub check_updates: bool,
    /// In-memory only (tests): never written to or reloaded from disk.
    #[serde(skip)]
    pub ephemeral: bool,
}

impl Default for Config {
    fn default() -> Self {
        let mac = Os::current() == Os::Mac;
        Config {
            private_key: String::new(),
            public_key: String::new(),
            peer: None,
            peer_side: if mac { Side::Right } else { Side::Left },
            edge_resistance: 30.0,
            paused: false,
            incoming_pointer_scale: 1.0,
            incoming_scroll_scale: 1.0,
            invert_incoming_scroll: false,
            swap_cmd_ctrl: true,
            hotkey: if mac { crate::keymap::RALT } else { crate::keymap::RCTRL },
            share_clipboard: true,
            max_clipboard_files_mb: 200,
            block_in_fullscreen: true,
            port: crate::proto::PORT,
            discovery: true,
            check_updates: true,
            ephemeral: false,
        }
    }
}

pub fn dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir().unwrap().join("Library/Application Support")
    } else {
        dirs::config_dir().unwrap()
    };
    base.join("Bridge")
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

impl Config {
    pub fn load() -> Config {
        let mut cfg: Config = match std::fs::read_to_string(path()) {
            Err(_) => Config::default(),
            Ok(s) => match toml::from_str(&s) {
                Ok(c) => c,
                Err(e) => {
                    // Keep the broken file (it holds the keys) rather than silently re-keying over it.
                    let keep = path().with_extension("toml.broken");
                    log::error!("config.toml is invalid ({e}); moved it to {}", keep.display());
                    let _ = std::fs::rename(path(), keep);
                    Config::default()
                }
            },
        };
        if cfg.private_key.is_empty() {
            let kp = snow::Builder::new(crate::net::NOISE.parse().unwrap())
                .generate_keypair()
                .unwrap();
            cfg.private_key = hex::encode(kp.private);
            cfg.public_key = hex::encode(kp.public);
            cfg.save();
        }
        cfg
    }

    pub fn save(&self) {
        if self.ephemeral {
            return;
        }
        let _ = std::fs::create_dir_all(dir());
        let tmp = path().with_extension("tmp");
        if write_private(&tmp, toml::to_string_pretty(self).unwrap().as_bytes()).is_ok() {
            let _ = std::fs::rename(tmp, path());
        }
    }

    pub fn private_key(&self) -> Vec<u8> {
        hex::decode(&self.private_key).unwrap_or_default()
    }
}

/// The config holds this machine's private key: owner-only on Unix. (On Windows, %APPDATA% is
/// already private to the user.)
fn write_private(p: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(data)
}

/// Modification time of config.toml, to notice edits made outside the running app.
pub fn mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}
