//! Reads the one version number, kept in the Swift package, so every build of Barista agrees.
use std::{env, fs, path::Path};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let source = Path::new(&manifest).join("../../Sources/BaristaCore/Version.swift");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-env-changed=BARISTA_VERSION");
    let version = env::var("BARISTA_VERSION").ok().or_else(|| {
        let text = fs::read_to_string(&source).ok()?;
        let line = text.lines().find(|line| line.contains("static let current"))?;
        let start = line.find('"')? + 1;
        let end = start + line[start..].find('"')?;
        Some(line[start..end].to_string())
    });
    let version = version.expect("Couldn't read the version from Sources/BaristaCore/Version.swift (or set BARISTA_VERSION).");
    assert!(
        !version.is_empty()
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())),
        "Bad version: {version}"
    );
    println!("cargo:rustc-env=BARISTA_VERSION={version}");
}
