//! `bridge selftest`: exercises the real OS pieces on this machine (not the other one) and
//! prints what works. Exit code is the number of failures.

use crate::clipboard::Clipboard;
use crate::proto::Clip;
use std::sync::mpsc::channel;
use std::time::Duration;

fn check(name: &str, failures: &mut i32, r: Result<String, String>) {
    match r {
        Ok(detail) => println!("  ok    {name}: {detail}"),
        Err(e) => {
            *failures += 1;
            println!("  FAIL  {name}: {e}");
        }
    }
}

pub fn run() -> i32 {
    let mut failures = 0;
    println!("Bridge {} self-test on {} ({:?})", env!("CARGO_PKG_VERSION"), crate::machine_name(), crate::proto::Os::current());

    let cfg = crate::config::Config::load();
    check(
        "config",
        &mut failures,
        if cfg.public_key.len() == 64 { Ok(crate::config::path().display().to_string()) } else { Err("no keys".into()) },
    );

    let (tx, _rx) = channel();
    let plat = crate::platform::start(tx);
    std::thread::sleep(Duration::from_millis(500));

    let ds = plat.displays();
    check(
        "displays",
        &mut failures,
        if ds.is_empty() {
            Err("none found".into())
        } else {
            Ok(ds.iter().map(|r| format!("{}x{} at {},{}", r.w, r.h, r.x, r.y)).collect::<Vec<_>>().join("; "))
        },
    );

    // Move the pointer the way the other machine would, then read it back.
    let (cx, cy) = crate::geometry::center(&ds);
    let before = plat.cursor();
    let target = (cx.round() + 37.0, cy.round() + 23.0);
    plat.move_to(target.0, target.1);
    std::thread::sleep(Duration::from_millis(300));
    let got = plat.cursor();
    check(
        "pointer injection",
        &mut failures,
        if (got.0 - target.0).abs() <= 2.0 && (got.1 - target.1).abs() <= 2.0 {
            Ok(format!("moved to {:.0},{:.0}", got.0, got.1))
        } else if cfg!(target_os = "macos") {
            Err(format!("asked for {target:?}, pointer is at {got:?}. Grant Bridge Accessibility permission"))
        } else {
            Err(format!("asked for {target:?}, pointer is at {got:?}"))
        },
    );
    plat.warp(before.0, before.1);

    // Clipboard: put text and a file on it as if they'd arrived from the other machine.
    let saved = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok());
    let mut clip = Clipboard::new();
    let text = format!("bridge selftest {}", std::process::id());
    clip.apply(Clip::Text(text.clone()));
    let read = arboard::Clipboard::new().ok().and_then(|mut c| c.get_text().ok());
    check(
        "clipboard text",
        &mut failures,
        if read.as_deref() == Some(text.as_str()) { Ok("round-tripped".into()) } else { Err(format!("read back {read:?}")) },
    );
    let name = format!("bridge-selftest-{}.txt", std::process::id());
    clip.apply(Clip::Files(vec![(name.clone(), b"hello".to_vec())]));
    let path = crate::clipboard::inbox().join(&name);
    let listed = arboard::Clipboard::new().ok().and_then(|mut c| c.get().file_list().ok()).unwrap_or_default();
    check(
        "clipboard files",
        &mut failures,
        if std::fs::read(&path).ok().as_deref() == Some(b"hello") && listed.iter().any(|p| p.ends_with(&name)) {
            Ok(format!("saved to {} and placed on the clipboard", path.display()))
        } else {
            Err(format!("file present: {}, clipboard lists {listed:?}", path.exists()))
        },
    );
    let _ = std::fs::remove_file(&path);
    if let (Some(t), Ok(mut c)) = (saved, arboard::Clipboard::new()) {
        let _ = c.set_text(t);
    }

    // Network: the port is free (or Bridge is already running and owns it).
    check(
        "network",
        &mut failures,
        match std::net::TcpListener::bind(("0.0.0.0", cfg.port)) {
            Ok(_) => Ok(format!(
                "port {} free; LAN address {}",
                cfg.port,
                crate::pair::local_ip().map(|i| i.to_string()).unwrap_or("unknown".into())
            )),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Ok(format!("port {} in use (Bridge running?)", cfg.port)),
            Err(e) => Err(e.to_string()),
        },
    );

    match &cfg.peer {
        Some(p) => println!("  info  paired with {} ({:?}) last seen at {}", p.name, p.os, p.addr.as_deref().unwrap_or("?")),
        None => println!("  info  not paired yet"),
    }
    println!("{}", if failures == 0 { "All checks passed." } else { "Some checks failed." });
    failures
}
