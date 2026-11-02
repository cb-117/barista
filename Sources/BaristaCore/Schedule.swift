import Foundation

/// A repeating stretch of time, such as Monday to Friday, 8:00 to 17:00, in this computer's clock and time zone.
public struct ScheduleRule: Codable, Identifiable, Sendable, Equatable, Hashable {
    public var id: String
    /// Calendar weekdays the window starts on: 1 = Sunday … 7 = Saturday.
    public var days: Set<Int>
    /// Minutes after midnight, 0..<1440.
    public var start: Int
    /// Minutes after midnight, 0..<1440. At or before `start`, the window runs past midnight into the
    /// next day (22:00–06:00); equal to `start`, it lasts a whole day.
    public var end: Int

    public init(id: String = UUID().uuidString.lowercased(), days: Set<Int>, start: Int, end: Int) {
        self.id = id
        self.days = days.filter { (1...7).contains($0) }
        self.start = min(1439, max(0, start))
        self.end = min(1439, max(0, end))
    }

    /// Monday to Friday, 8:00 AM to 5:00 PM.
    public static let workweek = ScheduleRule(days: [2, 3, 4, 5, 6], start: 8 * 60, end: 17 * 60)

    public var crossesMidnight: Bool { end <= start }

    /// The windows this rule opens on each day from `from` through `through` (both start-of-day dates).
    func intervals(from: Date, through: Date, calendar: Calendar) -> [DateInterval] {
        var result: [DateInterval] = []
        var day = calendar.startOfDay(for: from)
        while day <= through {
            if days.contains(calendar.component(.weekday, from: day)),
               let opens = Self.time(start, on: day, calendar: calendar),
               let closingDay = crossesMidnight ? calendar.date(byAdding: .day, value: 1, to: day) : day,
               let closes = Self.time(end, on: closingDay, calendar: calendar), closes > opens {
                result.append(DateInterval(start: opens, end: closes))
            }
            guard let next = calendar.date(byAdding: .day, value: 1, to: day) else { break }
            day = next
        }
        return result
    }

    /// A wall-clock time on a day. On a daylight-saving change that skips it, the next time that exists.
    static func time(_ minutes: Int, on day: Date, calendar: Calendar) -> Date? {
        calendar.date(bySettingHour: minutes / 60, minute: minutes % 60, second: 0, of: day, matchingPolicy: .nextTime,
                      repeatedTimePolicy: .first, direction: .forward)
    }
}

/// When Barista keeps the computer awake on its own: the hours in `rules`, by the system clock.
public struct AwakeSchedule: Codable, Sendable, Equatable {
    public var enabled: Bool
    public var rules: [ScheduleRule]
    public var keepDisplayAwake: Bool

    public init(enabled: Bool = false, rules: [ScheduleRule] = [.workweek], keepDisplayAwake: Bool = false) {
        self.enabled = enabled
        self.rules = rules
        self.keepDisplayAwake = keepDisplayAwake
    }

    /// The longest a scheduled session runs before it's renewed; sessions are capped at 24 hours.
    public static let maxLease: TimeInterval = 86400

    private func intervals(around now: Date, calendar: Calendar) -> [DateInterval] {
        guard enabled, let from = calendar.date(byAdding: .day, value: -1, to: now),
              let through = calendar.date(byAdding: .day, value: 8, to: now) else { return [] }
        return rules.filter { !$0.days.isEmpty }
            .flatMap { $0.intervals(from: from, through: through, calendar: calendar) }
            .sorted { $0.start < $1.start }
    }

    /// If `now` is in scheduled hours, when they end. Back-to-back and overlapping windows count as one,
    /// so Monday 8–12 and 12–17 run until 17:00 without a gap. Nil outside scheduled hours or when off;
    /// `.distantFuture` when the hours never stop (every day, all day).
    public func activeUntil(_ now: Date, calendar: Calendar = .current) -> Date? {
        let windows = intervals(around: now, calendar: calendar)
        guard var end = windows.filter({ $0.start <= now && now < $0.end }).map(\.end).max() else { return nil }
        var extended = true
        while extended {
            extended = false
            for window in windows where window.start <= end && window.end > end {
                end = window.end
                extended = true
            }
        }
        // Covered for a whole week without a break: a weekly schedule like that never ends. A calendar
        // week, not 168 hours: the week daylight saving ends has 169, and a gap can fall in the extra hour.
        guard let weekLater = calendar.date(byAdding: .day, value: 7, to: now) else { return end }
        return end > weekLater ? .distantFuture : end
    }

    /// When scheduled hours next begin after `now`, within the coming week.
    public func nextStart(after now: Date, calendar: Calendar = .current) -> Date? {
        intervals(around: now, calendar: calendar).map(\.start).filter { $0 > now }.min()
    }
}

// MARK: Describing a schedule

extension ScheduleRule {
    /// "Mon–Fri", "Mon, Wed, Fri", "Every day", or "Sat, Sun", in the calendar's week order.
    public func daysDescription(calendar: Calendar = .current) -> String {
        if days.count == 7 { return "Every day" }
        if days.isEmpty { return "No days" }
        let symbols = calendar.shortWeekdaySymbols
        let order = (0..<7).map { (calendar.firstWeekday - 1 + $0) % 7 + 1 }
        var runs: [[Int]] = []
        for day in order where days.contains(day) {
            if let last = runs.last?.last, order.firstIndex(of: last)! + 1 == order.firstIndex(of: day)! {
                runs[runs.count - 1].append(day)
            } else {
                runs.append([day])
            }
        }
        // A weekend run that wraps around the week reads Sat, Sun rather than Sun, Sat.
        if runs.count > 1, runs[0].first == order[0], runs[runs.count - 1].last == order[6] {
            runs[0] = runs.removeLast() + runs[0]
        }
        return runs.map { run in
            let first = symbols[run[0] - 1], last = symbols[run[run.count - 1] - 1]
            switch run.count {
            case 1: return first
            case 2: return "\(first), \(last)"
            default: return "\(first)–\(last)"
            }
        }.joined(separator: ", ")
    }

    /// "Mon–Fri, 8:00 AM – 5:00 PM".
    public func description(calendar: Calendar = .current, locale: Locale = .current) -> String {
        "\(daysDescription(calendar: calendar)), \(Self.clock(start, locale: locale)) – \(Self.clock(end, locale: locale))"
            + (end == start ? " (24 hours)" : crossesMidnight ? " next day" : "")
    }

    static func clock(_ minutes: Int, locale: Locale) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let date = Date(timeIntervalSince1970: TimeInterval(minutes * 60))
        var style = Date.FormatStyle(date: .omitted, time: .shortened, locale: locale, calendar: calendar)
        style.timeZone = calendar.timeZone
        return date.formatted(style)
    }
}
