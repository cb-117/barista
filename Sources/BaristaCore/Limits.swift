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
        let candidates = [ClaudeAccount.current(now: now), fromStatusLine(now: now), fromApp(now: now)].compactMap { $0 }
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
        // The app writes this file rarely; an old snapshot is likely from a past window, so it's ignored.
        guard let (t, usage) = latest, now.timeIntervalSince1970 - t / 1000 <= 3600 else { return nil }
        let windows = [("five_hour", "fh", 300), ("seven_day", "sd", 10080)].compactMap { id, key, minutes -> LimitWindow? in
            (usage[key] as? NSNumber).map { PlanLimits.window(id: id, minutes: minutes, usedPercent: $0.doubleValue, resetsAt: nil, now: now) }
        }
        guard !windows.isEmpty else { return nil }
        return PlanLimits(agent: .claude, name: "Claude plan", source: "Claude app",
                          observedAt: Date(timeIntervalSince1970: t / 1000), plan: nil, credits: nil, windows: windows)
    }
}

/// Claude plan limits from the user's Claude account, as the Claude app's Settings → Usage shows them.
/// The Barista app fetches in the background (at most every 5 minutes); `barista limits refresh` is the only
/// other fetch. Uses Claude Code's saved login read-only: never refreshed, written, or logged.
public enum ClaudeAccount {
    static var resultFile: URL { UsageLocations.dataDirectory.appendingPathComponent("claude-account-limits.json") }
    static let defaultEndpoint = URL(string: "https://api.anthropic.com/api/oauth/usage")!
    /// Results older than this are not shown.
    static let maxAge: TimeInterval = 3600

    /// The test credentials file, if one is configured.
    private static var testCredentials: String? {
        guard let path = ProcessInfo.processInfo.environment["BARISTA_CLAUDE_CREDENTIALS"], !path.isEmpty else { return nil }
        return path
    }
    /// The endpoint override applies only with test credentials, so nothing can redirect a real login.
    static var endpoint: URL {
        guard testCredentials != nil, let override = ProcessInfo.processInfo.environment["BARISTA_CLAUDE_USAGE_URL"],
              !override.isEmpty, let url = URL(string: override) else { return defaultEndpoint }
        return url
    }

    public enum Outcome: Sendable, Equatable {
        case updated, noLogin, loginExpired
        /// macOS needs your permission to read Claude Code's login (background reads never ask).
        case needsPermission
        /// You denied or cancelled the Keychain prompt.
        case denied
        case unreachable(String)
        case httpStatus(Int)
        public var ok: Bool { self == .updated }
        public var message: String {
            switch self {
            case .updated: "Claude account limits updated."
            case .noLogin: "No Claude Code login found."
            case .loginExpired: "Claude Code\u{2019}s login has expired. Open Claude Code to renew it."
            case .needsPermission: "Barista needs your permission to read Claude Code\u{2019}s login. Click Allow access in Usage & Limits."
            case .denied: "Access to Claude Code\u{2019}s login was denied. Click Allow access in Usage & Limits to try again."
            case .unreachable(let detail): "Couldn\u{2019}t reach Anthropic (\(detail))."
            case .httpStatus(let status): "Anthropic replied \(status)."
            }
        }
    }

    enum Credential: Equatable {
        case token(String, expires: Date)
        case noLogin, expired, needsPermission, denied
    }

    /// The token stays in memory until it expires, so the Keychain is read rarely. Never written anywhere.
    private final class TokenCache: @unchecked Sendable {
        private let lock = NSLock()
        private var value: (token: String, expires: Date)?
        func get(now: Date) -> String? {
            lock.lock(); defer { lock.unlock() }
            guard let value, value.expires.timeIntervalSince(now) > 60 else { return nil }
            return value.token
        }
        func set(_ token: String, expires: Date) { lock.lock(); value = (token, expires); lock.unlock() }
        func clear() { lock.lock(); value = nil; lock.unlock() }
    }
    private static let cache = TokenCache()

    /// Reads Claude Code's login. `interactive` allows macOS's Keychain prompt; background reads never show it.
    static func credential(interactive: Bool, now: Date = Date()) -> Credential {
        if testCredentials == nil, let token = cache.get(now: now) {
            return .token(token, expires: now.addingTimeInterval(61))
        }
        let data: Data
        if let path = testCredentials {
            guard let file = try? Data(contentsOf: URL(fileURLWithPath: path)) else { return .noLogin }
            data = file
        } else {
            switch readKeychain(interactive: interactive) {
            case .success(let found): data = found
            case .failure(let failure): return failure.credential
            }
        }
        guard let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let oauth = object["claudeAiOauth"] as? [String: Any],
              let token = oauth["accessToken"] as? String, !token.isEmpty else { return .noLogin }
        guard let expiresMs = (oauth["expiresAt"] as? NSNumber)?.doubleValue else { return .expired }
        let expires = Date(timeIntervalSince1970: expiresMs / 1000)
        guard expires.timeIntervalSince(now) > 60 else { return .expired }
        if testCredentials == nil { cache.set(token, expires: expires) }
        return .token(token, expires: expires)
    }

    /// Reads the item with /usr/bin/security, the tool Claude Code itself uses to save and read it. The item's
    /// access list trusts that tool, so macOS never asks. Reading it directly (SecItemCopyMatching) made macOS
    /// ask for Barista, again after every update, since each build has a new ad-hoc signature.
    private static func readKeychain(interactive: Bool) -> Result<Data, CredentialFailure> {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/security")
        process.arguments = ["find-generic-password", "-s", "Claude Code-credentials", "-w"]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        process.standardInput = FileHandle.nullDevice
        do { try process.run() } catch { return .failure(CredentialFailure(.denied)) }
        // If macOS ever does ask (an item Claude Code didn't save), a background read gives up rather than wait.
        let timeout = DispatchWorkItem { process.terminate() }
        DispatchQueue.global().asyncAfter(deadline: .now() + (interactive ? 120 : 10), execute: timeout)
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        timeout.cancel()
        switch process.terminationStatus {
        case 0: return .success(data)
        case 44: return .failure(CredentialFailure(.noLogin)) // errSecItemNotFound
        default: return .failure(CredentialFailure(interactive ? .denied : .needsPermission))
        }
    }

    private struct CredentialFailure: Error {
        let credential: Credential
        init(_ credential: Credential) { self.credential = credential }
    }

    /// Refuses every redirect, so a redirect is a failure and never replaces the saved result.
    private final class NoRedirects: NSObject, URLSessionTaskDelegate, Sendable {
        func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                        newRequest request: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
            completionHandler(nil)
        }
    }

    /// Asks Anthropic for the account's current limits and saves them. Keeps the old result on any failure.
    public static func refresh(interactive: Bool = false) async -> Outcome {
        let token: String
        switch credential(interactive: interactive) {
        case .token(let value, _): token = value
        case .noLogin: return .noLogin
        case .expired: return .loginExpired
        case .needsPermission: return .needsPermission
        case .denied: return .denied
        }
        var request = URLRequest(url: endpoint, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 15)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("oauth-2025-04-20", forHTTPHeaderField: "anthropic-beta")
        request.setValue("Barista/\(BaristaVersion.current)", forHTTPHeaderField: "User-Agent")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCache = nil
        let session = URLSession(configuration: configuration, delegate: NoRedirects(), delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        let data: Data, response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            return .unreachable((error as? URLError).map { "\($0.code.rawValue)" } ?? "network error")
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else { return .httpStatus(status) }
        guard let usage = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return .httpStatus(status) }
        do {
            let object: [String: Any] = ["observedAt": UsageStore.formatDate(Date()), "usage": usage]
            let saved = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
            try FileManager.default.createDirectory(at: UsageLocations.dataDirectory, withIntermediateDirectories: true)
            try saved.write(to: resultFile, options: .atomic)
        } catch {
            return .unreachable("couldn\u{2019}t save the result")
        }
        return .updated
    }

    /// Turning the setting off: forget the saved result and the in-memory token.
    public static func forget() {
        cache.clear()
        try? FileManager.default.removeItem(at: resultFile)
    }

    static func current(now: Date) -> PlanLimits? {
        guard let data = try? Data(contentsOf: resultFile),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let observed = (object["observedAt"] as? String).flatMap(UsageStore.parseDate),
              let usage = object["usage"] as? [String: Any],
              now.timeIntervalSince(observed) <= maxAge else { return nil }
        return PlanLimits(agent: .claude, name: "Claude plan", source: "Claude account", observedAt: observed,
                          plan: nil, credits: nil, windows: windows(usage, now: now))
    }

    static func windows(_ usage: [String: Any], now: Date) -> [LimitWindow] {
        let date = { (value: Any?) in (value as? String).flatMap(UsageStore.parseDate) }
        var windows: [LimitWindow] = []
        if let limits = usage["limits"] as? [[String: Any]] {
            for limit in limits {
                guard let kind = limit["kind"] as? String, let percent = (limit["percent"] as? NSNumber)?.doubleValue else { continue }
                let resets = date(limit["resets_at"])
                switch kind {
                case "session":
                    windows.append(PlanLimits.window(id: "five_hour", minutes: 300, usedPercent: percent, resetsAt: resets, now: now, label: "Session"))
                case "weekly_all":
                    windows.append(PlanLimits.window(id: "seven_day", minutes: 10080, usedPercent: percent, resetsAt: resets, now: now, label: "Weekly"))
                case "weekly_scoped":
                    let scope = limit["scope"] as? [String: Any]
                    let name = ((scope?["model"] as? [String: Any])?["display_name"] as? String) ?? (scope?["surface"] as? String) ?? "Scoped"
                    let id = "seven_day_" + name.lowercased().replacingOccurrences(of: " ", with: "_")
                    windows.append(PlanLimits.window(id: id, minutes: 10080, usedPercent: percent, resetsAt: resets, now: now, label: "Weekly · \(name)"))
                default:
                    windows.append(PlanLimits.window(id: kind, minutes: nil, usedPercent: percent, resetsAt: resets, now: now,
                                                     label: kind.replacingOccurrences(of: "_", with: " ")))
                }
            }
        } else {
            for (key, id, label, minutes) in [("five_hour", "five_hour", "Session", 300), ("seven_day", "seven_day", "Weekly", 10080)] {
                guard let raw = usage[key] as? [String: Any], let used = (raw["utilization"] as? NSNumber)?.doubleValue else { continue }
                windows.append(PlanLimits.window(id: id, minutes: minutes, usedPercent: used, resetsAt: date(raw["resets_at"]), now: now, label: label))
            }
        }
        if let spend = usage["spend"] as? [String: Any], spend["enabled"] as? Bool == true {
            let money = { (value: Any?) -> Double? in
                guard let raw = value as? [String: Any], let amount = (raw["amount_minor"] as? NSNumber)?.doubleValue else { return nil }
                return amount / pow(10, (raw["exponent"] as? NSNumber)?.doubleValue ?? 2)
            }
            var extra = PlanLimits.window(id: "extra_usage", minutes: nil, usedPercent: (spend["percent"] as? NSNumber)?.doubleValue ?? 0,
                                          resetsAt: nil, now: now, label: "Extra usage")
            extra.usedUSD = money(spend["used"])
            extra.limitUSD = money(spend["limit"])
            windows.append(extra)
        }
        return windows
    }
}
