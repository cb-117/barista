//! The anonymous active-user count (docs/ACTIVE-USERS.md): when the app's daily ping is due, what it
//! says, and sending it. Only the app pings; the CLI, the hooks, and the MCP server never do.
use crate::models::VERSION;
use chrono::{Datelike, NaiveDate};
use std::process::Stdio;

pub const PING_URL: &str = "https://baristatime.app/api/ping";

/// `os` in the ping. The Mac app is the Swift one; `mac` is here so this crate builds and tests there.
pub const OS: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "mac"
} else {
    "linux"
};

/// What a ping says besides "first today": whether it is also this copy's first of the ISO week, of
/// the calendar month, and ever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags {
    pub weekly: bool,
    pub monthly: bool,
    pub new: bool,
}

/// Today's date in UTC.
pub fn today() -> NaiveDate {
    chrono::Utc::now().date_naive()
}

/// A date as it is remembered: `YYYY-MM-DD`.
pub fn date_text(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// A remembered date: exactly `YYYY-MM-DD`, and a real day. Anything else counts as none.
fn remembered(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    let well_formed = bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        });
    if !well_formed {
        return None;
    }
    NaiveDate::from_ymd_opt(text[..4].parse().ok()?, text[5..7].parse().ok()?, text[8..].parse().ok()?)
}

/// The flags for a ping on `today` (UTC) given the date of the last successful one, or None when
/// that was today, so none is due. Dates are compared by inequality: a last ping dated after today
/// (the clock moved back) still pings.
pub fn due(last: Option<&str>, today: NaiveDate) -> Option<Flags> {
    let Some(last) = last.and_then(remembered) else {
        return Some(Flags {
            weekly: true,
            monthly: true,
            new: true,
        });
    };
    if last == today {
        return None;
    }
    let (last_week, this_week) = (last.iso_week(), today.iso_week());
    Some(Flags {
        weekly: (last_week.year(), last_week.week()) != (this_week.year(), this_week.week()),
        monthly: (last.year(), last.month()) != (today.year(), today.month()),
        new: false,
    })
}

/// Where pings go: `BARISTA_PING_URL` if set (for tests), else baristatime.app.
pub fn base_url() -> String {
    std::env::var("BARISTA_PING_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| PING_URL.to_string())
}

/// The whole request: everything a ping says is in its query.
pub fn url_with(base: &str, version: &str, os: &str, flags: Flags) -> String {
    let bit = |flag: bool| if flag { 1 } else { 0 };
    format!(
        "{base}?v={version}&os={os}&d=1&w={}&m={}&n={}",
        bit(flags.weekly),
        bit(flags.monthly),
        bit(flags.new)
    )
}

/// This copy's request for a ping with `flags`.
pub fn url(flags: Flags) -> String {
    url_with(&base_url(), VERSION, OS, flags)
}

/// Sends one ping with the system's `curl` (see `update::curl`): no redirects, cookies, or cache, and
/// the reply's body is thrown away. Besides the Host every request needs, the only header is
/// User-Agent (`Accept:` drops the `Accept: */*` curl would add). True on any HTTP 2xx; false on
/// anything else, including no reply.
pub fn send(flags: Flags) -> bool {
    send_to(&url(flags))
}

fn send_to(url: &str) -> bool {
    let nowhere = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = crate::update::curl()
        .args(["-sS", "--max-time", "15", "-o", nowhere, "-w", "%{http_code}"])
        .args(["-H", &format!("User-Agent: Barista/{VERSION}"), "-H", "Accept:"])
        .arg(url)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match output {
        Ok(output) => output.status.success() && is_success(String::from_utf8_lossy(&output.stdout).trim()),
        Err(_) => false,
    }
}

/// An HTTP status as curl prints it (`204`; `000` when there was no reply).
fn is_success(status: &str) -> bool {
    status.len() == 3 && status.starts_with('2') && status.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        remembered(text).unwrap()
    }

    fn flags(last: &str, today: &str) -> Option<(bool, bool, bool)> {
        due(Some(last), day(today)).map(|f| (f.weekly, f.monthly, f.new))
    }

    #[test]
    fn the_first_ping_ever_counts_everywhere() {
        let first = due(None, day("2026-11-09")).unwrap();
        assert_eq!((first.weekly, first.monthly, first.new), (true, true, true));
    }

    #[test]
    fn one_ping_a_day() {
        assert_eq!(flags("2026-11-09", "2026-11-09"), None);
        // Monday, then Tuesday of the same week and month.
        assert_eq!(flags("2026-11-09", "2026-11-10"), Some((false, false, false)));
    }

    #[test]
    fn weeks_and_months_turn_over_separately() {
        // Sunday, then Monday: a new ISO week inside November.
        assert_eq!(flags("2026-11-15", "2026-11-16"), Some((true, false, false)));
        // Monday, then Tuesday: a new month inside one ISO week.
        assert_eq!(flags("2026-11-30", "2026-12-01"), Some((false, true, false)));
        // Days apart but far more than a week: both.
        assert_eq!(flags("2026-09-30", "2026-11-09"), Some((true, true, false)));
        // The same week number and month a year later is another week and month.
        assert_eq!(flags("2025-11-10", "2026-11-09"), Some((true, true, false)));
    }

    #[test]
    fn iso_weeks_cross_the_new_year() {
        // Thursday 2026-12-31 and Friday 2027-01-01 are both in ISO week 2026-W53.
        assert_eq!(flags("2026-12-31", "2027-01-01"), Some((false, true, false)));
        // Sunday 2027-01-03 ends that week; Monday 2027-01-04 starts 2027-W01.
        assert_eq!(flags("2027-01-01", "2027-01-03"), Some((false, false, false)));
        assert_eq!(flags("2027-01-03", "2027-01-04"), Some((true, false, false)));
    }

    #[test]
    fn an_invalid_remembered_date_counts_as_none() {
        for invalid in [
            "",
            "yesterday",
            "2026-13-01",
            "2026-02-30",
            "2026-1-5",
            "2026/11/09",
            " 2026-11-09",
            "2026-11-09T00:00:00Z",
            "+026-11-09",
        ] {
            assert_eq!(flags(invalid, "2026-11-09"), Some((true, true, true)), "{invalid:?}");
        }
    }

    #[test]
    fn a_clock_moved_back_still_pings() {
        // Remembered Tuesday 2026-11-10, today Monday 2026-11-09: same week and month.
        assert_eq!(flags("2026-11-10", "2026-11-09"), Some((false, false, false)));
        // Remembered 2026-12-01, today 2026-11-09: another week and month.
        assert_eq!(flags("2026-12-01", "2026-11-09"), Some((true, true, false)));
    }

    #[test]
    fn remembers_dates_as_it_reads_them() {
        assert_eq!(date_text(day("2026-01-05")), "2026-01-05");
        assert_eq!(due(Some(&date_text(day("2026-01-05"))), day("2026-01-05")), None);
    }

    #[test]
    fn the_request_is_exactly_the_documented_one() {
        let none = Flags {
            weekly: false,
            monthly: false,
            new: false,
        };
        assert_eq!(
            url_with(PING_URL, "0.1.0", "mac", none),
            "https://baristatime.app/api/ping?v=0.1.0&os=mac&d=1&w=0&m=0&n=0"
        );
        let all = Flags {
            weekly: true,
            monthly: true,
            new: true,
        };
        assert_eq!(
            url_with("http://127.0.0.1:8787/api/ping", "1.2.3", "windows", all),
            "http://127.0.0.1:8787/api/ping?v=1.2.3&os=windows&d=1&w=1&m=1&n=1"
        );
        let weekly = Flags { weekly: true, ..none };
        assert!(url_with(PING_URL, "0.1.0", "linux", weekly).ends_with("?v=0.1.0&os=linux&d=1&w=1&m=0&n=0"));
        // This build's own request: its version and platform, whatever the base.
        assert!(url(none).ends_with(&format!("?v={VERSION}&os={OS}&d=1&w=0&m=0&n=0")));
        assert!(["mac", "windows", "linux"].contains(&OS));
    }

    /// A stand-in for baristatime.app on this computer: answers one request with `reply` and hands
    /// back what it was sent.
    fn stand_in(reply: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/api/ping", listener.local_addr().unwrap());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut request = Vec::new();
            let mut buffer = [0u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => request.extend_from_slice(&buffer[..count]),
                }
            }
            let _ = stream.write_all(reply.as_bytes());
            let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
        });
        (base, receiver)
    }

    /// The tests below run the system's `curl`, as the app does, against a server on this computer
    /// only. Where there is no `curl` to run (the app then never pings), they have nothing to check.
    fn has_curl() -> bool {
        let found = crate::update::curl()
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok();
        if !found {
            eprintln!("skipped: no curl on this computer");
        }
        found
    }

    const NEW_WEEK: Flags = Flags {
        weekly: true,
        monthly: false,
        new: false,
    };

    #[test]
    fn sends_only_what_is_documented() {
        if !has_curl() {
            return;
        }
        let (base, received) = stand_in("HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
        assert!(send_to(&url_with(&base, VERSION, OS, NEW_WEEK)));
        let request = received.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        let mut lines = request.lines();
        assert_eq!(
            lines.next().unwrap(),
            format!("GET /api/ping?v={VERSION}&os={OS}&d=1&w=1&m=0&n=0 HTTP/1.1")
        );
        let mut headers: Vec<(String, String)> = lines
            .take_while(|line| !line.is_empty())
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                (name.to_lowercase(), value.trim().to_string())
            })
            .collect();
        headers.sort();
        // No cookie, no ID, nothing else: only the Host every request needs.
        let host = base.trim_start_matches("http://").trim_end_matches("/api/ping").to_string();
        assert_eq!(
            headers,
            [("host".to_string(), host), ("user-agent".to_string(), format!("Barista/{VERSION}")),]
        );
        // And no body after the headers.
        assert!(request.ends_with("\r\n\r\n"), "{request:?}");
    }

    #[test]
    fn a_refusal_or_no_reply_is_not_a_ping() {
        if !has_curl() {
            return;
        }
        let (base, _received) = stand_in("HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(!send_to(&url_with(&base, VERSION, OS, NEW_WEEK)));
        let (base, _received) = stand_in("HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(!send_to(&url_with(&base, VERSION, OS, NEW_WEEK)));
        // Nothing is listening here any more.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/api/ping", listener.local_addr().unwrap())
        };
        assert!(!send_to(&url_with(&closed, VERSION, OS, NEW_WEEK)));
    }

    #[test]
    fn redirects_are_not_followed() {
        if !has_curl() {
            return;
        }
        let elsewhere = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        elsewhere.set_nonblocking(true).unwrap();
        let reply: &'static str = Box::leak(
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{}/api/ping\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                elsewhere.local_addr().unwrap()
            )
            .into_boxed_str(),
        );
        let (base, received) = stand_in(reply);
        assert!(!send_to(&url_with(&base, VERSION, OS, NEW_WEEK)));
        received.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        assert!(elsewhere.accept().is_err(), "the redirect was followed");
    }

    #[test]
    fn only_a_2xx_reply_counts() {
        for status in ["200", "204", "299"] {
            assert!(is_success(status), "{status}");
        }
        for status in ["000", "301", "400", "404", "500", "", "2", "20x", "2040"] {
            assert!(!is_success(status), "{status}");
        }
    }
}
