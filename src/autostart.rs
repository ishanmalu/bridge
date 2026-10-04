//! Start at login. On Windows this is a scheduled task with highest privileges, so Bridge can
//! type into admin windows without a UAC prompt every boot (creating it needs admin once).

pub fn enabled() -> bool {
    #[cfg(target_os = "macos")]
    return plist().exists();
    #[cfg(windows)]
    return hidden("schtasks")
        .args(["/Query", "/TN", "Bridge"])
        .output()
        .is_ok_and(|o| o.status.success())
        || run_key_set();
    #[allow(unreachable_code)]
    false
}

pub fn set(on: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    set_for(&exe, on)
}

pub fn set_for(exe: &std::path::Path, on: bool) -> Result<(), String> {
    let exe = exe.to_path_buf();
    #[cfg(target_os = "macos")]
    {
        let p = plist();
        if on {
            // Launch the .app bundle when we're inside one, so permissions stay attached to it.
            let exe = exe.to_string_lossy();
            let body = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.ishanmalu.bridge</string>
<key>ProgramArguments</key><array><string>{exe}</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>ProcessType</key><string>Interactive</string>
</dict></plist>
"#
            );
            std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&p, body).map_err(|e| e.to_string())
        } else {
            let _ = std::fs::remove_file(&p);
            Ok(())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let cmd = |name: &str| {
            let mut c = std::process::Command::new(name);
            c.creation_flags(0x0800_0000); // no console window
            c
        };
        if on {
            let tr = format!("\"{}\"", exe.display());
            // An elevated task must only ever run a binary ordinary programs can't replace.
            let protected = std::env::var_os("ProgramFiles")
                .map(std::path::PathBuf::from)
                .is_some_and(|pf| exe.starts_with(pf));
            let ok = protected
                && cmd("schtasks")
                .args(["/Create", "/F", "/TN", "Bridge", "/TR", &tr, "/SC", "ONLOGON", "/RL", "HIGHEST"])
                .output()
                .is_ok_and(|o| o.status.success());
            if ok {
                return Ok(());
            }
            log::warn!("starting without admin rights at login (needs Bridge in Program Files and run as administrator); admin windows won't take input");
            cmd("reg")
                .args(["add", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "Bridge", "/t", "REG_SZ", "/d", &tr, "/f"])
                .output()
                .map_err(|e| e.to_string())
                .map(|_| ())
        } else {
            let _ = cmd("schtasks").args(["/Delete", "/F", "/TN", "Bridge"]).output();
            let _ = cmd("reg")
                .args(["delete", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "Bridge", "/f"])
                .output();
            Ok(())
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (on, exe);
        Err("unsupported".into())
    }
}

#[cfg(target_os = "macos")]
fn plist() -> std::path::PathBuf {
    dirs::home_dir().unwrap().join("Library/LaunchAgents/dev.ishanmalu.bridge.plist")
}

/// A helper process with no console window (a GUI app's children would otherwise flash one).
#[cfg(windows)]
fn hidden(name: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut c = std::process::Command::new(name);
    c.creation_flags(0x0800_0000);
    c
}

#[cfg(windows)]
fn run_key_set() -> bool {
    hidden("reg")
        .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", "/v", "Bridge"])
        .output()
        .is_ok_and(|o| o.status.success())
}
