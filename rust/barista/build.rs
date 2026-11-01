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

    // The one price table for usage estimates also lives in the Swift package.
    let prices = Path::new(&manifest).join("../../Sources/BaristaCore/ModelPrices.swift");
    println!("cargo:rerun-if-changed={}", prices.display());
    let text = fs::read_to_string(&prices).expect("Couldn't read Sources/BaristaCore/ModelPrices.swift.");
    let anchor = text.find("static let json").expect("ModelPrices.swift has no `static let json`");
    let start = anchor + text[anchor..].find("#\"\"\"").expect("ModelPrices.swift has no #\"\"\" block") + 4;
    let end = start
        + text[start..]
            .find("\"\"\"#")
            .expect("ModelPrices.swift's #\"\"\" block isn't closed");
    let json = text[start..end].trim();
    assert!(
        json.starts_with('[') && json.ends_with(']'),
        "ModelPrices.swift's price table must be a JSON array"
    );
    let checked = text
        .lines()
        .find(|line| line.contains("static let checkedOn"))
        .and_then(|line| line.split('"').nth(1))
        .expect("ModelPrices.swift has no checkedOn date");
    let out = env::var("OUT_DIR").unwrap();
    fs::write(Path::new(&out).join("model-prices.json"), json).unwrap();
    println!("cargo:rustc-env=BARISTA_PRICES_CHECKED={checked}");

    // The visual report's page (`barista report`) is the same template on every platform.
    let template = Path::new(&manifest).join("../../Sources/BaristaCore/ReportTemplate.swift");
    println!("cargo:rerun-if-changed={}", template.display());
    let text = fs::read_to_string(&template).expect("Couldn't read Sources/BaristaCore/ReportTemplate.swift.");
    let anchor = text.find("static let html").expect("ReportTemplate.swift has no `static let html`");
    let start = anchor + text[anchor..].find("#\"\"\"").expect("ReportTemplate.swift has no #\"\"\" block") + 4;
    let end = start
        + text[start..]
            .find("\"\"\"#")
            .expect("ReportTemplate.swift's #\"\"\" block isn't closed");
    // Like Swift, drop the line break after the opening marker and before the closing one.
    let html = text[start..end]
        .strip_prefix("\r\n")
        .or_else(|| text[start..end].strip_prefix('\n'))
        .unwrap_or(&text[start..end]);
    let html = html.strip_suffix('\n').map(|h| h.strip_suffix('\r').unwrap_or(h)).unwrap_or(html);
    assert!(
        html.matches("/*BARISTA_REPORT_DATA*/null").count() == 1,
        "ReportTemplate.swift's page must hold /*BARISTA_REPORT_DATA*/null exactly once"
    );
    fs::write(Path::new(&out).join("report-template.html"), html).unwrap();
}
