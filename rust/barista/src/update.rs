//! Checks GitHub for a newer release. Sends no identifiers beyond a standard HTTPS request.
use crate::models::VERSION;
use serde_json::Value;
use std::process::{Command, Stdio};

pub const LATEST_RELEASE_API: &str = "https://api.github.com/repos/cb-117/barista/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/cb-117/barista/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub struct AvailableUpdate {
    pub version: String,
    /// The release page, always available.
    pub page_url: String,
    /// This platform's download, when the release has one.
    pub download_url: Option<String>,
}

/// This platform's release downloads, best first: on Windows the installer
/// (`Barista-Setup-0.0.11.exe`), then the portable zip; on Linux the tarball
/// (`barista-0.0.6-linux-x86_64.tar.gz`).
pub fn asset_names(version: &str) -> Vec<String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    if cfg!(windows) {
        vec![
            format!("Barista-Setup-{version}.exe"),
            format!("barista-{version}-windows-{arch}.zip"),
        ]
    } else {
        vec![format!("barista-{version}-linux-{arch}.tar.gz")]
    }
}

/// Parses GitHub's "latest release" JSON. Returns None unless that release is newer than `current`.
pub fn newer_release(data: &[u8], current: &str) -> Option<AvailableUpdate> {
    let release: Value = serde_json::from_slice(data).ok()?;
    let tag = release.get("tag_name")?.as_str()?;
    if release.get("draft").and_then(Value::as_bool) == Some(true)
        || release.get("prerelease").and_then(Value::as_bool) == Some(true)
        || !is_newer(tag, current)
    {
        return None;
    }
    let version = normalized(tag).to_string();
    let page_url = release
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| trusted(u))
        .unwrap_or(RELEASES_PAGE)
        .to_string();
    let assets: Vec<&Value> = release.get("assets").and_then(Value::as_array).into_iter().flatten().collect();
    let download_url = asset_names(&version).iter().find_map(|wanted| {
        assets
            .iter()
            .find(|asset| asset.get("name").and_then(Value::as_str) == Some(wanted.as_str()))
            .and_then(|asset| asset.get("browser_download_url")?.as_str())
            .filter(|u| trusted(u))
            .map(String::from)
    });
    Some(AvailableUpdate {
        version,
        page_url,
        download_url,
    })
}

/// Compares dotted numeric versions ("v1.2" == "1.2.0"). Anything unparseable is never newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some(a), Some(b)) = (components(candidate), components(current)) else {
        return false;
    };
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn normalized(tag: &str) -> &str {
    tag.strip_prefix(['v', 'V']).unwrap_or(tag)
}

fn components(version: &str) -> Option<Vec<u64>> {
    let parts: Vec<&str> = normalized(version.trim()).split('.').collect();
    if !(1..=4).contains(&parts.len()) {
        return None;
    }
    parts
        .iter()
        .map(|part| {
            if !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()) {
                part.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

/// Only ever open or download GitHub URLs from release data.
fn trusted(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").to_lowercase();
    host == "github.com" || host.ends_with(".github.com") || host == "objects.githubusercontent.com"
}

/// The system's `curl`, ignoring the user's `.curlrc` (`-q` must come first) and, on Windows, without
/// opening a console window.
pub fn curl() -> Command {
    let mut command = Command::new("curl");
    command.arg("-q");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Asks GitHub for the latest release with the system's `curl` (built into Windows 10 and later,
/// and nearly every Linux), so Barista ships no TLS stack of its own.
/// Ok(None): up to date or nothing released yet. Err: GitHub couldn't be reached.
pub fn check() -> Result<Option<AvailableUpdate>, String> {
    let output = curl()
        .args(["-sS", "-L", "--max-time", "15", "-w", "\n%{http_code}"])
        .args(["-H", "Accept: application/vnd.github+json"])
        .args(["-H", &format!("User-Agent: Barista/{VERSION}")])
        .arg(LATEST_RELEASE_API)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let (body, status) = text.rsplit_once('\n').unwrap_or(("", ""));
    match status.trim() {
        "200" => Ok(newer_release(body.as_bytes(), VERSION)),
        "404" => Ok(None), // Nothing has been released yet.
        other => Err(format!("GitHub replied {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("v0.0.10", "0.0.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("1.0-beta", "0.1"));
        assert!(!is_newer("", "0.1"));
    }

    #[test]
    fn prefers_the_installer_on_windows() {
        let names = asset_names("9.0.0");
        let asset = |name: &str| {
            format!(r#"{{"name":"{name}","browser_download_url":"https://github.com/cb-117/barista/releases/download/v9.0.0/{name}"}}"#)
        };
        let all = names.iter().map(|n| asset(n)).collect::<Vec<_>>().join(",");
        let release = format!(r#"{{"tag_name":"v9.0.0","assets":[{all}]}}"#);
        let update = newer_release(release.as_bytes(), "0.0.5").unwrap();
        assert!(update.download_url.unwrap().ends_with(&names[0]));
        if cfg!(windows) {
            assert_eq!(names[0], "Barista-Setup-9.0.0.exe");
            // A release without the installer falls back to the zip.
            let zip_only = format!(r#"{{"tag_name":"v9.0.0","assets":[{}]}}"#, asset(&names[1]));
            assert!(newer_release(zip_only.as_bytes(), "0.0.5")
                .unwrap()
                .download_url
                .unwrap()
                .ends_with(".zip"));
        }
    }

    #[test]
    fn finds_this_platforms_download() {
        let name = asset_names("9.0.0").last().unwrap().clone();
        let release = format!(
            r#"{{"tag_name":"v9.0.0","html_url":"https://github.com/cb-117/barista/releases/tag/v9.0.0",
            "assets":[{{"name":"{name}","browser_download_url":"https://github.com/cb-117/barista/releases/download/v9.0.0/{name}"}},
            {{"name":"Barista.dmg","browser_download_url":"https://evil.example/Barista.dmg"}}]}}"#
        );
        let update = newer_release(release.as_bytes(), "0.0.5").unwrap();
        assert_eq!(update.version, "9.0.0");
        assert!(update.download_url.unwrap().ends_with(&name));
        assert!(newer_release(release.as_bytes(), "9.0.0").is_none());
        let untrusted = release.replace("https://github.com/cb-117/barista/releases/download", "https://evil.example");
        assert_eq!(newer_release(untrusted.as_bytes(), "0.0.5").unwrap().download_url, None);
    }
}
