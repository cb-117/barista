import Foundation
import AppKit
import BaristaCore
import CryptoKit

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

private func expect(_ condition: @autoclosure () -> Bool, _ message: String = "", file: StaticString = #file, line: UInt = #line) {
    precondition(condition(), "Session check failed\(message.isEmpty ? "" : ": " + message)", file: file, line: line)
}

@MainActor
func menuBarShowsTimerOrTaskCount() {
    var now = Date(timeIntervalSince1970: 1000)
    let session = AwakeSession(power: FakePower(), schedulesTimer: false, now: { now }, batteryProvider: { nil })
    _ = try? session.acquire(title: "Claude Code", source: .agent, duration: 1200)
    expect(session.menuBarCountdown == "1 task")
    _ = try? session.acquire(title: "make test", source: .command, duration: 300)
    expect(session.menuBarCountdown == "2 tasks")
    session.start(duration: 600)
    now = now.addingTimeInterval(30)
    session.refresh()
    expect(session.menuBarCountdown == "10m")
    session.stop()
}

@MainActor
func updateHandoffContinuesSessions() {
    var now = Date(timeIntervalSince1970: 5000)
    let alive: Int32 = 4242, gone: Int32 = 4343
    let identity: (Int32) -> String? = { $0 == alive ? "alive-identity" : nil }
    let oldPower = FakePower()
    let old = AwakeSession(power: oldPower, schedulesTimer: false, now: { now }, processIdentity: identity, batteryProvider: { nil })
    old.start(duration: 1800)
    let agent = try! old.acquire(title: "Codex · app", source: .agent, duration: 1200, processID: alive)
    _ = try! old.acquire(title: "Short", source: .command, duration: 30)
    _ = try? old.acquire(title: "Exits during update", source: .command, duration: 600, processID: gone)
    let token = old.stopToken
    let data = old.handoff()!
    old.suspendForHandoff()
    expect(!oldPower.active && old.history.isEmpty, "an update relaunch isn't a stop")

    now = now.addingTimeInterval(60)  // the short command lease expires during the update
    let newPower = FakePower()
    let fresh = AwakeSession(power: newPower, schedulesTimer: false, now: { now }, processIdentity: identity, batteryProvider: { nil })
    expect(fresh.restore(data) == 2)
    expect(fresh.manualSession != nil && fresh.sessions.contains { $0.id == agent.id } && newPower.active)
    expect(fresh.stopToken == token, "hooks keep their right to renew")
    expect((try? fresh.heartbeat(id: agent.id, seconds: 1200)) != nil)

    let late = AwakeSession(power: FakePower(), schedulesTimer: false, now: { now.addingTimeInterval(600) }, batteryProvider: { nil })
    expect(late.restore(data) == 0 && !late.isActive, "stale handoffs are ignored")
}

func updateCheckComparesReleases() {
    expect(UpdateCheck.isVersion("v0.0.2", newerThan: "0.0.1"))
    expect(UpdateCheck.isVersion("0.1", newerThan: "0.0.9"))
    expect(UpdateCheck.isVersion("1.10.0", newerThan: "1.9.9"))
    expect(!UpdateCheck.isVersion("v0.0.1", newerThan: "0.0.1"))
    expect(!UpdateCheck.isVersion("0.0.1.0", newerThan: "0.0.1"))
    expect(!UpdateCheck.isVersion("0.0.0", newerThan: "0.0.1"))
    expect(!UpdateCheck.isVersion("nightly", newerThan: "0.0.1"))
    expect(!UpdateCheck.isVersion("1.0-beta", newerThan: "0.0.1"))
    func release(_ fields: [String: Any]) -> Data { try! JSONSerialization.data(withJSONObject: fields) }
    let base = "https://github.com/cb-117/barista/releases/download/v0.2.0/"
    func asset(_ name: String) -> [String: Any] { ["name": name, "browser_download_url": base + name] }
    let newer = release(["tag_name": "v0.2.0", "html_url": "https://github.com/cb-117/barista/releases/tag/v0.2.0",
                         "assets": [asset("Barista-0.2.0.zip"), asset("Barista-0.2.0.zip.sig"), asset("Barista-0.2.0.dmg")]])
    let update = UpdateCheck.newerRelease(in: newer, than: "0.0.1")
    expect(update?.version == "0.2.0")
    expect(update?.downloadURL?.lastPathComponent == "Barista-0.2.0.dmg")
    expect(update?.archiveURL?.lastPathComponent == "Barista-0.2.0.zip")
    expect(update?.signatureURL?.lastPathComponent == "Barista-0.2.0.zip.sig")
    expect(update?.canInstallInPlace == true)
    expect(UpdateCheck.newerRelease(in: newer, than: "0.2.0") == nil)
    // Without a signature there's no in-place install, only a download.
    let unsigned = UpdateCheck.newerRelease(in: release(["tag_name": "v0.2.0", "assets": [asset("Barista-0.2.0.zip")]]), than: "0.0.1")
    expect(unsigned?.canInstallInPlace == false && unsigned?.downloadURL?.lastPathComponent == "Barista-0.2.0.zip")
    expect(UpdateCheck.newerRelease(in: release(["tag_name": "v9.0.0", "prerelease": true]), than: "0.0.1") == nil)
    // Release data can't send the app to an arbitrary site.
    let hostile = release(["tag_name": "v9.0.0", "html_url": "https://evil.example/x",
                           "assets": [["name": "Barista-9.0.0.zip", "browser_download_url": "http://github.com/x.zip"],
                                      ["name": "Barista-9.0.0.zip.sig", "browser_download_url": "https://evil.example/x.sig"]]])
    expect(UpdateCheck.newerRelease(in: hostile, than: "0.0.1")?.pageURL == UpdateCheck.releasesPage)
    expect(UpdateCheck.newerRelease(in: hostile, than: "0.0.1")?.canInstallInPlace == false)
    // Only archives signed by the release key verify.
    let key = Curve25519.Signing.PrivateKey()
    let publicKey = key.publicKey.rawRepresentation.base64EncodedString()
    let archive = Data("barista archive".utf8)
    let signature = Data(try! key.signature(for: archive).base64EncodedString().utf8)
    expect(UpdateSignature.isValid(signature: signature, for: archive, publicKey: publicKey))
    expect(!UpdateSignature.isValid(signature: signature, for: Data("tampered".utf8), publicKey: publicKey))
    expect(!UpdateSignature.isValid(signature: signature, for: archive))  // the real release key didn't sign it
    expect(!UpdateSignature.isValid(signature: Data("garbage".utf8), for: archive, publicKey: publicKey))
    expect(UpdateCheck.newerRelease(in: Data("not json".utf8), than: "0.0.1") == nil)
}

/// Scheduled hours: stopping only them keeps agent turns; on the wire they look like an app session.
@MainActor
func scheduledSessionChecks() {
    let power = FakePower()
    let now = Date(timeIntervalSince1970: 1000)
    let session = AwakeSession(power: power, schedulesTimer: false, now: { now }, batteryProvider: { nil })
    var ended: [SessionCompletion] = []
    session.onCompletion = { ended.append($0) }
    let lease = try! session.acquire(title: "Scheduled hours", source: .application, duration: 3600, scheduled: true)
    expect(lease.isScheduled && power.active)
    let token = session.stopToken
    session.cancel(id: lease.id)
    expect(session.stopToken == token, "Stopping only scheduled hours keeps agent turns")
    expect(ended.last?.isScheduled == true && ended.last?.reason == .stopped)
    expect(!session.isActive && !power.active)
    _ = try! session.acquire(title: "Scheduled hours", source: .application, duration: 3600, scheduled: true)
    session.stop()
    expect(session.stopToken != token, "Stop all still revokes agent turns")

    // An older CLI or MCP server decodes the reply: the source is a value it knows, and it ignores `scheduled`.
    let scheduled = try! session.acquire(title: "Scheduled hours", source: .application, duration: 60, scheduled: true)
    let json = String(decoding: try! CoffeeJSON.encoder().encode(scheduled), as: UTF8.self)
    expect(json.contains("\"source\":\"application\"") && json.contains("\"scheduled\":true"))
    let plain = try! session.acquire(title: "App", source: .application, duration: 60)
    let plainJSON = try! CoffeeJSON.encoder().encode(plain)
    expect(!String(decoding: plainJSON, as: UTF8.self).contains("scheduled"), "Other sessions don't carry the field")
    expect(try! CoffeeJSON.decoder().decode(CoffeeLease.self, from: plainJSON).scheduled == nil)
    session.stop()
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
        menuBarShowsTimerOrTaskCount()
        updateCheckComparesReleases()
        updateHandoffContinuesSessions()
        print("Passed all 11 session checks: expiration, indefinite sessions, extensions, display changes, failures, failed transitions, stop tokens and turn caps, custom-duration input, menu-bar text, update checks, and update handoff.")
        try await runTaskSessionChecks()
        weeklySummaryChecks()
        limitAlertChecks()
        activePingChecks()
        scheduledSessionChecks()
        scheduleChecks()
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
