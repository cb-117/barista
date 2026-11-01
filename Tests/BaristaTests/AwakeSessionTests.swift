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
    power.shouldFail = true
    session.keepDisplayAwake = true
    expect(!session.isActive && !power.active)
    expect(session.error != nil)
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
        customDurationRejectsInvalidText()
        print("Passed all 6 session checks: expiration, indefinite sessions, extensions, display changes, failures, and custom-duration input.")
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
