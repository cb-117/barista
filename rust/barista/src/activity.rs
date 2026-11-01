//! Desktop apps that run agents but offer no hooks: the Claude app (Cowork) and the ChatGPT app.
//! Barista keeps the computer awake while they're busy, judged by the CPU time of the app and its
//! helper processes. Mirrors Sources/BaristaCore/AppActivity.swift; see docs/AGENTS.md.
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchedApp {
    pub id: &'static str,
    /// The awake session's title.
    pub title: &'static str,
}

impl WatchedApp {
    pub const COWORK: WatchedApp = WatchedApp {
        id: "cowork",
        title: "Claude app",
    };
    pub const CHATGPT: WatchedApp = WatchedApp {
        id: "chatgpt",
        title: "ChatGPT app",
    };
    pub const ALL: [WatchedApp; 2] = [Self::COWORK, Self::CHATGPT];

    /// Whether an executable (lowercased file name and full path) is this app's main program.
    /// The Claude desktop app lives under `AnthropicClaude`, which keeps out the Claude Code CLI.
    fn is_main(self, name: &str, path: &str) -> bool {
        match self.id {
            "cowork" => name == "claude.exe" && path.contains("\\anthropicclaude\\"),
            _ => name == "chatgpt.exe",
        }
    }
}

/// When an app counts as busy. Idle, the Claude and ChatGPT apps use about 1–2% of one core.
pub mod policy {
    pub const SAMPLE_INTERVAL: f64 = 10.0;
    /// Busy: at least `BUSY_SHARE` of one core, averaged over `WINDOW` seconds.
    pub const WINDOW: f64 = 30.0;
    pub const BUSY_SHARE: f64 = 0.10;
    /// Each busy sample keeps the computer awake this long, so pauses while the agent waits on
    /// the server don't let it sleep.
    pub const LEASE: f64 = 300.0;
    /// After you stop an app's session, it stays stopped until the app has been quiet this long.
    pub const QUIET_BEFORE_RESTART: f64 = 120.0;
    pub const HARD_CAP: f64 = 28800.0;
}

/// Decides, sample by sample, whether an app is busy. Pure, so it can be tested with made-up samples.
#[derive(Clone, Debug, Default)]
pub struct ActivityTracker {
    previous: HashMap<i32, f64>,
    /// (end, seconds, cpu seconds used)
    intervals: Vec<(f64, f64, f64)>,
    last_sample: Option<f64>,
    pub last_busy: Option<f64>,
}

impl ActivityTracker {
    /// Records the app's per-process CPU seconds (empty when it isn't running) at `time` (seconds).
    /// Returns whether it's busy.
    pub fn record(&mut self, cpu: &HashMap<i32, f64>, time: f64) -> bool {
        let busy = self.evaluate(cpu, time);
        self.last_sample = Some(time);
        self.previous = cpu.clone();
        busy
    }

    fn evaluate(&mut self, cpu: &HashMap<i32, f64>, time: f64) -> bool {
        let Some(last) = self.last_sample.filter(|_| !cpu.is_empty() && !self.previous.is_empty()) else {
            self.intervals.clear();
            return false;
        };
        // Processes that started since the last sample count from zero.
        let used: f64 = cpu
            .iter()
            .map(|(pid, now)| (now - self.previous.get(pid).copied().unwrap_or(0.0)).max(0.0))
            .sum();
        self.intervals.push((time, (time - last).max(0.001), used));
        self.intervals.retain(|(end, _, _)| time - end < policy::WINDOW);
        let seconds: f64 = self.intervals.iter().map(|i| i.1).sum();
        let busy = seconds > 0.0 && self.intervals.iter().map(|i| i.2).sum::<f64>() / seconds >= policy::BUSY_SHARE;
        if busy {
            self.last_busy = Some(time);
        }
        busy
    }
}

/// Each app's root process and the CPU seconds of it and its descendants (empty when not running).
pub fn sample(apps: &[WatchedApp]) -> Vec<(Option<i32>, HashMap<i32, f64>)> {
    let table = imp::processes();
    apps.iter().map(|app| tree(*app, &table)).collect()
}

/// A process as (pid, parent, lowercased file name, lowercased full path).
type Process = (i32, i32, String, String);

fn tree(app: WatchedApp, table: &[Process]) -> (Option<i32>, HashMap<i32, f64>) {
    let matches: Vec<i32> = table.iter().filter(|p| app.is_main(&p.2, &p.3)).map(|p| p.0).collect();
    let roots: Vec<i32> = table
        .iter()
        .filter(|p| matches.contains(&p.0) && !matches.contains(&p.1))
        .map(|p| p.0)
        .collect();
    if roots.is_empty() {
        return (None, HashMap::new());
    }
    let mut members = roots.clone();
    let mut grew = true;
    while grew {
        grew = false;
        for process in table {
            if !members.contains(&process.0) && members.contains(&process.1) {
                members.push(process.0);
                grew = true;
            }
        }
    }
    let cpu = members
        .iter()
        .filter_map(|pid| imp::cpu_seconds(*pid).map(|seconds| (*pid, seconds)))
        .collect();
    (roots.iter().min().copied(), cpu)
}

#[cfg(windows)]
mod imp {
    use super::Process;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn path(pid: i32) -> String {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
            if handle.is_null() {
                return String::new();
            }
            let mut buffer = [0u16; 1024];
            let mut length = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut length) != 0;
            CloseHandle(handle);
            if ok {
                String::from_utf16_lossy(&buffer[..length as usize]).to_lowercase()
            } else {
                String::new()
            }
        }
    }

    pub fn processes() -> Vec<Process> {
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
                let pid = entry.th32ProcessID as i32;
                // Only the apps' own executables need a path.
                let full = if name == "claude.exe" || name == "chatgpt.exe" {
                    path(pid)
                } else {
                    String::new()
                };
                result.push((pid, entry.th32ParentProcessID as i32, name, full));
                more = Process32NextW(snapshot, &mut entry) != 0;
            }
            CloseHandle(snapshot);
        }
        result
    }

    /// Kernel + user time, in seconds.
    pub fn cpu_seconds(pid: i32) -> Option<f64> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid as u32);
            if handle.is_null() {
                return None;
            }
            let zero = FILETIME {
                dwLowDateTime: 0,
                dwHighDateTime: 0,
            };
            let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
            let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
            CloseHandle(handle);
            let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
            ok.then(|| (ticks(kernel) + ticks(user)) as f64 / 1e7)
        }
    }
}

/// Neither app exists for Linux; the Mac app (Swift) watches them on macOS.
#[cfg(not(windows))]
mod imp {
    use super::Process;
    pub fn processes() -> Vec<Process> {
        Vec::new()
    }
    pub fn cpu_seconds(_pid: i32) -> Option<f64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu(entries: &[(i32, f64)]) -> HashMap<i32, f64> {
        entries.iter().copied().collect()
    }

    /// The same scenario as the Swift check `watchedAppActivity`.
    #[test]
    fn busy_by_cpu_share_over_a_window() {
        let mut tracker = ActivityTracker::default();
        let start = 1_000_000.0;
        assert!(
            !tracker.record(&cpu(&[(1, 5.0), (2, 1.0)]), start),
            "first sample has nothing to compare against"
        );
        assert!(
            !tracker.record(&cpu(&[(1, 5.1), (2, 1.05)]), start + 10.0),
            "1.5% of a core is idle"
        );
        assert!(tracker.record(&cpu(&[(1, 8.0), (2, 1.1)]), start + 20.0), "a burst of work is busy");
        assert!(
            tracker.record(&cpu(&[(1, 8.1), (2, 1.15), (3, 0.4)]), start + 30.0),
            "a new helper counts; the window still holds the burst"
        );
        assert!(
            !tracker.record(&cpu(&[(1, 8.15), (2, 1.15), (3, 0.4)]), start + 60.0),
            "quiet once the burst leaves the window"
        );
        assert_eq!(tracker.last_busy, Some(start + 30.0));
        assert!(
            !tracker.record(&HashMap::new(), start + 70.0),
            "an app that isn't running is never busy"
        );
        assert!(!tracker.record(&cpu(&[(1, 0.0)]), start + 80.0), "a relaunch starts over");
    }

    #[test]
    fn trees_start_at_the_apps_main_process() {
        let table: Vec<Process> = vec![
            (
                10,
                1,
                "claude.exe".into(),
                "c:\\users\\me\\appdata\\local\\anthropicclaude\\app-1\\claude.exe".into(),
            ),
            (
                11,
                10,
                "claude.exe".into(),
                "c:\\users\\me\\appdata\\local\\anthropicclaude\\app-1\\claude.exe".into(),
            ),
            (12, 11, "node.exe".into(), String::new()),
            (20, 5, "claude.exe".into(), "c:\\users\\me\\.local\\bin\\claude.exe".into()),
            (
                30,
                1,
                "chatgpt.exe".into(),
                "c:\\program files\\windowsapps\\openai\\chatgpt.exe".into(),
            ),
        ];
        let (root, members) = tree(WatchedApp::COWORK, &table);
        assert_eq!(root, Some(10), "the Claude Code CLI isn't the Claude app");
        let _ = members; // CPU reads need real processes; membership is covered below.
        let matches: Vec<i32> = table
            .iter()
            .filter(|p| WatchedApp::COWORK.is_main(&p.2, &p.3))
            .map(|p| p.0)
            .collect();
        assert_eq!(matches, vec![10, 11]);
        assert!(WatchedApp::CHATGPT.is_main("chatgpt.exe", ""));
        assert_eq!(tree(WatchedApp::CHATGPT, &table).0, Some(30));
        assert_eq!(tree(WatchedApp::COWORK, &[]).0, None);
    }
}
