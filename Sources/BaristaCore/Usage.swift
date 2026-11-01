import Darwin
import Foundation

/// Token and session usage for Claude Code, Codex, and Cursor, computed from files on this Mac.
/// docs/USAGE.md is the specification; the Rust build implements the same rules.
public enum UsageAgent: String, CaseIterable, Sendable {
    case claude, codex, cursor
    public var displayName: String {
        switch self {
        case .claude: "Claude Code"
        case .codex: "Codex"
        case .cursor: "Cursor"
        }
    }
}

public enum UsagePeriod: String, CaseIterable, Sendable {
    case today, week, month, all
    public var label: String {
        switch self {
        case .today: "Today"
        case .week: "This week"
        case .month: "This month"
        case .all: "All time"
        }
    }
    /// Local midnight, Monday 00:00, the 1st at 00:00, or nil for all time.
    public func start(now: Date = Date(), calendar: Calendar = .current) -> Date? {
        var calendar = calendar
        calendar.firstWeekday = 2
        switch self {
        case .today: return calendar.startOfDay(for: now)
        case .week: return calendar.dateInterval(of: .weekOfYear, for: now)?.start
        case .month: return calendar.dateInterval(of: .month, for: now)?.start
        case .all: return nil
        }
    }
}

/// Totals for an agent or one of its models within a period.
public struct UsageTotals: Sendable, Equatable {
    public var model = ""
    public var sessions = 0
    public var requests = 0
    public var turns = 0
    public var activeSeconds = 0
    public var inputTokens = 0
    public var outputTokens = 0
    public var cacheReadTokens = 0
    public var cacheWriteTokens = 0
    public var unpricedTokens = 0
    /// nil when there are tokens but none are priced, and always for Cursor.
    public var estimatedCost: Double? = 0
    public var totalTokens: Int { inputTokens + outputTokens + cacheReadTokens + cacheWriteTokens }
}

public struct AgentUsage: Sendable, Equatable {
    public let agent: UsageAgent
    public var totals: UsageTotals
    public var models: [UsageTotals]
}

public struct PeriodUsage: Sendable, Equatable {
    public let period: UsagePeriod
    public let start: Date?
    public var agents: [AgentUsage]
    public var totalTokens: Int { agents.reduce(0) { $0 + $1.totals.totalTokens } }
    public var estimatedCost: Double { agents.reduce(0) { $0 + ($1.totals.estimatedCost ?? 0) } }
}

public struct UsageReport: Sendable, Equatable {
    public let generatedAt: Date
    public let periods: [PeriodUsage]
    /// Claude, ChatGPT (Codex), and Cursor plans, in that order.
    public let limits: [PlanLimits]
    public static let estimateNote = "Costs are estimates at API list prices (checked \(ModelPrices.checkedOn)), not what a subscription charges."
}

// MARK: - Locations

public enum UsageLocations {
    /// Barista's data folder: BARISTA_DATA_DIR, else ~/Library/Application Support/Barista.
    public static var dataDirectory: URL {
        if let override = ProcessInfo.processInfo.environment["BARISTA_DATA_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: override)
        }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Barista")
    }
    static var claudeProjects: [URL] {
        let home = FileManager.default.homeDirectoryForCurrentUser
        if let configured = ProcessInfo.processInfo.environment["CLAUDE_CONFIG_DIR"], !configured.isEmpty {
            return configured.split(separator: ",").map { URL(fileURLWithPath: String($0).trimmingCharacters(in: .whitespaces)).appendingPathComponent("projects") }
        }
        return [home.appendingPathComponent(".claude/projects"), home.appendingPathComponent(".config/claude/projects")]
    }
    static var codexHome: URL {
        if let configured = ProcessInfo.processInfo.environment["CODEX_HOME"], !configured.isEmpty { return URL(fileURLWithPath: configured) }
        return FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".codex")
    }
    static var cursorLog: URL { dataDirectory.appendingPathComponent("cursor-usage.jsonl") }
}

// MARK: - Prices

public struct ModelPrice: Decodable, Sendable {
    public let model: String
    public let input, output, cacheRead, cacheWrite5m, cacheWrite1h: Double
}

public enum UsagePricing {
    public static let table: [ModelPrice] = (try? JSONDecoder().decode([ModelPrice].self, from: Data(ModelPrices.json.utf8))) ?? []

    /// A log's model ID cut at the first `[` or `@`.
    public static func normalized(_ model: String) -> String {
        String(model.prefix { $0 != "[" && $0 != "@" })
    }
    /// The longest entry equal to the model or followed in it by "-".
    public static func price(for model: String) -> ModelPrice? {
        let id = normalized(model)
        return table.filter { id == $0.model || id.hasPrefix($0.model + "-") }.max { $0.model.count < $1.model.count }
    }
}

// MARK: - Cache and parsing

/// Per-hour token counts: input, output, cache read, 5-minute writes, 1-hour writes, requests.
typealias Bucket = [Int]

private struct FileState: Codable {
    var offset: Int = 0
    var agent: String
    /// Codex: the last counted total_token_usage (input, cached, output, reasoning, total).
    var previous: [Int]?
    var model: String?
    var session: String?
    /// Claude dedupe keys this file contributed.
    var keys: [String] = []
    /// "model\u{1f}session\u{1f}hour" → bucket.
    var buckets: [String: Bucket] = [:]
    /// Codex: the newest plan limits this file reported.
    var limits: CodexLimitState?
}

private struct CacheFile: Codable {
    var version = 3
    var files: [String: FileState] = [:]
}

public enum UsageStore {
    static let separator: Character = "\u{1f}"

    /// Reads new log lines into the cache, then reports the requested periods.
    public static func report(periods: [UsagePeriod], now: Date = Date(), calendar: Calendar = .current) -> UsageReport {
        let cache = refreshedCache()
        return build(cache: cache, cursor: readCursorLog(), periods: periods, now: now, calendar: calendar)
    }

    private static var cacheURL: URL { UsageLocations.dataDirectory.appendingPathComponent("usage-cache.json") }

    private static func refreshedCache() -> CacheFile {
        var cache = (try? Data(contentsOf: cacheURL)).flatMap { try? JSONDecoder().decode(CacheFile.self, from: $0) } ?? CacheFile()
        if cache.version != CacheFile().version { cache = CacheFile() }
        var logs: [(path: String, agent: UsageAgent)] = []
        for root in UsageLocations.claudeProjects { logs += jsonl(under: root).map { ($0, .claude) } }
        for folder in ["sessions", "archived_sessions"] {
            logs += jsonl(under: UsageLocations.codexHome.appendingPathComponent(folder)).map { ($0, .codex) }
        }
        logs.sort { Array($0.path.utf8).lexicographicallyPrecedes(Array($1.path.utf8)) }
        var seen = Set(cache.files.values.flatMap(\.keys))
        var changed = false
        var visited = Set<String>()
        for (path, agent) in logs {
            // Codex moves a conversation's log when it's archived; its file name stays, so the
            // cache entry follows it instead of counting it twice. Two copies: the first one wins.
            let key = cacheKey(path: path, agent: agent)
            guard visited.insert(key).inserted else { continue }
            guard let size = (try? FileManager.default.attributesOfItem(atPath: path)[.size] as? NSNumber)?.intValue else { continue }
            var state = cache.files[key] ?? FileState(agent: agent.rawValue)
            if size < state.offset {
                // Emptied or rewritten: forget what this file contributed and read it again.
                seen.subtract(state.keys)
                state = FileState(agent: agent.rawValue)
                cache.files[key] = state
                changed = true
            }
            guard size > state.offset else { continue }
            if read(path: path, agent: agent, state: &state, seen: &seen) {
                cache.files[key] = state
                changed = true
            }
        }
        if changed { save(cache) }
        return cache
    }

    /// Claude logs by path; Codex logs by file name, which survives archiving.
    private static func cacheKey(path: String, agent: UsageAgent) -> String {
        agent == .codex ? "codex/" + URL(fileURLWithPath: path).lastPathComponent : path
    }

    private static func save(_ cache: CacheFile) {
        guard let data = try? JSONEncoder().encode(cache) else { return }
        try? FileManager.default.createDirectory(at: UsageLocations.dataDirectory, withIntermediateDirectories: true)
        try? data.write(to: cacheURL, options: .atomic)
    }

    private static func jsonl(under root: URL) -> [String] {
        guard let enumerator = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isRegularFileKey]) else { return [] }
        var paths: [String] = []
        for case let url as URL in enumerator where url.pathExtension == "jsonl" {
            if (try? url.resourceValues(forKeys: [.isRegularFileKey]))?.isRegularFile == true { paths.append(url.path) }
        }
        return paths
    }

    /// Reads complete lines after `state.offset`. Returns false if nothing new could be read.
    private static func read(path: String, agent: UsageAgent, state: inout FileState, seen: inout Set<String>) -> Bool {
        guard let handle = FileHandle(forReadingAtPath: path) else { return false }
        defer { try? handle.close() }
        do { try handle.seek(toOffset: UInt64(state.offset)) } catch { return false }
        guard let data = try? handle.readToEnd(), let lastNewline = data.lastIndex(of: 10) else { return false }
        let complete = data[data.startIndex...lastNewline]
        let fallbackSession = URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
        for line in complete.split(separator: 10, omittingEmptySubsequences: true) {
            switch agent {
            case .claude: readClaude(Data(line), session: fallbackSession, state: &state, seen: &seen)
            case .codex: readCodex(Data(line), session: fallbackSession, state: &state)
            case .cursor: break
            }
        }
        state.offset += complete.count
        return true
    }

    private static func contains(_ line: Data, _ needle: String) -> Bool { line.range(of: Data(needle.utf8)) != nil }

    private static func readClaude(_ line: Data, session fallback: String, state: inout FileState, seen: inout Set<String>) {
        guard contains(line, "\"usage\""),
              let object = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any],
              object["type"] as? String == "assistant",
              let message = object["message"] as? [String: Any],
              let usage = message["usage"] as? [String: Any],
              let model = message["model"] as? String, model != "<synthetic>",
              let time = (object["timestamp"] as? String).flatMap(parseDate) else { return }
        if let id = message["id"] as? String {
            let key = id + ":" + (object["requestId"] as? String ?? "")
            guard seen.insert(key).inserted else { return }
            state.keys.append(key)
        }
        let number = { (key: String, from: [String: Any]) -> Int in (from[key] as? NSNumber)?.intValue ?? 0 }
        var write5m = number("cache_creation_input_tokens", usage), write1h = 0
        if let split = usage["cache_creation"] as? [String: Any] {
            write5m = number("ephemeral_5m_input_tokens", split)
            write1h = number("ephemeral_1h_input_tokens", split)
        }
        add([number("input_tokens", usage), number("output_tokens", usage), number("cache_read_input_tokens", usage), write5m, write1h, 1],
            model: model, session: object["sessionId"] as? String ?? fallback, time: time, to: &state)
    }

    private static func readCodex(_ line: Data, session fallback: String, state: inout FileState) {
        guard contains(line, "token_count") || contains(line, "turn_context") || contains(line, "session_meta"),
              let object = (try? JSONSerialization.jsonObject(with: line)) as? [String: Any],
              let payload = object["payload"] as? [String: Any] else { return }
        switch object["type"] as? String {
        case "session_meta":
            state.session = payload["id"] as? String ?? payload["session_id"] as? String
        case "turn_context":
            if let model = payload["model"] as? String { state.model = model }
        case "event_msg":
            guard payload["type"] as? String == "token_count",
                  let time = (object["timestamp"] as? String).flatMap(parseDate) else { return }
            // Limits count even when the token totals repeat.
            if let limits = CodexLimitState(payload["rate_limits"], at: time), limits.observedAt >= (state.limits?.observedAt ?? 0) {
                state.limits = limits
            }
            guard let total = (payload["info"] as? [String: Any])?["total_token_usage"] as? [String: Any] else { return }
            let values = ["input_tokens", "cached_input_tokens", "output_tokens", "reasoning_output_tokens", "total_tokens"]
                .map { (total[$0] as? NSNumber)?.intValue ?? 0 }
            var delta = values
            if let previous = state.previous {
                guard values[4] > previous[4] else { return }  // A repeated report.
                delta = zip(values, previous).map { $0 - $1 }
            }
            state.previous = values
            add([max(0, delta[0] - delta[1]), delta[2], delta[1], 0, 0, 1],
                model: state.model ?? "unknown", session: state.session ?? fallback, time: time, to: &state)
        default: break
        }
    }

    private static func add(_ counts: Bucket, model: String, session: String, time: Date, to state: inout FileState) {
        // Quarter-hour buckets line up with every time zone's midnight (some are offset by 30 or 45 minutes).
        let hour = Int(floor(time.timeIntervalSince1970 / 900)) * 900
        let key = "\(UsagePricing.normalized(model))\(separator)\(session)\(separator)\(hour)"
        let existing = state.buckets[key] ?? [0, 0, 0, 0, 0, 0]
        state.buckets[key] = zip(existing, counts).map { $0 + $1 }
    }

    // MARK: Cursor

    struct CursorRecord {
        let conversation, generation, event: String
        let model: String?
        let time: Date
    }

    /// Appends one line for a Cursor hook event. Never throws: tracking must not disturb Cursor.
    public static func recordCursor(event: String, conversation: String, generation: String, model: String?, status: String?) {
        var record: [String: Any] = ["conversation": conversation, "event": event, "generation": generation, "ts": formatDate(Date())]
        if let model, !model.isEmpty { record["model"] = model }
        if let status, !status.isEmpty { record["status"] = status }
        guard var data = try? JSONSerialization.data(withJSONObject: record, options: [.sortedKeys, .withoutEscapingSlashes]) else { return }
        data.append(10)
        try? FileManager.default.createDirectory(at: UsageLocations.dataDirectory, withIntermediateDirectories: true)
        let fd = open(UsageLocations.cursorLog.path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0o600)
        guard fd >= 0 else { return }
        defer { close(fd) }
        // One write per record, so concurrent hooks never interleave lines.
        _ = data.withUnsafeBytes { Darwin.write(fd, $0.baseAddress, $0.count) }
    }

    static func readCursorLog() -> [CursorRecord] {
        guard let data = try? Data(contentsOf: UsageLocations.cursorLog) else { return [] }
        return data.split(separator: 10).compactMap { line in
            guard let object = (try? JSONSerialization.jsonObject(with: Data(line))) as? [String: Any],
                  let conversation = object["conversation"] as? String, let generation = object["generation"] as? String,
                  let event = object["event"] as? String, let time = (object["ts"] as? String).flatMap(parseDate) else { return nil }
            return CursorRecord(conversation: conversation, generation: generation, event: event, model: object["model"] as? String, time: time)
        }
    }

    // MARK: Report

    private struct Accumulator {
        var counts: Bucket = [0, 0, 0, 0, 0, 0]
        var sessions = Set<String>()
        var turns = 0
        var activeSeconds = 0
    }

    private static func build(cache: CacheFile, cursor: [CursorRecord], periods: [UsagePeriod], now: Date, calendar: Calendar) -> UsageReport {
        // Flatten buckets once: (agent, model, session, hour, counts).
        var rows: [(agent: UsageAgent, model: String, session: String, hour: Date, counts: Bucket)] = []
        for state in cache.files.values {
            guard let agent = UsageAgent(rawValue: state.agent) else { continue }
            for (key, counts) in state.buckets {
                let parts = key.split(separator: separator, omittingEmptySubsequences: false).map(String.init)
                guard parts.count == 3, let hour = Double(parts[2]) else { continue }
                rows.append((agent, parts[0], parts[1], Date(timeIntervalSince1970: hour), counts))
            }
        }
        // Cursor turns: one per generation with a stop.
        var prompts: [String: Date] = [:]
        var turns: [String: (conversation: String, model: String, stop: Date, active: Int)] = [:]
        for record in cursor where record.event == "prompt" { prompts[record.generation] = prompts[record.generation] ?? record.time }
        var promptModels: [String: String] = [:]
        for record in cursor where record.event == "prompt" { if let model = record.model { promptModels[record.generation] = model } }
        for record in cursor where record.event == "stop" && turns[record.generation] == nil {
            let active = prompts[record.generation].map { Int(min(max(0, record.time.timeIntervalSince($0)), 14400)) } ?? 0
            let model = record.model ?? promptModels[record.generation] ?? "unknown"
            turns[record.generation] = (record.conversation, UsagePricing.normalized(model), record.time, active)
        }

        let result = periods.map { period -> PeriodUsage in
            let start = period.start(now: now, calendar: calendar)
            let inPeriod = { (date: Date) in (start.map { date >= $0 } ?? true) && date <= now }
            var agents: [AgentUsage] = []
            for agent in UsageAgent.allCases {
                var models: [String: Accumulator] = [:]
                var sessions = Set<String>()
                if agent == .cursor {
                    for turn in turns.values where inPeriod(turn.stop) {
                        models[turn.model, default: Accumulator()].turns += 1
                        models[turn.model, default: Accumulator()].activeSeconds += turn.active
                        models[turn.model, default: Accumulator()].sessions.insert(turn.conversation)
                    }
                    for record in cursor where inPeriod(record.time) { sessions.insert(record.conversation) }
                } else {
                    for row in rows where row.agent == agent && inPeriod(row.hour) {
                        models[row.model, default: Accumulator()].counts = zip(models[row.model, default: Accumulator()].counts, row.counts).map { $0 + $1 }
                        models[row.model, default: Accumulator()].sessions.insert(row.session)
                        sessions.insert(row.session)
                    }
                }
                var modelTotals = models.map { name, accumulator in totals(name, accumulator, agent: agent) }
                modelTotals.sort { a, b in
                    if a.totalTokens != b.totalTokens { return a.totalTokens > b.totalTokens }
                    if a.turns != b.turns { return a.turns > b.turns }
                    return a.model < b.model
                }
                var total = UsageTotals()
                total.sessions = sessions.count
                for model in modelTotals {
                    total.requests += model.requests
                    total.turns += model.turns
                    total.activeSeconds += model.activeSeconds
                    total.inputTokens += model.inputTokens
                    total.outputTokens += model.outputTokens
                    total.cacheReadTokens += model.cacheReadTokens
                    total.cacheWriteTokens += model.cacheWriteTokens
                    total.unpricedTokens += model.unpricedTokens
                }
                if agent == .cursor {
                    total.estimatedCost = nil
                } else {
                    let cost = modelTotals.compactMap(\.estimatedCost).reduce(0, +)
                    total.estimatedCost = total.totalTokens > 0 && total.unpricedTokens == total.totalTokens ? nil : rounded(cost)
                }
                agents.append(AgentUsage(agent: agent, totals: total, models: modelTotals))
            }
            return PeriodUsage(period: period, start: start, agents: agents)
        }
        let codex = cache.files.values.compactMap(\.limits).max { $0.observedAt < $1.observedAt }
        let limits = [ClaudeLimits.current(now: now),
                      codex?.limits(now: now) ?? .unavailable(.codex, name: "ChatGPT plan"),
                      .unavailable(.cursor, name: "Cursor")]
        return UsageReport(generatedAt: now, periods: result, limits: limits)
    }

    private static func totals(_ model: String, _ accumulator: Accumulator, agent: UsageAgent) -> UsageTotals {
        let c = accumulator.counts
        var totals = UsageTotals(model: model, sessions: accumulator.sessions.count, requests: c[5], turns: accumulator.turns,
                                 activeSeconds: accumulator.activeSeconds, inputTokens: c[0], outputTokens: c[1],
                                 cacheReadTokens: c[2], cacheWriteTokens: c[3] + c[4])
        if agent == .cursor {
            totals.estimatedCost = nil
        } else if let price = UsagePricing.price(for: model) {
            let cost = Double(c[0]) * price.input + Double(c[1]) * price.output + Double(c[2]) * price.cacheRead
                + Double(c[3]) * price.cacheWrite5m + Double(c[4]) * price.cacheWrite1h
            totals.estimatedCost = rounded(cost / 1_000_000)
        } else {
            totals.unpricedTokens = totals.totalTokens
            totals.estimatedCost = totals.totalTokens > 0 ? nil : 0
        }
        return totals
    }

    private static func rounded(_ value: Double) -> Double { (value * 1_000_000).rounded() / 1_000_000 }

    // MARK: Dates

    static func parseDate(_ text: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = fractional.date(from: text) { return date }
        return ISO8601DateFormatter().date(from: text)
    }
    static func formatDate(_ date: Date) -> String { ISO8601DateFormatter().string(from: date) }
}

// MARK: - Output

public extension UsageReport {
    /// The JSON object described in docs/USAGE.md.
    var jsonObject: [String: Any] {
        func totals(_ t: UsageTotals) -> [String: Any] {
            ["sessions": t.sessions, "requests": t.requests, "turns": t.turns, "activeSeconds": t.activeSeconds,
             "inputTokens": t.inputTokens, "outputTokens": t.outputTokens, "cacheReadTokens": t.cacheReadTokens,
             "cacheWriteTokens": t.cacheWriteTokens, "totalTokens": t.totalTokens, "unpricedTokens": t.unpricedTokens,
             "estimatedCost": t.estimatedCost.map { $0 as Any } ?? NSNull()]
        }
        func window(_ w: LimitWindow) -> [String: Any] {
            var object: [String: Any] = ["id": w.id, "label": w.label, "windowMinutes": w.windowMinutes.map { $0 as Any } ?? NSNull(),
                                         "usedPercent": w.usedPercent, "resetsAt": w.resetsAt.map { UsageStore.formatDate($0) as Any } ?? NSNull(),
                                         "reset": w.reset]
            if let usd = w.usedUSD { object["usedUSD"] = usd }
            if let usd = w.limitUSD { object["limitUSD"] = usd }
            if let period = w.period { object["period"] = period }
            return object
        }
        return [
            "limits": limits.map { plan -> [String: Any] in
                ["agent": plan.agent.rawValue, "name": plan.name, "available": plan.available,
                 "source": plan.source.map { $0 as Any } ?? NSNull(),
                 "observedAt": plan.observedAt.map { UsageStore.formatDate($0) as Any } ?? NSNull(),
                 "plan": plan.plan.map { $0 as Any } ?? NSNull(),
                 "credits": plan.credits.map { ["balance": $0.balance.map { $0 as Any } ?? NSNull(), "unlimited": $0.unlimited] as Any } ?? NSNull(),
                 "windows": plan.windows.map(window)]
            },
            "estimateNote": Self.estimateNote,
            "generatedAt": UsageStore.formatDate(generatedAt),
            "periods": periods.map { period -> [String: Any] in
                [
                    "id": period.period.rawValue, "label": period.period.label,
                    "start": period.start.map { UsageStore.formatDate($0) as Any } ?? NSNull(),
                    "totalTokens": period.totalTokens, "estimatedCost": (period.estimatedCost * 1_000_000).rounded() / 1_000_000,
                    "agents": period.agents.map { agent -> [String: Any] in
                        var object = totals(agent.totals)
                        object["agent"] = agent.agent.rawValue
                        object["name"] = agent.agent.displayName
                        object["models"] = agent.models.map { model -> [String: Any] in
                            var entry = totals(model)
                            entry["model"] = model.model
                            return entry
                        }
                        return object
                    }
                ]
            }
        ]
    }

    func json() -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: jsonObject, options: [.sortedKeys, .withoutEscapingSlashes]) else { return "{}" }
        return String(decoding: data, as: UTF8.self)
    }

    /// The `barista usage` text. Per-model lines appear when `detailed`.
    func text(detailed: Bool) -> String {
        var lines = ["Limits"]
        for plan in limits {
            lines.append("  " + plan.name.padding(toLength: 14, withPad: " ", startingAt: 0) + Self.limitSummary(plan, now: generatedAt))
        }
        lines += ["", "Usage from local logs · costs are API-price estimates, not your subscription bill"]
        let dateFormat = Date.FormatStyle().weekday(.abbreviated).month(.abbreviated).day()
        for period in periods {
            lines.append("")
            switch period.period {
            case .week, .month: lines.append("\(period.period.label) (since \(period.start.map { $0.formatted(dateFormat) } ?? ""))")
            default: lines.append(period.period.label)
            }
            for agent in period.agents {
                lines.append("  " + Self.summary(agent.agent.displayName.padding(toLength: 12, withPad: " ", startingAt: 0), agent.totals, agent: agent.agent))
                if detailed {
                    let width = agent.models.map(\.model.count).max() ?? 0
                    for model in agent.models {
                        let name = model.model.padding(toLength: width, withPad: " ", startingAt: 0)
                        lines.append("      " + Self.summary(name, model, agent: agent.agent, sessions: false))
                    }
                }
            }
        }
        return lines.joined(separator: "\n")
    }

    static func summary(_ name: String, _ t: UsageTotals, agent: UsageAgent, sessions: Bool = true) -> String {
        let count = { (n: Int, word: String) in "\(n) \(word)\(n == 1 ? "" : "s")" }
        if agent == .cursor {
            guard t.turns > 0 || t.sessions > 0 else { return "\(name)  —" }
            let parts = (sessions ? [count(t.sessions, "session")] : []) + [count(t.turns, "turn"), "\(formatDuration(t.activeSeconds)) active"]
            return "\(name)  " + parts.joined(separator: " · ")
        }
        guard t.totalTokens > 0 else { return "\(name)  —" }
        let parts = (sessions ? [count(t.sessions, "session")] : []) + ["\(formatTokens(t.totalTokens)) tokens", formatCost(t.estimatedCost)]
        return "\(name)  " + parts.joined(separator: " · ")
    }

    /// "5-hour 24% (resets 3:10 PM) · Weekly 0% (reset) · plan prolite · as of 2 hr ago".
    static func limitSummary(_ plan: PlanLimits, now: Date) -> String {
        guard plan.available else {
            switch plan.agent {
            case .claude: return "no data yet (open Barista and allow it to read your Claude account, or run barista limits refresh)"
            case .codex: return "no data yet (appears after your next Codex turn)"
            case .cursor: return "not available on this computer"
            }
        }
        var parts = plan.windows.map { window -> String in
            var percent = "\(window.label) \(Int(window.usedPercent.rounded()))%"
            if let used = window.usedUSD, let limit = window.limitUSD {
                percent += String(format: " ($%.2f of $%.2f)", used, limit)
            }
            if window.reset { return percent + " (reset)" }
            guard let resets = window.resetsAt else { return percent }
            return percent + " (resets \(formatReset(resets, now: now)))"
        }
        if let plan = plan.plan { parts.append("plan \(plan)") }
        if let credits = plan.credits, !credits.unlimited, let balance = credits.balance, let value = Double(balance) {
            parts.append("\(Int(value)) credits")
        }
        if let observed = plan.observedAt, now.timeIntervalSince(observed) > 600 {
            parts.append("as of \(RelativeDateTimeFormatter().localizedString(for: observed, relativeTo: now))")
        }
        return parts.joined(separator: " · ")
    }
    /// A time today or tomorrow ("3:10 PM"), else a date ("Nov 4").
    static func formatReset(_ date: Date, now: Date = Date()) -> String {
        date.timeIntervalSince(now) < 86400
            ? date.formatted(date: .omitted, time: .shortened)
            : date.formatted(.dateTime.month(.abbreviated).day())
    }

    static func formatTokens(_ value: Int) -> String {
        let n = Double(value)
        switch value {
        case ..<1000: return "\(value)"
        case ..<1_000_000: return String(format: "%.1fK", n / 1e3)
        case ..<1_000_000_000: return String(format: "%.1fM", n / 1e6)
        default: return String(format: "%.1fB", n / 1e9)
        }
    }
    static func formatCost(_ cost: Double?) -> String {
        guard let cost else { return "no price" }
        if cost > 0 && cost < 0.01 { return "<$0.01" }
        return String(format: "~$%.2f", cost)
    }
    static func formatDuration(_ seconds: Int) -> String {
        seconds >= 3600 ? "\(seconds / 3600)h \(seconds % 3600 / 60)m" : "\(max(0, seconds) / 60)m"
    }
}
