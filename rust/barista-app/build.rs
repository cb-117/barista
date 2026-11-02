//! Windows: gives barista-app.exe Barista's cup and name, for the Start menu, the taskbar, Task
//! Manager, and Apps & features. Built only on Windows itself (the resource compiler is part of
//! Windows' SDK); cross-compiled checks from other systems skip it.
use std::env;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=BARISTA_VERSION");
    let windows_target = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let windows_host = env::var("HOST").is_ok_and(|host| host.contains("windows"));
    if !(windows_target && windows_host) {
        return;
    }
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon = Path::new(&manifest).join("../../installer/windows/barista.ico");
    let swift = Path::new(&manifest).join("../../Sources/BaristaCore/Version.swift");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed={}", swift.display());
    let version = env::var("BARISTA_VERSION").ok().or_else(|| {
        std::fs::read_to_string(&swift)
            .ok()?
            .lines()
            .find(|line| line.contains("static let current"))?
            .split('"')
            .nth(1)
            .map(String::from)
    });
    let version = version.unwrap_or_else(|| "0.0.0".into());
    // FILEVERSION wants four numbers.
    let mut parts: Vec<u64> = version.split('.').map(|part| part.parse().unwrap_or(0)).collect();
    parts.resize(4, 0);
    let packed = parts[0] << 48 | parts[1] << 32 | parts[2] << 16 | parts[3];

    let mut resource = winresource::WindowsResource::new();
    // Id 1: the window class and the installer's shortcuts load the icon by this id.
    resource
        .set_icon_with_id(&icon.to_string_lossy(), "1")
        .set("ProductName", "Barista")
        .set("FileDescription", "Barista")
        .set("CompanyName", "Barista")
        .set("LegalCopyright", "Barista contributors")
        .set("OriginalFilename", "barista-app.exe")
        .set("ProductVersion", &version)
        .set("FileVersion", &version)
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed);
    if let Err(error) = resource.compile() {
        // A missing resource compiler shouldn't stop the build; the app just has no icon.
        println!("cargo:warning=Couldn't embed Barista's icon: {error}");
    }
}
