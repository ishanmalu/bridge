//! Putting Bridge where it belongs.
//!
//! Mac: offer to move the app into /Applications. Run from Downloads, macOS launches it from a
//! random read-only folder ("App Translocation"), which breaks login items and permissions.
//!
//! Windows: install into Program Files (admin-only, so nothing else can swap the binary), with a
//! Start menu shortcut, firewall rule, a login task that starts it elevated without a prompt, and
//! an entry in Settings → Apps so it can be uninstalled like any other program.

#[cfg(target_os = "macos")]
mod mac {
    use std::path::PathBuf;

    pub fn installed() -> bool {
        crate::updater::app_bundle().is_some_and(|b| b.starts_with("/Applications"))
    }

    /// Copy the running bundle into /Applications and relaunch from there.
    pub fn install() -> Result<(), String> {
        let from = crate::updater::app_bundle().ok_or("not running from an app bundle")?;
        let to = PathBuf::from("/Applications/Bridge.app");
        let tmp = PathBuf::from("/Applications/.Bridge.app.new");
        let _ = std::fs::remove_dir_all(&tmp);
        let ok = std::process::Command::new("ditto").arg(&from).arg(&tmp).status().is_ok_and(|s| s.success());
        if !ok {
            return Err("couldn't copy Bridge into Applications".into());
        }
        let _ = std::process::Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&tmp).status();
        let _ = std::fs::remove_dir_all(&to);
        std::fs::rename(&tmp, &to).map_err(|e| e.to_string())?;
        // The login item, if any, should point at the new home.
        if crate::autostart::enabled() {
            let _ = crate::autostart::set_for(&to.join("Contents/MacOS/bridge"), true);
        }
        crate::updater::relaunch_mac(&to)
    }
}

#[cfg(target_os = "macos")]
pub use mac::{install, installed};

#[cfg(windows)]
mod win {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;

    const NO_WINDOW: u32 = 0x0800_0000;
    const LAUNCH_TASK: &str = "Bridge Launch";
    const UNINSTALL_KEY: &str = r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall\Bridge";

    fn cmd(name: &str) -> Command {
        let mut c = Command::new(name);
        c.creation_flags(NO_WINDOW);
        c
    }

    pub fn dir() -> PathBuf {
        std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files")).join("Bridge")
    }

    pub fn installed() -> bool {
        std::env::current_exe().is_ok_and(|e| e.starts_with(dir()))
    }

    fn shortcut() -> PathBuf {
        std::env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join(r"Microsoft\Windows\Start Menu\Programs\Bridge.lnk")
    }

    /// Ask Windows to rerun us elevated with `arg` (shows the UAC prompt).
    pub fn elevate(arg: &str) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let script = format!(
            "Start-Process -Verb RunAs -FilePath '{}' -ArgumentList '{}'",
            exe.display().to_string().replace('\'', "''"),
            arg
        );
        let ok = cmd("powershell").args(["-NoProfile", "-Command", &script]).status().is_ok_and(|s| s.success());
        if ok { Ok(()) } else { Err("Windows didn't allow it (the administrator prompt was declined)".into()) }
    }

    /// Runs elevated: copy in, register everything, start the installed copy.
    pub fn install() -> Result<(), String> {
        let src = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let exe = dir.join("Bridge.exe");
        // Stop a running installed copy so its exe can be replaced.
        if exe.exists() && src != exe {
            let _ = cmd("taskkill").args(["/F", "/IM", "Bridge.exe", "/FI", &format!("PID ne {}", std::process::id())]).status();
            std::thread::sleep(std::time::Duration::from_millis(800));
            let old = dir.join("Bridge.old.exe");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&exe, &old);
        }
        if src != exe {
            std::fs::copy(&src, &exe).map_err(|e| format!("couldn't copy into {}: {e}", dir.display()))?;
        }
        // The shortcut runs an on-demand elevated task, so Bridge opened from the Start menu can
        // type into admin windows and update itself, without a UAC prompt every time.
        let _ = cmd("schtasks")
            .args(["/Create", "/F", "/TN", LAUNCH_TASK, "/TR", &format!("\"{}\"", exe.display())])
            .args(["/SC", "ONCE", "/ST", "00:00", "/SD", "01/01/2000", "/RL", "HIGHEST"])
            .status();
        let ps = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath=\"$env:SystemRoot\\System32\\schtasks.exe\"; $s.Arguments='/run /tn \"{}\"'; $s.IconLocation='{},0'; $s.WindowStyle=7; $s.WorkingDirectory='{}'; $s.Description='One keyboard and mouse for a Mac and a PC'; $s.Save()",
            shortcut().display(),
            LAUNCH_TASK,
            exe.display(),
            dir.display()
        );
        let _ = cmd("powershell").args(["-NoProfile", "-Command", &ps]).status();
        let version = env!("CARGO_PKG_VERSION");
        for (name, kind, value) in [
            ("DisplayName", "REG_SZ", "Bridge".to_string()),
            ("DisplayVersion", "REG_SZ", version.to_string()),
            ("Publisher", "REG_SZ", "Ishan Malu".to_string()),
            ("DisplayIcon", "REG_SZ", exe.display().to_string()),
            ("InstallLocation", "REG_SZ", dir.display().to_string()),
            ("URLInfoAbout", "REG_SZ", "https://bridge.ishanmalu.dev".to_string()),
            ("UninstallString", "REG_SZ", format!("\"{}\" uninstall", exe.display())),
            ("NoModify", "REG_DWORD", "1".to_string()),
            ("NoRepair", "REG_DWORD", "1".to_string()),
        ] {
            let _ = cmd("reg").args(["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", &value, "/f"]).status();
        }
        // Firewall rule and elevated login task, both pointing at the installed exe.
        let _ = cmd("netsh").args(["advfirewall", "firewall", "delete", "rule", "name=Bridge"]).status();
        let _ = cmd("netsh")
            .args(["advfirewall", "firewall", "add", "rule", "name=Bridge", "dir=in", "action=allow"])
            .arg(format!("program={}", exe.display()))
            .args(["protocol=TCP", "localport=24800-24801", "profile=private,domain"])
            .status();
        let _ = cmd("schtasks")
            .args(["/Create", "/F", "/TN", "Bridge", "/TR", &format!("\"{}\"", exe.display()), "/SC", "ONLOGON", "/RL", "HIGHEST"])
            .status();
        log::info!("installed into {}", dir.display());
        relaunch_windows(&exe)
    }

    pub fn uninstall() -> Result<(), String> {
        let dir = dir();
        let _ = cmd("schtasks").args(["/Delete", "/F", "/TN", "Bridge"]).status();
        let _ = cmd("schtasks").args(["/Delete", "/F", "/TN", LAUNCH_TASK]).status();
        let _ = cmd("reg").args(["delete", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "Bridge", "/f"]).status();
        let _ = cmd("netsh").args(["advfirewall", "firewall", "delete", "rule", "name=Bridge"]).status();
        let _ = cmd("reg").args(["delete", UNINSTALL_KEY, "/f"]).status();
        let _ = std::fs::remove_file(shortcut());
        let _ = cmd("taskkill").args(["/F", "/IM", "Bridge.exe", "/FI", &format!("PID ne {}", std::process::id())]).status();
        crate::ui::notify_cli("Bridge was uninstalled. Your settings stay in %APPDATA%\\Bridge.");
        // The folder holds this very exe, so remove it a moment after we exit.
        let _ = cmd("cmd")
            .args(["/C", &format!("timeout /T 2 /NOBREAK >NUL & rmdir /S /Q \"{}\"", dir.display())])
            .spawn();
        std::process::exit(0);
    }

    pub fn relaunch_windows(exe: &std::path::Path) -> Result<(), String> {
        // Any other running copy (e.g. the tray app when updating from a terminal) would hold
        // the port and the new version would bow out; stop it.
        let _ = cmd("taskkill").args(["/F", "/IM", "Bridge.exe", "/FI", &format!("PID ne {}", std::process::id())]).status();
        // `start` detaches it from us, so it outlives this process.
        cmd("cmd")
            .args(["/C", "timeout /T 1 /NOBREAK >NUL & start \"\""])
            .arg(exe)
            .spawn()
            .map_err(|e| e.to_string())?;
        std::process::exit(0);
    }
}

#[cfg(windows)]
pub use win::{elevate, install, installed, relaunch_windows, uninstall};

#[cfg(not(any(target_os = "macos", windows)))]
pub fn installed() -> bool {
    true
}
#[cfg(not(any(target_os = "macos", windows)))]
pub fn install() -> Result<(), String> {
    Ok(())
}
