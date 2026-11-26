//! Apply a verified Inno update to this installation. Setup replaces the binaries while the
//! existing tray process serves requests, then the new process inherits its sessions from a private
//! handoff file that whichever copy binds the control pipe first claims.
use crate::{lock, platform, App, Shared};
use barista_core::activity::{ActivityTracker, WatchedApp};
use barista_core::control::{default_path, state_dir};
use barista_core::update::{self, AvailableUpdate};
use barista_core::{now, CoffeeLease};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use winreg::{enums::*, RegKey};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn download(base: &str, name: &str, folder: &Path, max: usize) -> Result<PathBuf> {
    let file = folder.join(name);
    let status = update::curl()
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-time",
            "120",
            "--max-filesize",
            &max.to_string(),
            "--output",
        ])
        .arg(&file)
        .arg(format!("{base}/{name}"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() || file.metadata()?.len() > max as u64 {
        return Err("Could not download the signed Barista update.".into());
    }
    Ok(file)
}

fn expected_hash(manifest: &[u8], signature: &[u8], key: &[u8; 32], name: &str) -> Result<String> {
    let sig = STANDARD.decode(std::str::from_utf8(signature)?.trim())?;
    VerifyingKey::from_bytes(key)?.verify_strict(manifest, &Signature::from_slice(&sig)?)?;
    for line in std::str::from_utf8(manifest)?.lines() {
        if let Some((hash, filename)) = line.split_once("  ") {
            if filename == name && hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Ok(hash.to_ascii_lowercase());
            }
        }
    }
    Err("The signed release manifest did not list this installer.".into())
}

fn scope(folder: &Path) -> &'static str {
    let key = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{8F3C2A1E-6B4D-4E7A-9C51-2D7F0B3A9E64}_is1";
    for (root, flag) in [(HKEY_LOCAL_MACHINE, "/ALLUSERS"), (HKEY_CURRENT_USER, "/CURRENTUSER")] {
        let location = RegKey::predef(root)
            .open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
            .and_then(|k| k.get_value::<String, _>("InstallLocation"));
        if let Ok(location) = location {
            if fs::canonicalize(&location).is_ok_and(|path| path == folder) {
                return flag;
            }
        }
    }
    "/PORTABLEUPDATE"
}

const HANDOFF_FILE: &str = "update-handoff.json";

fn handoff_path(endpoint: &str) -> PathBuf {
    state_dir(endpoint).join(HANDOFF_FILE)
}

pub(super) fn install(app: &Shared, update: &AvailableUpdate) -> Result<()> {
    let exe = barista_core::connection::current_executable().with_file_name("barista-app.exe");
    if platform::is_development_build(&exe) {
        return Err("Development builds don't update themselves.".into());
    }
    let folder = exe.parent().ok_or("Cannot locate the installed app.")?;
    let installer = format!("Barista-Setup-{}.exe", update.version);
    let base = format!("https://github.com/cb-117/barista/releases/download/v{}", update.version);
    if update.download_url.as_deref() != Some(&format!("{base}/{installer}")) {
        return Err("This release has no supported Windows installer.".into());
    }
    let mode = scope(&fs::canonicalize(folder)?);
    // All-users Setup elevates; anything else must be able to write here before downloading.
    if mode != "/ALLUSERS" && tempfile::tempfile_in(folder).is_err() {
        return Err(format!("Barista can't write to {}.", folder.display()).into());
    }
    let staging = tempfile::Builder::new().prefix("Barista-update-").tempdir()?;
    let manifest = fs::read(download(&base, "SHA256SUMS.txt", staging.path(), 65536)?)?;
    let signature = fs::read(download(&base, "SHA256SUMS.txt.sig", staging.path(), 4096)?)?;
    let key: [u8; 32] = STANDARD
        .decode(env!("BARISTA_UPDATE_PUBLIC_KEY"))?
        .try_into()
        .map_err(|_| "Invalid release key.")?;
    let expected = expected_hash(&manifest, &signature, &key, &installer)?;
    let path = download(&base, &installer, staging.path(), 200 * 1024 * 1024)?;
    // Deny writes and replacement from verification until Setup exits.
    let mut locked = OpenOptions::new().read(true).share_mode(1).open(&path)?;
    let mut digest = Sha256::new();
    std::io::copy(&mut locked, &mut digest)?;
    if format!("{:x}", digest.finalize()) != expected {
        return Err("Installer signature verification failed.".into());
    }
    let mut cmd = Command::new(&path);
    cmd.current_dir(staging.path())
        .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/BARISTAAUTOUPDATE", "/NOLAUNCH"])
        .arg(mode)
        .arg(format!("/DIR={}", folder.display()))
        .stdin(Stdio::null());
    if mode == "/PORTABLEUPDATE" {
        cmd.arg("/CURRENTUSER");
    }
    let mut setup = {
        let state = lock(app);
        if !state.prefs.checks_for_updates {
            return Ok(());
        }
        cmd.spawn()?
    };
    if !setup.wait()?.success() {
        return Err("Windows Setup did not complete. Barista will retry later.".into());
    }
    drop(locked);
    drop(staging);
    // Hold the state lock through the handoff: a heartbeat or user stop cannot fall into the gap.
    let mut state = lock(app);
    state.session.refresh();
    let saved = handoff_path(&default_path());
    fs::create_dir_all(saved.parent().ok_or("Unable to hand off active sessions.")?)?;
    barista_core::hooks::write_atomically(&saved, &serde_json::to_vec(&Handoff::capture(&state))?).map_err(|e| e.to_string())?;
    let started = Command::new(&exe)
        .args(["--background", "--resume-update"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = started {
        let _ = fs::remove_file(&saved);
        return Err(error.into());
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn verifies_the_versioned_installer_in_the_signed_manifest() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let name = "Barista-Setup-1.2.3.exe";
        let hash = format!("{:x}", Sha256::digest(b"synthetic installer"));
        let manifest = format!("{hash}  {name}\n");
        let signature = STANDARD.encode(key.sign(manifest.as_bytes()).to_bytes());
        let verify = |data: &str, file: &str| expected_hash(data.as_bytes(), signature.as_bytes(), key.verifying_key().as_bytes(), file);
        assert_eq!(verify(&manifest, name).unwrap(), hash);
        assert!(verify(&manifest, "Barista-Setup-1.2.4.exe").is_err());
        assert!(verify(&manifest.replace("1.2.3", "1.2.4"), "Barista-Setup-1.2.4.exe").is_err());
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Handoff {
    saved_at: f64,
    sessions: Vec<CoffeeLease>,
    stop_token: String,
    activity: HashMap<String, ActivityTracker>,
    leases: HashMap<String, String>,
    stopped: HashSet<String>,
    burst: HashMap<String, f64>,
}

impl Handoff {
    fn capture(app: &App) -> Self {
        Self {
            saved_at: now(),
            sessions: app.session.sessions().to_vec(),
            stop_token: app.session.stop_token().into(),
            activity: app.activity.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            leases: app.activity_leases.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
            stopped: app.activity_stopped.iter().map(|k| k.to_string()).collect(),
            burst: app.activity_burst.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }
    /// Takes the handoff left for `endpoint`, if any. The rename lets only one starting copy have it.
    pub(super) fn claim(endpoint: &str) -> Option<Self> {
        let saved = handoff_path(endpoint);
        let claimed = saved.with_file_name(format!("update-handoff.{}.json", std::process::id()));
        fs::rename(&saved, &claimed).ok()?;
        let mut text = Vec::new();
        let read = fs::File::open(&claimed).and_then(|file| file.take(4 * 1024 * 1024).read_to_end(&mut text));
        let _ = fs::remove_file(&claimed);
        read.ok()?;
        serde_json::from_slice(&text).ok()
    }
    pub(super) fn restore(mut self, app: &mut App) {
        // A little backward skew: the clock can step during the restart.
        if !(-5.0..=300.0).contains(&(now() - self.saved_at)) {
            return;
        }
        app.session.resume_after_update(self.sessions, self.stop_token);
        for watched in WatchedApp::ALL {
            let id = watched.id;
            if let Some(v) = self.activity.remove(id) {
                app.activity.insert(id, v);
            }
            if let Some(v) = self.leases.remove(id) {
                app.activity_leases.insert(id, v);
            }
            if self.stopped.contains(id) {
                app.activity_stopped.insert(id);
            }
            if let Some(v) = self.burst.remove(id) {
                app.activity_burst.insert(id, v);
            }
        }
    }
}
