import Foundation

/// The once-a-day "a copy of Barista is running" ping that counts active users without identifying
/// anyone. docs/ACTIVE-USERS.md is the specification; the Rust build implements the same rules.
public enum ActivePing {
    public static let defaultURL = "https://baristatime.app/api/ping"

    /// Whether this is the copy's first ping of the ISO week, of the month, and ever. Worked out here,
    /// from the date of the last ping, so the server needs no ID to count weekly and monthly users.
    public struct Flags: Sendable, Equatable {
        public let weekly: Bool
        public let monthly: Bool
        public let new: Bool
    }

    private static var utc: Calendar {
        var calendar = Calendar(identifier: .iso8601)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }
    private static var dayFormat: DateFormatter {
        let format = DateFormatter()
        format.calendar = Calendar(identifier: .gregorian)
        format.locale = Locale(identifier: "en_US_POSIX")
        format.timeZone = TimeZone(identifier: "UTC")
        format.dateFormat = "yyyy-MM-dd"
        format.isLenient = false
        return format
    }

    /// A UTC date as "YYYY-MM-DD".
    public static func day(_ date: Date) -> String { dayFormat.string(from: date) }

    /// The flags for a ping today, or nil when today's ping was already sent. `last` and `today` are
    /// UTC dates as "YYYY-MM-DD"; a `last` that isn't one counts as never having pinged.
    public static func flags(last: String?, today: String) -> Flags? {
        let format = dayFormat
        let parse = { (text: String) -> Date? in format.date(from: text).flatMap { format.string(from: $0) == text ? $0 : nil } }
        guard let now = parse(today) else { return nil }
        guard let last, let before = parse(last) else { return Flags(weekly: true, monthly: true, new: true) }
        if last == today { return nil }
        let calendar = utc
        let week = { (date: Date) in calendar.dateComponents([.yearForWeekOfYear, .weekOfYear], from: date) }
        let month = { (date: Date) in calendar.dateComponents([.year, .month], from: date) }
        return Flags(weekly: week(before) != week(now), monthly: month(before) != month(now), new: false)
    }

    /// The request: version, platform, and the flags. Nothing else is sent.
    public static func url(_ flags: Flags, version: String = BaristaVersion.current) -> URL? {
        let override = ProcessInfo.processInfo.environment["BARISTA_PING_URL"].flatMap { $0.isEmpty ? nil : $0 }
        var components = URLComponents(string: override ?? defaultURL)
        let bit = { (value: Bool) in value ? "1" : "0" }
        components?.queryItems = [
            URLQueryItem(name: "v", value: version), URLQueryItem(name: "os", value: "mac"), URLQueryItem(name: "d", value: "1"),
            URLQueryItem(name: "w", value: bit(flags.weekly)), URLQueryItem(name: "m", value: bit(flags.monthly)),
            URLQueryItem(name: "n", value: bit(flags.new)),
        ]
        return components?.url
    }

    /// Sends the ping. True on any HTTP 2xx answer.
    public static func send(_ flags: Flags) async -> Bool {
        guard let url = url(flags) else { return false }
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 15)
        request.setValue("Barista/\(BaristaVersion.current)", forHTTPHeaderField: "User-Agent")
        // The system would otherwise add your preferred languages; the count has no use for them.
        request.setValue("en", forHTTPHeaderField: "Accept-Language")
        let session = AccountRequest.session()
        defer { session.finishTasksAndInvalidate() }
        guard let (_, response) = try? await session.data(for: request),
              let status = (response as? HTTPURLResponse)?.statusCode else { return false }
        return (200..<300).contains(status)
    }
}
