import Foundation

/// Plan limits for each agent's subscription, as the agents themselves report them.
/// docs/USAGE.md ("Plan limits") is the specification; the Rust build implements the same rules.
public struct LimitWindow: Sendable, Equatable {
    public let id: String
    public let label: String
    public let windowMinutes: Int?
    public let usedPercent: Double
    public let resetsAt: Date?
    /// The window's reset time has passed since it was observed, so nothing is used yet.
    public let reset: Bool
    public var usedUSD: Double?
    public var limitUSD: Double?
    public var period: String?
}

public struct PlanCredits: Sendable, Equatable {
    public let balance: String?
    public let unlimited: Bool
}

public struct PlanLimits: Sendable, Equatable {
    public let agent: UsageAgent
    public let name: String
    public let source: String?
    public let observedAt: Date?
    public let plan: String?
    public let credits: PlanCredits?
    public let windows: [LimitWindow]
    public var available: Bool { observedAt != nil }

    /// "5-hour", "Weekly", "N-hour" (whole hours under a day), or "N-day".
    public static func label(minutes: Int?) -> String {
        guard let minutes, minutes > 0 else { return "Limit" }
        switch minutes {
        case 300: return "5-hour"
        case 10080: return "Weekly"
        case ..<1440: return "\(max(1, Int((Double(minutes) / 60).rounded())))-hour"
        default: return "\(max(1, Int((Double(minutes) / 1440).rounded())))-day"
        }
    }

    /// A window whose reset time has passed reports 0% used.
    public static func window(id: String, minutes: Int?, usedPercent: Double, resetsAt: Date?, now: Date, label: String? = nil) -> LimitWindow {
        let reset = resetsAt.map { $0 <= now } ?? false
        return LimitWindow(id: id, label: label ?? Self.label(minutes: minutes), windowMinutes: minutes,
                           usedPercent: reset ? 0 : usedPercent, resetsAt: resetsAt, reset: reset)
    }

    static func unavailable(_ agent: UsageAgent, name: String) -> PlanLimits {
        PlanLimits(agent: agent, name: name, source: nil, observedAt: nil, plan: nil, credits: nil, windows: [])
    }
}

/// The latest Codex `rate_limits`, kept per log file in the usage cache.
struct CodexLimitState: Codable, Equatable {
    struct Window: Codable, Equatable {
        var usedPercent: Double
        var windowMinutes: Int?
        var resetsAt: Double?
    }
    var observedAt: Double
    var primary: Window?
    var secondary: Window?
    var plan: String?
    var balance: String?
    var unlimited: Bool?

    /// Parses a token_count event's `rate_limits`; nil unless it has a primary or secondary window.
    init?(_ value: Any?, at time: Date) {
        guard let limits = value as? [String: Any] else { return nil }
        func window(_ key: String) -> Window? {
            guard let raw = limits[key] as? [String: Any], let used = raw["used_percent"] as? NSNumber else { return nil }
            return Window(usedPercent: used.doubleValue, windowMinutes: (raw["window_minutes"] as? NSNumber)?.intValue,
                          resetsAt: (raw["resets_at"] as? NSNumber)?.doubleValue)
        }
        primary = window("primary")
        secondary = window("secondary")
        guard primary != nil || secondary != nil else { return nil }
        observedAt = time.timeIntervalSince1970
        plan = limits["plan_type"] as? String
        if let credits = limits["credits"] as? [String: Any] {
            balance = credits["balance"] as? String
            unlimited = credits["unlimited"] as? Bool ?? false
        }
    }

    func limits(now: Date) -> PlanLimits {
        let windows = [("primary", primary), ("secondary", secondary)].compactMap { id, raw in
            raw.map { PlanLimits.window(id: id, minutes: $0.windowMinutes, usedPercent: $0.usedPercent,
                                        resetsAt: $0.resetsAt.map(Date.init(timeIntervalSince1970:)), now: now) }
        }
        let credits = unlimited.map { PlanCredits(balance: balance, unlimited: $0) }
        return PlanLimits(agent: .codex, name: "ChatGPT plan", source: "Codex", observedAt: Date(timeIntervalSince1970: observedAt),
                          plan: plan, credits: credits, windows: windows)
    }
}

public enum ClaudeLimits {
    /// Written by `barista statusline` from Claude Code's `rate_limits`.
    static var statusLineFile: URL { UsageLocations.dataDirectory.appendingPathComponent("claude-limits.json") }
    /// The Claude desktop app's own snapshots (undocumented; ignored when unreadable).
    static var appFile: URL {
        if let override = ProcessInfo.processInfo.environment["BARISTA_CLAUDE_APP_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: override).appendingPathComponent("plan-usage-history.json")
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/Claude/plan-usage-history.json")
    }

    /// Saves Claude Code's `rate_limits` (when it sent them) for the Usage & Limits view.
    public static func record(_ rateLimits: [String: Any], now: Date = Date()) {
        let object: [String: Any] = ["observedAt": UsageStore.formatDate(now), "rateLimits": rateLimits]
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return }
        try? FileManager.default.createDirectory(at: UsageLocations.dataDirectory, withIntermediateDirectories: true)
        try? data.write(to: statusLineFile, options: .atomic)
    }

    static func current(now: Date) -> PlanLimits {
        let candidates = [fromStatusLine(now: now), fromApp(now: now)].compactMap { $0 }
        return candidates.max { ($0.observedAt ?? .distantPast) < ($1.observedAt ?? .distantPast) }
            ?? .unavailable(.claude, name: "Claude plan")
    }

    private static func fromStatusLine(now: Date) -> PlanLimits? {
        guard let data = try? Data(contentsOf: statusLineFile),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let observed = (object["observedAt"] as? String).flatMap(UsageStore.parseDate),
              let limits = object["rateLimits"] as? [String: Any] else { return nil }
        var windows: [LimitWindow] = []
        for (id, minutes) in [("five_hour", 300), ("seven_day", 10080)] {
            guard let raw = limits[id] as? [String: Any], let used = raw["used_percentage"] as? NSNumber else { continue }
            windows.append(PlanLimits.window(id: id, minutes: minutes, usedPercent: used.doubleValue,
                                             resetsAt: (raw["resets_at"] as? NSNumber).map { Date(timeIntervalSince1970: $0.doubleValue) }, now: now))
        }
        if let raw = limits["spend_limit"] as? [String: Any], let used = raw["used_percentage"] as? NSNumber {
            var spend = PlanLimits.window(id: "spend_limit", minutes: nil, usedPercent: used.doubleValue,
                                          resetsAt: (raw["resets_at"] as? NSNumber).map { Date(timeIntervalSince1970: $0.doubleValue) },
                                          now: now, label: "Spend limit")
            spend.usedUSD = (raw["used_usd"] as? NSNumber)?.doubleValue
            spend.limitUSD = (raw["limit_usd"] as? NSNumber)?.doubleValue
            spend.period = raw["period"] as? String
            windows.append(spend)
        }
        return PlanLimits(agent: .claude, name: "Claude plan", source: "Claude Code", observedAt: observed, plan: nil, credits: nil, windows: windows)
    }

    private static func fromApp(now: Date) -> PlanLimits? {
        guard let data = try? Data(contentsOf: appFile),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let samples = object["samples"] as? [[String: Any]] else { return nil }
        let latest = samples.compactMap { sample -> (Double, [String: Any])? in
            guard let t = (sample["t"] as? NSNumber)?.doubleValue, let u = sample["u"] as? [String: Any] else { return nil }
            return (t, u)
        }.max { $0.0 < $1.0 }
        guard let (t, usage) = latest else { return nil }
        let windows = [("five_hour", "fh", 300), ("seven_day", "sd", 10080)].compactMap { id, key, minutes -> LimitWindow? in
            (usage[key] as? NSNumber).map { PlanLimits.window(id: id, minutes: minutes, usedPercent: $0.doubleValue, resetsAt: nil, now: now) }
        }
        guard !windows.isEmpty else { return nil }
        return PlanLimits(agent: .claude, name: "Claude plan", source: "Claude app",
                          observedAt: Date(timeIntervalSince1970: t / 1000), plan: nil, credits: nil, windows: windows)
    }
}
