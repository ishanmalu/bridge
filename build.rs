// Windows: embed the icon, file details (what Properties → Details and SmartScreen show) and an
// application manifest. Build scripts run on the host, so check the target here.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/bridge.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut r = winresource::WindowsResource::new();
        r.set_icon("assets/icon.ico")
            .set_manifest_file("assets/bridge.manifest")
            .set("ProductName", "Bridge")
            .set("FileDescription", "Bridge: one keyboard and mouse for a Mac and a PC")
            .set("CompanyName", "Ishan Malu")
            .set("LegalCopyright", "© 2026 Ishan Malu. PolyForm Noncommercial 1.0.0.")
            .set("OriginalFilename", "Bridge.exe")
            .set("InternalName", "Bridge")
            .set("Comments", "https://bridge.ishanmalu.dev");
        r.compile().unwrap();
    }
}
