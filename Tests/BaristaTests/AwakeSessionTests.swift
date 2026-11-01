import Foundation
import AppKit
import BaristaCore

@MainActor
private final class FakePower: PowerAsserting {
    var active = false
    var display = false
    var shouldFail = false
    var releases = 0

    func activate(keepDisplayAwake: Bool) throws {
        if shouldFail { throw CocoaError(.featureUnsupported) }
        active = true
        display = keepDisplayAwake
    }

    func deactivate() {
        active = false
        releases += 1
    }
}

@MainActor
func timedSessionReleasesPowerAtDeadline() {
    let power = FakePower()
    var now = Date(timeIntervalSince1970: 1000)
    let session = AwakeSession(power: power, schedulesTimer: false, now: { now }, batteryProvider: { nil })
    session.start(duration: 90)
    expect(power.active && power.display)
    expect(session.countdown == "01:30")
    now = now.addingTimeInterval(45)
    session.refresh()
    expect(session.remaining == 45)
    expect(session.progress == 0.5)
    now = now.addingTimeInterval(50)
    session.refresh()
    expect(!session.isActive && !power.active)
    expect(power.releases == 1)
}

@MainActor
func indefiniteSessionNeedsExplicitStop() {
    let power = FakePower()
    var now = Date()
    let session = AwakeSession(power: power, schedulesTimer: false, now: { now }, batteryProvider: { nil })
    session.start(duration: nil)
    now = now.addingTimeInterval(1_000_000)
    session.refresh()
    expect(session.isIndefinite && power.active)
    expect(session.menuBarCountdown == "∞")
    session.stop()
    expect(!session.isActive && !power.active)
}

@MainActor
func extensionMovesDeadlineAndCannotReviveExpiredSession() {
    let power = FakePower()
    var now = Date(timeIntervalSince1970: 1000)
    let session = AwakeSession(power: power, schedulesTimer: false, now: { now }, batteryProvider: { nil })
    session.start(duration: 60)
    session.addTime(900)
    expect(session.endDate == Date(timeIntervalSince1970: 1960))
    expect(session.remaining == 960)
    now = Date(timeIntervalSince1970: 2000)
    session.addTime(900)
    expect(!session.isActive && !power.active)
}

@MainActor
func displayPreferenceUpdatesActiveAssertion() {
    let power = FakePower()
    let session = AwakeSession(power: power, schedulesTimer: false, batteryProvider: { nil })
    session.start(duration: 300)
    session.keepDisplayAwake = false
    expect(power.active && !power.display)
    expect(session.isActive)
    // A failed upgrade keeps the working assertion and the session, then retries.
    power.shouldFail = true
    session.keepDisplayAwake = true
    expect(session.isActive && power.active && !power.display)
    expect(session.error != nil)
    power.shouldFail = false
    session.refresh()
    expect(session.isActive && power.active && power.display)
    expect(session.error == nil)
}

@MainActor
func stopTokenAndTurnCap() {
    let power = FakePower()
    var now = Date(timeIntervalSince1970: 1000)
    let session = AwakeSession(power: power, schedulesTimer: false, now: { now }, processIdentity: { _ in "same" }, batteryProvider: { nil })
    let token = session.stopToken
    let lease = try! session.acquire(title: "Agent", source: .agent, duration: 60, maxDuration: 100)
    session.release(id: lease.id)
    expect(session.stopToken == token)  // An owner's release is not a user stop.
    expect(session.handle(ControlRequest(action: .acquire, title: "Resume", seconds: 60, stopToken: token)).ok)
    session.stop()
    expect(session.stopToken != token)
    expect(!session.handle(ControlRequest(action: .acquire, title: "Resume", seconds: 60, stopToken: token)).ok)
    // A heartbeat with maxSeconds restarts the hard cap (a new user turn).
    let turn = try! session.acquire(title: "Turn", source: .agent, duration: 60, maxDuration: 100)
    now = now.addingTimeInterval(50)
    _ = try! session.heartbeat(id: turn.id, seconds: 60, restartingCap: 100)  // Cap now t=150, not t=100.
    now = now.addingTimeInterval(50)
    _ = try! session.heartbeat(id: turn.id, seconds: 60)
    now = now.addingTimeInterval(40)
    session.refresh()
    expect(session.sessions.contains { $0.id == turn.id })
    now = now.addingTimeInterval(20)
    session.refresh()
    expect(!session.sessions.contains { $0.id == turn.id })
    expect(session.sessions.isEmpty)
    // Battery protection revokes lapsed turns even with nothing active, and stays revoked after reconnecting.
    var battery = BatteryState(percent: 50, onBattery: true)
    let guarded = AwakeSession(power: power, schedulesTimer: false, now: { now }, batteryProvider: { battery })
    let before = guarded.stopToken
    battery = BatteryState(percent: 10, onBattery: true)
    guarded.refresh()
    let during = guarded.stopToken
    expect(during != before)
    guarded.refresh()
    expect(guarded.stopToken == during)  // Rotates on entry, not every tick.
    battery = BatteryState(percent: 10, onBattery: false)
    expect(!guarded.handle(ControlRequest(action: .acquire, title: "Resume", seconds: 60, stopToken: before)).ok)
}

@MainActor
func failedDowngradeKeepsOtherSessionsProtected() {
    let power = FakePower()
    let session = AwakeSession(power: power, schedulesTimer: false, processIdentity: { _ in "same" }, batteryProvider: { nil })
    let display = try! session.acquire(title: "Display task", source: .agent, duration: 300, display: true)
    _ = try! session.acquire(title: "Build", source: .command, duration: 300, display: false)
    power.shouldFail = true
    session.release(id: display.id)
    expect(session.sessions.map(\.title) == ["Build"])
    expect(power.active && power.display) // Keeps the stronger assertion rather than none.
    power.shouldFail = false
    session.refresh()
    expect(power.active && !power.display)
}

@MainActor
func failedAndInvalidStartsDoNotClaimMacIsAwake() {
    let power = FakePower()
    let session = AwakeSession(power: power, schedulesTimer: false, batteryProvider: { nil })
    for duration in [0.0, -60.0, Double.nan, Double.infinity] {
        session.start(duration: duration)
        expect(!session.isActive && !power.active)
        expect(session.error != nil)
    }
    power.shouldFail = true
    session.start(duration: 60)
    expect(!session.isActive && session.error != nil)
}

func customDurationRejectsInvalidText() {
    expect(CustomDuration.parse("90") == 90)
    expect(CustomDuration.parse(" 1,440 ") == 1440)
    expect(CustomDuration.parse("1") == 1)
    for text in ["", "  ", "abc", "0", "-5", "1441", "1.5", "90m", "99999999999999999999"] {
        expect(CustomDuration.parse(text) == nil)
    }
}

private func expect(_ condition: @autoclosure () -> Bool, file: StaticString = #file, line: UInt = #line) {
    precondition(condition(), "Session check failed", file: file, line: line)
}

@main
struct SessionChecks {
    @MainActor static func main() async throws {
        if let index = CommandLine.arguments.firstIndex(of: "--serve"), index + 1 < CommandLine.arguments.count {
            let session = AwakeSession(power: FakePower(), batteryProvider: { nil })
            let server = ControlServer()
            try server.start(path: CommandLine.arguments[index + 1]) { request in
                await MainActor.run { session.handle(request) }
            }
            print("READY")
            while true { try await Task.sleep(nanoseconds: 1_000_000_000) }
        }
        timedSessionReleasesPowerAtDeadline()
        indefiniteSessionNeedsExplicitStop()
        extensionMovesDeadlineAndCannotReviveExpiredSession()
        displayPreferenceUpdatesActiveAssertion()
        failedAndInvalidStartsDoNotClaimMacIsAwake()
        failedDowngradeKeepsOtherSessionsProtected()
        stopTokenAndTurnCap()
        customDurationRejectsInvalidText()
        print("Passed all 8 session checks: expiration, indefinite sessions, extensions, display changes, failures, failed transitions, stop tokens and turn caps, and custom-duration input.")
        try await runTaskSessionChecks()
        if CommandLine.arguments.contains("--power") {
            try checkNativePowerAssertions()
        }
        if CommandLine.arguments.contains("--running-app") {
            let app = NSWorkspace.shared.runningApplications.first { $0.bundleIdentifier == "com.connorbescos.barista" }
            print(app.map { "Barista is running (PID \($0.processIdentifier))." } ?? "Barista is not running.")
        }
    }
}

@MainActor
private func checkNativePowerAssertions() throws {
    func ownedAssertions() throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/pmset")
        process.arguments = ["-g", "assertions"]
        let output = Pipe()
        process.standardOutput = output
        try process.run()
        let data = output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        precondition(process.terminationStatus == 0)
        let text = String(decoding: data, as: UTF8.self)
        return text.split(separator: "\n").filter {
            $0.contains("pid \(ProcessInfo.processInfo.processIdentifier)(") && $0.contains("Barista awake session")
        }.joined(separator: "\n")
    }

    let power = PowerAssertion()
    defer { power.deactivate() }
    try power.activate(keepDisplayAwake: true)
    let displayOn = try ownedAssertions()
    expect(displayOn.contains("PreventUserIdleSystemSleep"))
    expect(displayOn.contains("PreventUserIdleDisplaySleep"))
    try power.activate(keepDisplayAwake: false)
    let displayOff = try ownedAssertions()
    expect(displayOff.contains("PreventUserIdleSystemSleep"))
    expect(!displayOff.contains("PreventUserIdleDisplaySleep"))
    power.deactivate()
    let released = try ownedAssertions()
    expect(released.isEmpty)
    print("Passed native macOS integration: system and display assertions activate, update, and release.")
}
