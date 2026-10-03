# Bridge

One keyboard and mouse for a Mac and a PC, in either direction. Push the pointer off the shared
edge (or double-tap Right Option on the Mac / Right Ctrl on the PC) and your keyboard and mouse
drive the other machine. The clipboard comes with you.

One Rust codebase builds both sides. No server, no account: the two machines talk directly on your
network over Noise (KK, keys pinned at pairing).

## What it does

- **Edge crossing** onto the right display at the same point along the border, with push resistance.
- **Hotkey**: double-tap Right Option (Mac) or Right Ctrl (PC).
- **Clipboard**: text, images and **copied files** move when you switch (files land in `~/Downloads/Bridge`).
- **Shortcuts work**: Cmd↔Ctrl swap, so Cmd+C copies on the PC and the PC's Ctrl+C is Cmd+C on the Mac.
- **Media keys** both ways (play/pause, next, previous, volume, mute). Brightness stays local.
- **Touch to reclaim**: move a machine's own mouse while it's being driven and it takes control back.
- **Wake-on-LAN**: switching to a sleeping PC wakes it (learned automatically after the first connection).
- **Full-screen apps hold the edges** on Windows (games, films); the hotkey still works.
- **Pause crossing**, **start at login**, auto-reconnect after sleep or network changes.

## Download

From [Releases](https://github.com/ishanmalu/bridge/releases/latest): `Bridge-mac.zip` (Apple silicon
and Intel, macOS 11+) and `Bridge-windows.zip` (Windows 10/11, x64). You need it on both machines.
Checksums are in `SHA256SUMS.txt`.

Bridge isn't signed with paid Apple/Microsoft certificates yet, so both systems warn the first time:

- **Mac**: unzip, drag Bridge to Applications, open it. When macOS says it can't verify it, open
  System Settings → Privacy & Security, scroll down and click **Open Anyway**. (Or in Terminal:
  `xattr -dr com.apple.quarantine /Applications/Bridge.app`.)
- **Windows**: SmartScreen shows "Windows protected your PC": click **More info → Run anyway**.

## Set up

1. **Mac**: open Bridge. Grant it **Accessibility** when asked (System Settings → Privacy & Security
   → Accessibility, switch Bridge on). It appears in the menu bar.
2. **PC**: make a folder `C:\Program Files\Bridge`, put `Bridge.exe` in it, then right-click it →
   **Run as administrator**. Running as administrator lets Bridge type into admin windows and adds
   its firewall rule for private networks. It appears in the tray (click ^ by the clock).
   Then choose **Start at login** in its menu: from Program Files, that starts it elevated at every
   login without a prompt.
3. **Pair**: on the PC choose *Pair → Show a pairing code*. On the Mac choose *Pair → Enter a pairing
   code…* and type it. If it can't find the PC, type the code then the address the PC shows,
   e.g. `123456 192.168.1.20`.
4. **Cross**: push the pointer off the Mac's right edge. (PC on the other side? Set `peer_side`.)
   Or double-tap Right Option (Mac) / Right Ctrl (PC).

Check a machine on its own any time: `bridge selftest` (Mac:
`/Applications/Bridge.app/Contents/MacOS/bridge selftest`; PC: `"C:\Program Files\Bridge\Bridge.exe" selftest`
from a terminal).

## Security

- Nothing goes through a server. The machines connect directly on your network (TCP 24800).
- Pairing turns the six-digit code into a key with SPAKE2: someone watching the network learns
  nothing, and a guesser gets one try per attempt, three attempts per code.
- After pairing, every connection is Noise KK with both machines' keys pinned. An unpaired device
  can't connect, read or inject anything.
- Keystrokes leave the machine only while you're driving the other one. Nothing is logged except
  connection events.
- Received files are saved under `Downloads/Bridge` with sanitised names.
- The private key lives in the config file, readable only by you.
- The elevated Windows login task is only created when Bridge runs from Program Files, so an
  ordinary program can't swap the binary.

## Build from source

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-gnu
brew install mingw-w64
./scripts/build.sh        # tests, then dist/Bridge.app, dist/Bridge.exe and release zips
```

`cargo test` runs unit tests plus two end-to-end tests: two full Bridges talking over localhost,
covering crossing, keys, mouse, scroll, media, the hotkey, touch-to-reclaim and stuck-key release.
CI runs them on macOS and Windows, plus `bridge selftest` on a real Windows machine.

## Updating

Bridge checks for updates once a day and shows a banner when one is ready; click it, or choose
*Check for updates…* in the menu, then **Install and relaunch**. Every update is verified against a
signing key built into the app before it's installed. On the Mac, permissions carry over.

Coming from 0.1.x (no updater yet)? Download the new version once by hand: on the Mac replace
Bridge in Applications; on Windows run the new `Bridge.exe` and choose **Install**.

## Settings

Edit with *Open settings file* (Mac: `~/Library/Application Support/Bridge/config.toml`,
PC: `%APPDATA%\Bridge\config.toml`), then quit and reopen Bridge.

| key | default | |
|---|---|---|
| `peer_side` | Mac `right`, PC `left` | which edge faces the other machine (`left`/`right`/`top`/`bottom`) |
| `edge_resistance` | `30` | pixels of push before crossing |
| `incoming_pointer_scale` | `1.0` | raise on the PC if the Mac's trackpad feels slow there (high-DPI screens) |
| `incoming_scroll_scale` / `invert_incoming_scroll` | `1.0` / `false` | scrolling feel |
| `hotkey` | Mac `230` (Right Option), PC `228` (Right Ctrl) | HID usage of the double-tap key |
| `swap_cmd_ctrl` | `true` | |
| `share_clipboard`, `max_clipboard_files_mb` | `true`, `200` | |
| `block_in_fullscreen` | `true` | Windows: a full-screen app keeps the pointer (hotkey still works) |
| `port`, `discovery` | `24800`, `true` | listening port; mDNS lookup of the other machine |

## Troubleshooting

- *Open log* in the menu. `bridge status` from a terminal shows the pairing.
- Won't connect: both machines on the same network; TCP 24800 (and 24801 for pairing) allowed
  through the Windows firewall on private networks.
- The pointer is invisible on the PC after a crash: start Bridge again (it restores the cursors).
- Wake-on-LAN needs it enabled in the PC's BIOS/network adapter, and usually Ethernet.
- Typing doesn't reach admin windows on the PC: Bridge isn't running as administrator.
- Windows' UAC prompt and lock screen run on a secure desktop no app can type into.

## Not built yet

Mac trackpad gestures → Task View/virtual desktops, mirrored notifications, cross-machine search,
audio handoff, a visual screen-layout editor, Linux.
