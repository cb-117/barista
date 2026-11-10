@testable import BaristaCore
import Foundation

/// The active-user ping's flags and request (docs/ACTIVE-USERS.md). The Rust tests cover the same cases.
func activePingChecks() {
    typealias Flags = ActivePing.Flags
    func flags(_ last: String?, _ today: String) -> Flags? { ActivePing.flags(last: last, today: today) }
    let first = Flags(weekly: true, monthly: true, new: true)
    precondition(flags(nil, "2026-11-10") == first, "A first ping is new, and first of its week and month")
    precondition(flags("2026-11-10", "2026-11-10") == nil, "One ping a day")
    precondition(flags("2026-11-10", "2026-11-11") == Flags(weekly: false, monthly: false, new: false), "The next day, same week and month")
    // Sunday to Monday is a new ISO week inside the same month.
    precondition(flags("2026-11-15", "2026-11-16") == Flags(weekly: true, monthly: false, new: false), "A new week")
    // Monday, November 30 to Tuesday, December 1: a new month inside the same week.
    precondition(flags("2026-11-30", "2026-12-01") == Flags(weekly: false, monthly: true, new: false), "A new month")
    // December 31, 2026 (Thursday) and January 1, 2027 (Friday) share an ISO week; Sunday the 3rd to Monday the 4th don't.
    precondition(flags("2026-12-31", "2027-01-01") == Flags(weekly: false, monthly: true, new: false), "The ISO week spans the new year")
    precondition(flags("2027-01-03", "2027-01-04") == Flags(weekly: true, monthly: false, new: false), "The first ISO week boundary of 2027")
    // A week apart, a year apart, and a clock that moved back all still ping.
    precondition(flags("2026-11-03", "2026-11-10") == Flags(weekly: true, monthly: false, new: false), "A week later")
    precondition(flags("2025-11-10", "2026-11-10") == Flags(weekly: true, monthly: true, new: false), "A year later")
    precondition(flags("2026-11-12", "2026-11-10") == Flags(weekly: false, monthly: false, new: false), "A date after today still pings")
    for invalid in ["", "yesterday", "2026-13-40", "2026-2-3", "20261110"] {
        precondition(flags(invalid, "2026-11-10") == first, "An invalid remembered date counts as none: \(invalid)")
    }
    precondition(ActivePing.day(Date(timeIntervalSince1970: 1_794_355_199)) == "2026-11-10"
                 && ActivePing.day(Date(timeIntervalSince1970: 1_794_355_200)) == "2026-11-11", "Days are UTC")
    let url = ActivePing.url(Flags(weekly: true, monthly: false, new: false), version: "0.1.0")?.absoluteString
    if ProcessInfo.processInfo.environment["BARISTA_PING_URL"] == nil {
        precondition(url == "https://baristatime.app/api/ping?v=0.1.0&os=mac&d=1&w=1&m=0&n=0", "The request: \(url ?? "nil")")
    }
    print("Passed the active-user ping check: one ping a day, first of the week, month, and ever, and the request.")
}
