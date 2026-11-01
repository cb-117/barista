import Combine
import Foundation

/// Independent, expiring sessions share one native power assertion.
@MainActor
public final class AwakeSession: ObservableObject {
    @Published public private(set) var sessions: [CoffeeLease] = []
    @Published public private(set) var history: [SessionCompletion] = []
    @Published public private(set) var error: String?
    @Published public private(set) var battery: BatteryState?
    @Published public private(set) var currentDate: Date
    @Published public var batteryThreshold = 20 { didSet { refresh() } }
    @Published public var keepDisplayAwake = true {
        didSet {
            guard let index = sessions.firstIndex(where: { $0.source == .manual }),
                  oldValue != keepDisplayAwake else { return }
            sessions[index].keepDisplayAwake = keepDisplayAwake
            reconcilePower()
        }
    }

    public var onCompletion: ((SessionCompletion) -> Void)?
    private let power: any PowerAsserting
    private let now: () -> Date
    private let processIdentity: (Int32) -> String?
    private let batteryProvider: () -> BatteryState?
    private let defaults: UserDefaults?
    private var timer: Timer?
    private var assertionDisplay: Bool?
    private var powerRetryPending = false
    private var wasBatteryBlocked = false
    /// Random per app run; replaced on every user stop, battery cutoff, or quit.
    public private(set) var stopToken = UUID().uuidString.lowercased()

    public init(power: any PowerAsserting = PowerAssertion(), schedulesTimer: Bool = true,
                now: @escaping () -> Date = Date.init,
                processIdentity: @escaping (Int32) -> String? = SystemState.processIdentity,
                batteryProvider: @escaping () -> BatteryState? = SystemState.battery,
                defaults: UserDefaults? = nil) {
        self.power = power
        self.now = now
        self.processIdentity = processIdentity
        self.batteryProvider = batteryProvider
        self.defaults = defaults
        currentDate = now()
        battery = batteryProvider()
        if let data = defaults?.data(forKey: "sessionHistory"),
           let saved = try? CoffeeJSON.decoder().decode([SessionCompletion].self, from: data) {
            history = Array(saved.prefix(20))
        }
        if schedulesTimer {
            let timer = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
                Task { @MainActor in self?.refresh() }
            }
            self.timer = timer
            RunLoop.main.add(timer, forMode: .common)
        }
    }

    public var isActive: Bool { !sessions.isEmpty }
    public var manualSession: CoffeeLease? { sessions.first { $0.source == .manual } }
    public var isIndefinite: Bool { isActive && sessions.contains { $0.expiresAt == nil } }
    public var endDate: Date? { isIndefinite ? nil : sessions.compactMap(\.expiresAt).max() }
    public var remaining: TimeInterval { max(0, endDate?.timeIntervalSince(currentDate) ?? 0) }
    public var batteryBlocked: Bool {
        guard let battery, batteryThreshold > 0 else { return false }
        return battery.onBattery && battery.percent <= batteryThreshold
    }
    public var progress: Double {
        guard let session = manualSession, let end = session.expiresAt else { return 0 }
        let total = end.timeIntervalSince(session.startedAt)
        return total > 0 ? min(1, max(0, 1 - end.timeIntervalSince(currentDate) / total)) : 1
    }
    public var countdown: String { Self.formatTime(remaining) }
    nonisolated public static func formatTime(_ interval: TimeInterval) -> String {
        let seconds = Int(ceil(max(0, min(interval, 31_536_000))))
        if seconds >= 3600 { return String(format: "%d:%02d:%02d", seconds / 3600, seconds % 3600 / 60, seconds % 60) }
        return String(format: "%02d:%02d", seconds / 60, seconds % 60)
    }
    /// Your own timer shows its time left. Commands and agents renew a short lease while they
    /// work, so their expiry means nothing to a person; show how many are running instead.
    public var menuBarCountdown: String {
        if let manual = manualSession {
            guard let end = manual.expiresAt else { return "∞" }
            let minutes = Int(ceil(max(0, end.timeIntervalSince(currentDate)) / 60))
            return minutes >= 60 ? "\(minutes / 60)h \(minutes % 60)m" : "\(minutes)m"
        }
        return sessions.count == 1 ? "1 task" : "\(sessions.count) tasks"
    }

    public func start(duration: TimeInterval?) {
        do {
            _ = try acquire(title: "My coffee break", source: .manual, duration: duration,
                            display: keepDisplayAwake, replacingManual: true)
        } catch { self.error = error.localizedDescription }
    }

    @discardableResult
    public func acquire(title: String, source: SessionSource, duration: TimeInterval?, display: Bool = false,
                        processID: Int32? = nil, maxDuration: TimeInterval? = nil,
                        replacingManual: Bool = false) throws -> CoffeeLease {
        refresh()
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, title.count <= 100,
              !title.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
            throw CoffeeError.message("Use a session name of 1–100 characters without line breaks.")
        }
        if let duration { try validateDuration(duration) }
        if let maxDuration { try validateDuration(maxDuration) }
        guard !(source == .agent || source == .command) || duration != nil else {
            throw CoffeeError.message("Agent sessions need an expiry. Send heartbeats to renew them.")
        }
        guard !batteryBlocked else { throw CoffeeError.message("Battery protection is on. Connect power or charge above \(batteryThreshold)%.") }
        guard sessions.count < 64 || (replacingManual && manualSession != nil) else { throw CoffeeError.message("There are already 64 active sessions.") }
        let identity = processID.flatMap(processIdentity)
        guard processID == nil || identity != nil else { throw CoffeeError.message("That process is no longer running or cannot be inspected.") }
        let start = currentDate
        let deadline = maxDuration.map { start.addingTimeInterval($0) }
        var expiry = duration.map { start.addingTimeInterval($0) }
        if let deadline { expiry = min(expiry ?? deadline, deadline) }
        let lease = CoffeeLease(id: UUID().uuidString.lowercased(), title: title, source: source,
                                startedAt: start, expiresAt: expiry, hardDeadline: deadline,
                                keepDisplayAwake: display, processID: processID, processIdentity: identity,
                                progress: nil, message: nil, lastHeartbeat: start)
        let existing = replacingManual ? sessions.filter { $0.source != .manual } : sessions
        let desiredDisplay = display || existing.contains(where: \.keepDisplayAwake)
        // Create the replacement assertion before changing visible ownership.
        if assertionDisplay != desiredDisplay {
            try power.activate(keepDisplayAwake: desiredDisplay)
            assertionDisplay = desiredDisplay
        }
        sessions = existing + [lease]
        error = nil
        return lease
    }

    /// Revokes every agent turn's right to resume a lapsed lease, active or not.
    public func revokeAgentTurns() { stopToken = UUID().uuidString.lowercased() }
    public func stop(reason: EndReason = .stopped) {
        // Revokes lapsed agent turns too, even when nothing is visibly active.
        revokeAgentTurns()
        finish(ids: Set(sessions.map(\.id)), reason: reason)
        error = nil
    }
    public func stopManual() {
        finish(ids: Set(sessions.filter { $0.source == .manual }.map(\.id)), reason: .stopped)
    }
    public func release(id: String) { finish(ids: [id], reason: .released) }
    public func cancel(id: String) { finish(ids: [id], reason: .stopped) }
    public func clearHistory() {
        history = []
        defaults?.removeObject(forKey: "sessionHistory")
    }
    public func addTime(_ seconds: TimeInterval) {
        guard let id = manualSession?.id else { return }
        do { try extend(id: id, seconds: seconds) }
        catch { self.error = error.localizedDescription }
    }
    public func extend(id: String, seconds: TimeInterval) throws {
        refresh()
        try validateDuration(seconds)
        guard let index = sessions.firstIndex(where: { $0.id == id }), let expiry = sessions[index].expiresAt else {
            throw CoffeeError.message("Session has ended or has no timer.")
        }
        let candidate = expiry.addingTimeInterval(seconds)
        guard candidate.timeIntervalSince(currentDate) <= 86400 else { throw CoffeeError.message("A session can have at most 24 hours remaining.") }
        sessions[index].expiresAt = sessions[index].hardDeadline.map { min($0, candidate) } ?? candidate
    }
    public func heartbeat(id: String, seconds: TimeInterval = 300, progress: Double? = nil, message: String? = nil,
                          restartingCap: TimeInterval? = nil) throws -> CoffeeLease {
        refresh()
        try validateDuration(seconds)
        if let restartingCap { try validateDuration(restartingCap) }
        if let progress, !progress.isFinite || !(0...1).contains(progress) { throw CoffeeError.message("Progress must be between 0 and 1.") }
        if let message, message.count > 200 || message.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) {
            throw CoffeeError.message("Status messages must fit on one line and be at most 200 characters.")
        }
        guard let index = sessions.firstIndex(where: { $0.id == id && $0.isTask }) else { throw CoffeeError.message("Agent session not found. It may have expired; acquire a new session.") }
        if let restartingCap { sessions[index].hardDeadline = currentDate.addingTimeInterval(restartingCap) }
        let candidate = currentDate.addingTimeInterval(seconds)
        sessions[index].expiresAt = sessions[index].hardDeadline.map { min($0, candidate) } ?? candidate
        sessions[index].lastHeartbeat = currentDate
        if let progress { sessions[index].progress = progress }
        if let message { sessions[index].message = message }
        return sessions[index]
    }

    public func refresh() {
        currentDate = now()
        battery = batteryProvider()
        if batteryBlocked {
            // Entering protection revokes lapsed turns even when no session is active.
            if !wasBatteryBlocked { revokeAgentTurns() }
            wasBatteryBlocked = true
            finish(ids: Set(sessions.map(\.id)), reason: .batteryLow)
            return
        }
        wasBatteryBlocked = false
        let exited = Set(sessions.filter { lease in
            guard let pid = lease.processID else { return false }
            return processIdentity(pid) != lease.processIdentity
        }.map(\.id))
        finish(ids: exited, reason: .processExited)
        let expired = Set(sessions.filter { $0.expiresAt.map { $0 <= currentDate } ?? false }.map(\.id))
        finish(ids: expired, reason: .expired)
        reconcilePower()
    }
    public func snapshot() -> SessionSnapshot {
        refresh()
        return SessionSnapshot(version: BaristaVersion.current, awake: isActive, displayAwake: assertionDisplay == true,
                               battery: battery, batteryThreshold: batteryThreshold, batteryBlocked: batteryBlocked,
                               sessions: sessions, history: history, error: error, stopToken: stopToken)
    }
    public func handle(_ request: ControlRequest) -> ControlReply {
        do {
            var lease: CoffeeLease?
            switch request.action {
            case .status, .history: break
            case .acquire:
                let source: SessionSource = request.source == .command ? .command : .agent
                if let token = request.stopToken, token != stopToken {
                    throw CoffeeError.message("Sessions were stopped since this turn began. Waiting for new user-initiated work.")
                }
                lease = try acquire(title: request.title ?? "Agent task", source: source,
                                    duration: request.seconds ?? 300, display: request.display ?? false,
                                    processID: request.processID, maxDuration: request.maxSeconds ?? 28800)
            case .heartbeat:
                lease = try heartbeat(id: try requiredID(request.id), seconds: request.seconds ?? 300,
                                      progress: request.progress, message: request.message,
                                      restartingCap: request.maxSeconds)
            case .release: release(id: try requiredID(request.id))
            case .extend:
                guard let seconds = request.seconds else { throw CoffeeError.message("Specify how many seconds to add.") }
                let id = try requiredID(request.id)
                try extend(id: id, seconds: seconds)
                lease = sessions.first { $0.id == id }
            case .stopAll: stop()
            }
            return ControlReply(ok: true, session: lease, state: snapshot())
        } catch { return ControlReply(ok: false, state: snapshot(), error: error.localizedDescription) }
    }

    private func validateDuration(_ duration: TimeInterval) throws {
        guard duration.isFinite, duration > 0, duration <= 86400 else { throw CoffeeError.message("Duration must be greater than zero and at most 86,400 seconds (24 hours).") }
    }
    private func requiredID(_ id: String?) throws -> String {
        guard let id, UUID(uuidString: id) != nil else { throw CoffeeError.message("Provide the session ID returned by acquire.") }
        return id
    }
    private func finish(ids: Set<String>, reason: EndReason) {
        guard !ids.isEmpty else { return }
        let finished = sessions.filter { ids.contains($0.id) }
        guard !finished.isEmpty else { return }
        if [.stopped, .batteryLow, .appQuit, .powerError].contains(reason) { stopToken = UUID().uuidString.lowercased() }
        sessions.removeAll { ids.contains($0.id) }
        for lease in finished {
            let completion = SessionCompletion(id: lease.id, title: lease.title, source: lease.source,
                                               startedAt: lease.startedAt, endedAt: currentDate, reason: reason)
            history.insert(completion, at: 0)
            onCompletion?(completion)
        }
        history = Array(history.prefix(20))
        if let data = try? CoffeeJSON.encoder().encode(history) { defaults?.set(data, forKey: "sessionHistory") }
        reconcilePower()
    }
    private func reconcilePower() {
        guard !sessions.isEmpty else {
            if assertionDisplay != nil { power.deactivate() }
            assertionDisplay = nil
            return
        }
        let display = sessions.contains(where: \.keepDisplayAwake)
        guard assertionDisplay != display else { return }
        do {
            try power.activate(keepDisplayAwake: display)
            assertionDisplay = display
            if powerRetryPending { error = nil; powerRetryPending = false }
        } catch {
            // The previous assertion is still held: keep it (and every session) rather than
            // dropping all protection, and retry on the next refresh.
            if assertionDisplay != nil {
                self.error = error.localizedDescription
                powerRetryPending = true
                return
            }
            power.deactivate()
            assertionDisplay = nil
            let finished = sessions
            sessions = []
            for lease in finished {
                history.insert(SessionCompletion(id: lease.id, title: lease.title, source: lease.source,
                                                 startedAt: lease.startedAt, endedAt: currentDate, reason: .powerError), at: 0)
            }
            history = Array(history.prefix(20))
            self.error = error.localizedDescription
        }
    }
}
