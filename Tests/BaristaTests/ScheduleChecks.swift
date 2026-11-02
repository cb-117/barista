import BaristaCore
import Foundation

/// Scheduled hours by wall-clock time: weekdays, overnight windows, back-to-back windows, and DST.
func scheduleChecks() {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(identifier: "America/Los_Angeles")!
    calendar.locale = Locale(identifier: "en_US")
    func at(_ day: Int, _ hour: Int, _ minute: Int = 0, month: Int = 11) -> Date {
        calendar.date(from: DateComponents(year: 2026, month: month, day: day, hour: hour, minute: minute))!
    }
    // Nov 2, 2026 is a Monday; Nov 6 a Friday; Nov 7 a Saturday.
    var schedule = AwakeSchedule(enabled: true, rules: [.workweek])
    precondition(schedule.activeUntil(at(2, 8), calendar: calendar) == at(2, 17), "Starts at 8:00 on Monday")
    precondition(schedule.activeUntil(at(2, 12, 30), calendar: calendar) == at(2, 17), "Midday Monday")
    precondition(schedule.activeUntil(at(2, 7, 59), calendar: calendar) == nil, "Not before 8:00")
    precondition(schedule.activeUntil(at(2, 17), calendar: calendar) == nil, "Ends at 17:00")
    precondition(schedule.activeUntil(at(7, 12), calendar: calendar) == nil, "Not on Saturday")
    precondition(schedule.nextStart(after: at(6, 18), calendar: calendar) == at(9, 8), "Friday evening: next is Monday 8:00")
    precondition(schedule.nextStart(after: at(2, 9), calendar: calendar) == at(3, 8), "During Monday: next is Tuesday 8:00")

    schedule.enabled = false
    precondition(schedule.activeUntil(at(2, 12), calendar: calendar) == nil, "Off means never")
    precondition(schedule.nextStart(after: at(2, 12), calendar: calendar) == nil, "Off has no next start")

    // Friday 22:00 to Saturday 6:00 runs past midnight.
    schedule = AwakeSchedule(enabled: true, rules: [ScheduleRule(days: [6], start: 22 * 60, end: 6 * 60)])
    precondition(schedule.activeUntil(at(6, 23), calendar: calendar) == at(7, 6), "Overnight, before midnight")
    precondition(schedule.activeUntil(at(7, 3), calendar: calendar) == at(7, 6), "Overnight, after midnight")
    precondition(schedule.activeUntil(at(5, 23), calendar: calendar) == nil, "Thursday night isn't scheduled")

    // Back-to-back windows run as one, without a gap at noon.
    schedule = AwakeSchedule(enabled: true, rules: [ScheduleRule(days: [2], start: 8 * 60, end: 12 * 60),
                                                    ScheduleRule(days: [2], start: 12 * 60, end: 17 * 60)])
    precondition(schedule.activeUntil(at(2, 9), calendar: calendar) == at(2, 17), "Back-to-back windows merge")

    // Every day, midnight to midnight: always on, through the end of the week ahead.
    schedule = AwakeSchedule(enabled: true, rules: [ScheduleRule(days: Set(1...7), start: 0, end: 0)])
    precondition(schedule.activeUntil(at(2, 9), calendar: calendar) == .distantFuture, "Always on never ends")
    // Every day 0:00–23:59 leaves a minute's gap each night, so it does end.
    schedule = AwakeSchedule(enabled: true, rules: [ScheduleRule(days: Set(1...7), start: 0, end: 23 * 60 + 59)])
    precondition(schedule.activeUntil(at(2, 9), calendar: calendar) == at(2, 23, 59), "Nearly always on")

    // The week daylight saving ends is 169 hours: a one-minute gap just past 168 hours still counts.
    schedule = AwakeSchedule(enabled: true, rules: [ScheduleRule(days: [1, 2, 3, 4, 5, 6], start: 0, end: 0),
                                                    ScheduleRule(days: [7], start: 0, end: 23 * 60 + 59)])
    precondition(schedule.activeUntil(at(1, 0), calendar: calendar) == at(7, 23, 59), "Fall-back week keeps its gap")

    // Daylight saving ends Nov 1, 2026 (a 25-hour Sunday): 8:00–17:00 still means wall-clock 8 to 5.
    schedule = AwakeSchedule(enabled: true, rules: [ScheduleRule(days: [1], start: 8 * 60, end: 17 * 60)])
    precondition(schedule.activeUntil(at(1, 9), calendar: calendar) == at(1, 17), "Wall-clock hours across DST")
    // On the spring-forward day (Mar 8, 2026) a 2:30 start that doesn't exist begins at 3:00.
    schedule = AwakeSchedule(enabled: true, rules: [ScheduleRule(days: [1], start: 2 * 60 + 30, end: 5 * 60)])
    precondition(schedule.activeUntil(at(8, 3, 15, month: 3), calendar: calendar) == at(8, 5, month: 3), "Skipped start time")

    // Descriptions.
    let us = calendar
    precondition(ScheduleRule.workweek.daysDescription(calendar: us) == "Mon–Fri", "\(ScheduleRule.workweek.daysDescription(calendar: us))")
    precondition(ScheduleRule(days: [7, 1], start: 0, end: 60).daysDescription(calendar: us) == "Sat, Sun", "Weekend wraps")
    precondition(ScheduleRule(days: [2, 4, 6], start: 0, end: 60).daysDescription(calendar: us) == "Mon, Wed, Fri", "Separate days")
    precondition(ScheduleRule(days: Set(1...7), start: 0, end: 60).daysDescription(calendar: us) == "Every day", "Every day")
    let text = ScheduleRule.workweek.description(calendar: us, locale: Locale(identifier: "en_US"))
    precondition(text.hasPrefix("Mon–Fri, 8:00") && text.contains("5:00") && !text.contains("next day"), "Description: \(text)")
    precondition(ScheduleRule(days: [6], start: 22 * 60, end: 6 * 60).description(calendar: us, locale: Locale(identifier: "en_US")).hasSuffix("next day"), "Overnight description")

    // Saved schedules decode as written.
    let data = try! JSONEncoder().encode(AwakeSchedule(enabled: true))
    precondition(try! JSONDecoder().decode(AwakeSchedule.self, from: data) == AwakeSchedule(enabled: true), "Round trip")
    print("Passed the schedule check: weekdays, overnight and back-to-back hours, daylight saving, and descriptions.")
}
