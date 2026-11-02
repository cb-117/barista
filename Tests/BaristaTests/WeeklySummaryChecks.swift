@testable import BaristaCore
import Foundation

/// The same cases as the Rust test `last_week_and_the_last_seven_days_come_from_all_time`.
func weeklySummaryChecks() {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: "America/Los_Angeles")!
    func project(_ days: [(String, Int, Int, Double)]) -> ProjectUsage {
        ProjectUsage(project: "/p", name: "p", totals: UsageTotals(), agents: [],
                     days: days.map { ProjectDay(date: $0.0, activeSeconds: $0.1, totalTokens: $0.2, estimatedCost: $0.3) }, modelOrder: [])
    }
    var all = PeriodUsage(period: .all, start: nil, agents: [])
    all.projects = [
        project([("2026-10-26", 3600, 10, 1.25), ("2026-11-01", 1800, 5, 2.5), ("2026-11-02", 600, 1, 9)]),
        project([("2026-10-30", 0, 7, 0.25)]),
        project([("2026-10-20", 900, 1, 0.5)]),
    ]
    let report = UsageReport(generatedAt: Date(), periods: [all], limits: [])
    // Monday, Nov 2, 2026, noon: last week is Oct 26 to Nov 1.
    let monday = calendar.date(from: DateComponents(year: 2026, month: 11, day: 2, hour: 12))!
    guard let summary = report.lastWeekSummary(now: monday, calendar: calendar) else { preconditionFailure("No weekly summary") }
    precondition(summary.activeSeconds == 5400 && summary.projects == 2, "Last week's time and projects")
    precondition(abs(summary.estimatedCost - 4) < 1e-9, "Last week's cost")
    precondition(summary.text == "Last week: 1h 30m across 2 projects, about $4 at API prices.", "Summary text: \(summary.text)")
    let days = report.dailyActiveSeconds(days: 7, now: monday, calendar: calendar) ?? []
    precondition(days.map(\.seconds) == [0, 0, 0, 0, 0, 1800, 600], "Last seven days: \(days.map(\.seconds))")
    let later = calendar.date(from: DateComponents(year: 2026, month: 11, day: 20, hour: 12))!
    precondition(report.lastWeekSummary(now: later, calendar: calendar) == nil, "No summary for a quiet week")
    print("Passed the weekly summary check: last week's time, projects, cost, and the last seven days.")
}
