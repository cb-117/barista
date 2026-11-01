import BaristaCore
import Foundation

@MainActor
private final class TestPower: PowerAsserting {
    var active = false
    var display = false
    var activations = 0
    func activate(keepDisplayAwake: Bool) throws { active = true; display = keepDisplayAwake; activations += 1 }
    func deactivate() { active = false }
}

private func check(_ value: @autoclosure () -> Bool, _ message: String, file: StaticString = #file, line: UInt = #line) {
    precondition(value(), message, file: file, line: line)
}

@MainActor
func concurrentOwnershipAndDisplayAggregation() throws {
    let power = TestPower()
    let session = AwakeSession(power: power, schedulesTimer: false, batteryProvider: { nil })
    let build = try session.acquire(title: "Build", source: .agent, duration: 300)
    check(power.active && !power.display, "Agent work should allow display sleep by default")
    let render = try session.acquire(title: "Render", source: .command, duration: 300, display: true)
    check(power.display && session.sessions.count == 2, "Aggregate display requirements")
    session.release(id: render.id)
    check(power.active && !power.display, "Releasing one task cannot stop another")
    session.start(duration: 900)
    check(power.display && session.sessions.count == 2, "Manual work coexists with agents")
    session.stopManual()
    check(power.active && session.sessions.first?.id == build.id, "Stop my session affects only manual work")
    session.release(id: build.id)
    check(!power.active && !session.isActive, "Last owner releases power assertion")
    session.release(id: build.id)
    check(session.history.count == 3, "Repeated release is idempotent")
}

@MainActor
func heartbeatsExpireAndRespectHardLimit() throws {
    let power = TestPower()
    var now = Date(timeIntervalSince1970: 1000)
    let session = AwakeSession(power: power, schedulesTimer: false, now: { now }, batteryProvider: { nil })
    let task = try session.acquire(title: "Agent", source: .agent, duration: 20, maxDuration: 60)
    now = now.addingTimeInterval(10)
    let update = try session.heartbeat(id: task.id, seconds: 30, progress: 0.5, message: "Tests running")
    check(update.expiresAt == Date(timeIntervalSince1970: 1040), "Heartbeat renews from now")
    check(update.progress == 0.5 && update.message == "Tests running", "Progress is inspectable")
    now = now.addingTimeInterval(20)
    let capped = try session.heartbeat(id: task.id, seconds: 300)
    check(capped.expiresAt == Date(timeIntervalSince1970: 1060), "Renewal cannot escape hard limit")
    now = Date(timeIntervalSince1970: 1061)
    let reply = session.handle(ControlRequest(action: .heartbeat, id: task.id, seconds: 300))
    check(!reply.ok && !session.isActive && !power.active, "Expired work cannot be revived")
    check(session.history.first?.reason == .expired, "Expiration is recorded")
}

@MainActor
func deadAndReusedProcessesReleaseTheirLeases() throws {
    var identity: String? = "first-process"
    let power = TestPower()
    let session = AwakeSession(power: power, schedulesTimer: false, processIdentity: { _ in identity }, batteryProvider: { nil })
    _ = try session.acquire(title: "Watched app", source: .application, duration: 300, processID: 42)
    identity = "different-process-same-pid"
    session.refresh()
    check(!power.active && session.history.first?.reason == .processExited, "PID reuse must not keep a lease alive")
    identity = nil
    let reply = session.handle(ControlRequest(action: .acquire, title: "Dead PID", processID: 42))
    check(!reply.ok && !session.isActive, "Cannot attach to a dead process")
}

@MainActor
func batteryProtectionCannotBeBypassedByRenewal() throws {
    var battery = BatteryState(percent: 60, onBattery: true)
    let power = TestPower()
    let session = AwakeSession(power: power, schedulesTimer: false, batteryProvider: { battery })
    let task = try session.acquire(title: "Long job", source: .agent, duration: 300)
    session.start(duration: nil)
    battery = BatteryState(percent: 20, onBattery: true)
    session.refresh()
    check(!power.active && session.batteryBlocked && session.history.count == 2, "Low battery ends all awake requests")
    check(session.history.allSatisfy { $0.reason == .batteryLow }, "Battery reason is visible")
    check(!session.handle(ControlRequest(action: .heartbeat, id: task.id)).ok, "Heartbeat cannot bypass battery cutoff")
    check(!session.handle(ControlRequest(action: .acquire, title: "Retry")).ok, "New work is blocked until power recovers")
    battery = BatteryState(percent: 10, onBattery: false)
    session.refresh()
    check(!session.batteryBlocked && !session.isActive, "Power connection permits new work without reviving old sessions")
    _ = try session.acquire(title: "Connected", source: .agent, duration: 300)
    check(power.active, "Battery percentage alone must not block external power")
    session.stop()
}

@MainActor
func inputValidationAndUserStopRemainAuthoritative() throws {
    let session = AwakeSession(power: TestPower(), schedulesTimer: false, batteryProvider: { nil })
    for seconds in [0.0, -1.0, Double.nan, Double.infinity, 86401] {
        check(!session.handle(ControlRequest(action: .acquire, seconds: seconds)).ok, "Invalid TTL rejected")
    }
    check(!session.handle(ControlRequest(action: .acquire, title: "\n")).ok, "Blank names rejected")
    check(!session.handle(ControlRequest(action: .release, id: "not-an-id")).ok, "Malformed IDs rejected")
    let lease = try session.acquire(title: "Work", source: .agent, duration: 300)
    for progress in [-1.0, 1.1, Double.nan] {
        check(!session.handle(ControlRequest(action: .heartbeat, id: lease.id, progress: progress)).ok, "Invalid progress rejected")
    }
    check(!session.handle(ControlRequest(action: .heartbeat, id: lease.id, message: String(repeating: "x", count: 201))).ok, "Oversized message rejected")
    session.cancel(id: lease.id)
    check(!session.handle(ControlRequest(action: .heartbeat, id: lease.id)).ok, "User Stop cannot be undone by a stale heartbeat")
    check(session.history.first?.reason == .stopped, "User Stop is distinct from successful completion")
}

@MainActor
func boundedHistoryDoesNotRestoreAwakeRequests() throws {
    let suite = "BaristaChecks.\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    defer { defaults.removePersistentDomain(forName: suite) }
    let session = AwakeSession(power: TestPower(), schedulesTimer: false, batteryProvider: { nil }, defaults: defaults)
    for i in 0..<25 {
        let lease = try session.acquire(title: "Job \(i)", source: .agent, duration: 300)
        session.release(id: lease.id)
    }
    check(session.history.count == 20, "History is bounded")
    let restored = AwakeSession(power: TestPower(), schedulesTimer: false, batteryProvider: { nil }, defaults: defaults)
    check(restored.history.count == 20 && !restored.isActive, "History persists, active leases never silently resume")
    restored.clearHistory()
    check(defaults.data(forKey: "sessionHistory") == nil, "Clear removes saved summaries")
}

/// Blocking socket clients run on GCD threads. Running them in Swift tasks can use up the
/// cooperative pool on small CI machines, so the server's reply task never gets a thread.
private func blocking<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global().async { continuation.resume(with: Result { try work() }) }
    }
}

private final class ReplyBox: @unchecked Sendable {
    private let lock = NSLock()
    private var results: [Result<ControlReply, Error>] = []
    func add(_ result: Result<ControlReply, Error>) { lock.lock(); results.append(result); lock.unlock() }
    func values() throws -> [ControlReply] { lock.lock(); defer { lock.unlock() }; return try results.map { try $0.get() } }
}

@MainActor
func localSocketSupportsConcurrentAgents() async throws {
    let directory = URL(fileURLWithPath: "/tmp/barista-check-\(UUID().uuidString.prefix(8))")
    let path = directory.appendingPathComponent("control.sock").path
    let session = AwakeSession(power: TestPower(), schedulesTimer: false, batteryProvider: { nil })
    let server = ControlServer()
    try server.start(path: path) { request in await MainActor.run { session.handle(request) } }
    defer { server.stop(); try? FileManager.default.removeItem(at: directory) }
    let replies = try await blocking {
        let results = ReplyBox()
        DispatchQueue.concurrentPerform(iterations: 10) { i in
            results.add(Result { try ControlClient(path: path).send(ControlRequest(action: .acquire, title: "Parallel agent \(i)")) })
        }
        return try results.values()
    }
    check(replies.allSatisfy(\.ok) && session.sessions.count == 10, "Concurrent agents have independent sessions")
    let ids = replies.compactMap(\.session?.id)
    check(Set(ids).count == 10, "Every agent gets a distinct ID")
    let stopped = try await blocking { try ControlClient(path: path).send(ControlRequest(action: .release, id: ids[0])) }
    check(stopped.ok && session.sessions.count == 9, "Socket release preserves other clients")
    server.stop()
    try server.start(path: path) { request in await MainActor.run { session.handle(request) } }
    let restarted = try await blocking { try ControlClient(path: path).send(ControlRequest(action: .status)) }
    check(restarted.ok, "Rapid disable/enable must not lose the new socket to an old cancellation")
    session.stop()
}

@MainActor
func runTaskSessionChecks() async throws {
    try concurrentOwnershipAndDisplayAggregation()
    try heartbeatsExpireAndRespectHardLimit()
    try deadAndReusedProcessesReleaseTheirLeases()
    try batteryProtectionCannotBeBypassedByRenewal()
    try inputValidationAndUserStopRemainAuthoritative()
    try boundedHistoryDoesNotRestoreAwakeRequests()
    try await localSocketSupportsConcurrentAgents()
    print("Passed 7 task-session checks: concurrent ownership, heartbeat expiry, hard caps, PID reuse, battery cutoff, validation, history, and real Unix-socket concurrency.")
}
