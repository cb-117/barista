//! Process and battery inspection for each platform.
use crate::models::BatteryState;

/// A start-time token, so a reused PID never inherits somebody else's session.
/// None when the process isn't running (or can't be inspected).
pub fn process_identity(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    imp::process_identity(pid)
}

/// The built-in battery, or None on desktops and when it can't be read.
pub fn battery() -> Option<BatteryState> {
    imp::battery()
}

/// (parent PID, executable name without extension, lowercased on Windows).
pub fn parent_and_name(pid: i32) -> Option<(i32, String)> {
    if pid <= 0 {
        return None;
    }
    imp::parent_and_name(pid)
}

pub fn child_pids(parent: i32) -> Vec<i32> {
    imp::child_pids(parent)
}

/// The process's arguments joined by spaces (never its environment), or None if unreadable.
pub fn command_line(pid: i32) -> Option<String> {
    imp::command_line(pid)
}

pub fn current_pid() -> i32 {
    std::process::id() as i32
}

pub fn parent_pid() -> Option<i32> {
    #[cfg(unix)]
    return Some(std::os::unix::process::parent_id() as i32);
    #[cfg(not(unix))]
    return parent_and_name(current_pid()).map(|(parent, _)| parent);
}

/// The nearest ancestor that isn't a shell: the agent process that ran this hook.
pub fn agent_process_id() -> Option<i32> {
    const SHELLS: [&str; 10] = ["sh", "bash", "zsh", "dash", "fish", "env", "barista", "cmd", "powershell", "pwsh"];
    let mut current = parent_pid()?;
    for _ in 0..8 {
        if current <= 1 {
            return None;
        }
        let (parent, name) = parent_and_name(current)?;
        if !SHELLS.contains(&name.trim_matches('-')) {
            return Some(current);
        }
        current = parent;
    }
    None
}

/// This process and its ancestors, so a hook never counts its own shell as background work.
pub fn ancestry() -> Vec<i32> {
    let mut chain = Vec::new();
    let mut current = current_pid();
    for _ in 0..16 {
        if current <= 1 || chain.contains(&current) {
            break;
        }
        chain.push(current);
        match parent_and_name(current) {
            Some((parent, _)) => current = parent,
            None => break,
        }
    }
    chain
}

/// Claude Code's Bash tool runs each command as `<shell> -c source …/.claude/shell-snapshots/…`.
/// Once Claude has stopped, any of those still running are background commands it's waiting on.
/// Long-lived children such as MCP servers don't match, so they never keep the computer awake.
pub fn background_command_pids(agent: i32, excluding: &[i32]) -> Vec<i32> {
    child_pids(agent)
        .into_iter()
        .filter(|pid| !excluding.contains(pid) && command_line(*pid).is_some_and(|line| line.contains("/.claude/shell-snapshots/")))
        .collect()
}

#[cfg(target_os = "linux")]
mod imp {
    use super::BatteryState;
    use std::fs;

    /// Fields after the parenthesized command name in /proc/PID/stat, plus that name.
    fn stat(pid: i32) -> Option<(String, Vec<String>)> {
        let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let open = text.find('(')?;
        let close = text.rfind(')')?;
        let name = text[open + 1..close].to_string();
        let fields = text[close + 1..].split_whitespace().map(String::from).collect();
        Some((name, fields))
    }

    pub fn process_identity(pid: i32) -> Option<String> {
        let (_, fields) = stat(pid)?;
        // A zombie has exited; it's only waiting for its parent to collect it.
        if fields.first().map(String::as_str) == Some("Z") {
            return None;
        }
        fields.get(19).cloned() // starttime, in clock ticks since boot
    }

    pub fn parent_and_name(pid: i32) -> Option<(i32, String)> {
        let (name, fields) = stat(pid)?;
        Some((fields.get(1)?.parse().ok()?, name))
    }

    pub fn child_pids(parent: i32) -> Vec<i32> {
        let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
        entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
            .filter(|pid| parent_and_name(*pid).is_some_and(|(ppid, _)| ppid == parent))
            .collect()
    }

    pub fn command_line(pid: i32) -> Option<String> {
        let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let arguments: Vec<String> = bytes
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        Some(arguments.join(" "))
    }

    pub fn battery() -> Option<BatteryState> {
        let mut battery = None;
        let mut external_power = false;
        for entry in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let path = entry.path();
            let read = |name: &str| fs::read_to_string(path.join(name)).map(|s| s.trim().to_string()).ok();
            match read("type").as_deref() {
                Some("Mains") | Some("USB") => external_power |= read("online").as_deref() == Some("1"),
                // Only the computer's own battery; "scope: Device" marks mice, headsets, and the like.
                Some("Battery") if read("scope").as_deref() != Some("Device") && battery.is_none() => {
                    let percent = read("capacity").and_then(|c| c.parse::<i32>().ok()).or_else(|| {
                        let now: f64 = read("energy_now").or_else(|| read("charge_now"))?.parse().ok()?;
                        let full: f64 = read("energy_full").or_else(|| read("charge_full"))?.parse().ok()?;
                        (full > 0.0).then(|| (now / full * 100.0).round() as i32)
                    });
                    if let Some(percent) = percent {
                        battery = Some((percent.clamp(0, 100), read("status").as_deref() == Some("Discharging")));
                    }
                }
                _ => {}
            }
        }
        battery.map(|(percent, discharging)| BatteryState {
            percent,
            on_battery: discharging && !external_power,
        })
    }
}

#[cfg(windows)]
mod imp {
    use super::BatteryState;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    pub fn process_identity(pid: i32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
            if handle.is_null() {
                return None;
            }
            let mut code = 0u32;
            let running = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
            let zero = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
            let timed = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
            CloseHandle(handle);
            (running && timed).then(|| format!("{}", (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)))
        }
    }

    /// Every process as (pid, parent pid, lowercased name without ".exe").
    fn processes() -> Vec<(i32, i32, String)> {
        let mut result = Vec::new();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return result;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snapshot, &mut entry) != 0;
            while more {
                let length = entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..length]).to_lowercase();
                let name = name.strip_suffix(".exe").unwrap_or(&name).to_string();
                result.push((entry.th32ProcessID as i32, entry.th32ParentProcessID as i32, name));
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        result
    }

    pub fn parent_and_name(pid: i32) -> Option<(i32, String)> {
        processes().into_iter().find(|p| p.0 == pid).map(|(_, parent, name)| (parent, name))
    }

    pub fn child_pids(parent: i32) -> Vec<i32> {
        processes().into_iter().filter(|p| p.1 == parent).map(|p| p.0).collect()
    }

    /// Not read on Windows; background-command tracking after Claude's Stop is Unix-only.
    pub fn command_line(_pid: i32) -> Option<String> {
        None
    }

    pub fn battery() -> Option<BatteryState> {
        let mut status: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { GetSystemPowerStatus(&mut status) } == 0 {
            return None;
        }
        // 128: no system battery. 255: unknown.
        if status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255 || status.BatteryLifePercent == 255 {
            return None;
        }
        Some(BatteryState {
            percent: i32::from(status.BatteryLifePercent).min(100),
            on_battery: status.ACLineStatus == 0,
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    //! For development builds; the Mac app is the Swift one.
    use super::BatteryState;
    use std::ffi::c_void;

    fn info(pid: i32) -> Option<libc::proc_bsdinfo> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut c_void, size) };
        (read == size).then_some(info)
    }

    pub fn process_identity(pid: i32) -> Option<String> {
        info(pid).map(|info| format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec))
    }

    pub fn parent_and_name(pid: i32) -> Option<(i32, String)> {
        let info = info(pid)?;
        let bytes: Vec<u8> = info.pbi_comm.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        Some((info.pbi_ppid as i32, String::from_utf8_lossy(&bytes).into_owned()))
    }

    pub fn child_pids(parent: i32) -> Vec<i32> {
        const PROC_PPID_ONLY: u32 = 6;
        let mut pids = vec![0i32; 1024];
        let bytes = unsafe {
            libc::proc_listpids(
                PROC_PPID_ONLY,
                parent as u32,
                pids.as_mut_ptr() as *mut c_void,
                (pids.len() * 4) as i32,
            )
        };
        if bytes <= 0 {
            return Vec::new();
        }
        pids.truncate(bytes as usize / 4);
        pids.retain(|pid| *pid > 0);
        pids
    }

    /// KERN_PROCARGS2 is argc, the executable path, padding, the arguments, then the environment.
    pub fn command_line(pid: i32) -> Option<String> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size = 0usize;
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 || size <= 4 {
            return None;
        }
        let mut buffer = vec![0u8; size];
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buffer.as_mut_ptr() as *mut c_void,
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
            || size <= 4
        {
            return None;
        }
        let argc = i32::from_ne_bytes(buffer[..4].try_into().ok()?) as usize;
        let mut index = 4;
        while index < size && buffer[index] != 0 {
            index += 1;
        }
        while index < size && buffer[index] == 0 {
            index += 1;
        }
        let mut arguments = Vec::new();
        while arguments.len() < argc && index < size {
            let start = index;
            while index < size && buffer[index] != 0 {
                index += 1;
            }
            arguments.push(String::from_utf8_lossy(&buffer[start..index]).into_owned());
            index += 1;
        }
        Some(arguments.join(" "))
    }

    pub fn battery() -> Option<BatteryState> {
        None
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::BatteryState;
    pub fn process_identity(_pid: i32) -> Option<String> {
        None
    }
    pub fn parent_and_name(_pid: i32) -> Option<(i32, String)> {
        None
    }
    pub fn child_pids(_parent: i32) -> Vec<i32> {
        Vec::new()
    }
    pub fn command_line(_pid: i32) -> Option<String> {
        None
    }
    pub fn battery() -> Option<BatteryState> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_inspectable() {
        let pid = current_pid();
        assert!(process_identity(pid).is_some());
        assert_eq!(process_identity(pid), process_identity(pid), "identity is stable");
        assert!(process_identity(-1).is_none());
        assert!(ancestry().contains(&pid));
        if let Some(parent) = parent_pid() {
            assert!(child_pids(parent).contains(&pid));
        }
    }

    #[cfg(unix)]
    #[test]
    fn exited_children_lose_their_identity() {
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let pid = child.id() as i32;
        assert!(process_identity(pid).is_some());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(process_identity(pid).is_none());
    }
}
