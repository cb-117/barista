@testable import BaristaCore
import Foundation

/// Pace and limit alerts (docs/USAGE.md). The Rust tests cover the same cases.
func limitAlertChecks() {
    let now = Date(timeIntervalSince1970: 1_800_000_000)
    func window(_ id: String = "five_hour", minutes: Int? = 300, used: Double, resetsIn: TimeInterval?, label: String? = "Session", at time: Date? = nil) -> LimitWindow {
        let time = time ?? now
        return PlanLimits.window(id: id, minutes: minutes, usedPercent: used, resetsAt: resetsIn.map { time.addingTimeInterval($0) }, now: time, label: label)
    }
    func plan(_ agent: UsageAgent = .claude, observedAgo: TimeInterval = 0, at time: Date? = nil, _ windows: [LimitWindow]) -> PlanLimits {
        PlanLimits(agent: agent, name: agent == .claude ? "Claude plan" : "ChatGPT plan", source: "test",
                   observedAt: (time ?? now).addingTimeInterval(-observedAgo), plan: nil, credits: nil, windows: windows)
    }
    func close(_ a: Double?, _ b: Double) -> Bool { a.map { abs($0 - b) < 1e-9 } ?? false }

    // Pace. Half of a 5-hour window gone with 60% used: on course for 120%, out in 100 minutes.
    let hot = window(used: 60, resetsIn: 9000)
    precondition(close(hot.elapsedPercent, 50) && close(hot.projectedPercent, 120) && hot.pace == "over", "Pace over: \(hot)")
    precondition(hot.limitAt == now.addingTimeInterval(6000), "Runs out in 100 minutes: \(String(describing: hot.limitAt))")
    let steady = window("seven_day", minutes: 10080, used: 10, resetsIn: 6 * 86400)
    precondition(close(steady.elapsedPercent, 14.3) && close(steady.projectedPercent, 70) && steady.pace == "ok" && steady.limitAt == nil, "Pace ok: \(steady)")
    let early = window(used: 5, resetsIn: 17700)
    precondition(close(early.elapsedPercent, 1.7) && early.projectedPercent == nil && early.pace == nil && early.limitAt == nil, "Too early: \(early)")
    let barely = window(used: 0.5, resetsIn: 9000)
    precondition(close(barely.elapsedPercent, 50) && barely.pace == nil, "Under 1% used: \(barely)")
    let full = window(used: 100, resetsIn: 9000)
    precondition(full.pace == "over" && close(full.projectedPercent, 200) && full.limitAt == nil, "Used up: \(full)")
    // Exactly on the line: over, but it runs out at the reset, not before.
    let even = window(used: 50, resetsIn: 9000)
    precondition(even.pace == "over" && close(even.projectedPercent, 100) && even.limitAt == nil, "On the line: \(even)")
    for none in [window(used: 60, resetsIn: 90000), window(used: 60, resetsIn: -10), window(minutes: nil, used: 60, resetsIn: 9000), window(used: 60, resetsIn: nil)] {
        precondition(none.elapsedPercent == nil && none.projectedPercent == nil && none.pace == nil && none.limitAt == nil, "No pace: \(none)")
    }

    // Alerts: each level once per window, in the spec's words.
    let time: (Date, Date) -> String = { date, from in "in \(Int(date.timeIntervalSince(from) / 60))m" }
    var state = LimitAlertState()
    func check(_ windows: [LimitWindow], _ agent: UsageAgent = .claude, observedAgo: TimeInterval = 0, at when: Date? = nil) -> [LimitAlert] {
        LimitAlerts.evaluate([plan(agent, observedAgo: observedAgo, at: when, windows)], state: &state, now: when ?? now, time: time)
    }
    precondition(check([window(used: 30, resetsIn: 15000)]).isEmpty, "Over pace but under 40% says nothing")
    precondition(check([window(used: 45, resetsIn: 6000)]).isEmpty, "45% two thirds through is on pace")
    var alerts = check([hot])
    precondition(alerts == [LimitAlert(kind: .runningHot, title: "Claude Session limit is running hot",
                                       body: "60% used. At this pace it runs out in 100m, before it resets in 150m.")], "Running hot: \(alerts)")
    precondition(check([hot]).isEmpty, "Running hot is sent once")
    alerts = check([window(used: 82, resetsIn: 9000)])
    precondition(alerts == [LimitAlert(kind: .used(80), title: "Claude Session limit: 82% used",
                                       body: "At this pace it runs out in 32m. It resets in 150m.")], "80%: \(alerts)")
    // The reset time moving by a second is the same window.
    precondition(check([window(used: 83, resetsIn: 9001)]).isEmpty && check([window(used: 83, resetsIn: 8999)]).isEmpty, "Jitter doesn't repeat a notice")
    alerts = check([window(used: 96, resetsIn: 9000)])
    precondition(alerts.map(\.kind) == [.used(95)] && alerts[0].title == "Claude Session limit: 96% used", "95%: \(alerts)")
    alerts = check([window(used: 100, resetsIn: 9000)])
    precondition(alerts == [LimitAlert(kind: .reached, title: "Claude Session limit reached", body: "It resets in 150m.")], "Reached: \(alerts)")
    precondition(check([window(used: 100, resetsIn: 9000)]).isEmpty && check([window(used: 70, resetsIn: 9000)]).isEmpty, "Nothing repeats, or goes back down")

    // The reset: announced for a window that got hot, within an hour of it.
    let after = now.addingTimeInterval(9000 + 600)
    alerts = check([window(used: 2, resetsIn: 17400, at: after)], at: after)
    precondition(alerts == [LimitAlert(kind: .reset, title: "Claude Session limit reset", body: "A fresh window has started.")], "Reset: \(alerts)")
    // The next window starts again, and a jump sends only the highest level.
    alerts = check([window(used: 97, resetsIn: 17000, at: after)], at: after)
    precondition(alerts.map(\.kind) == [.used(95)], "A jump sends one notice: \(alerts)")
    // Noticed too late (over an hour after the reset): forgotten quietly.
    let muchLater = after.addingTimeInterval(17000 + 7200)
    precondition(check([], at: muchLater).isEmpty && state.slots.isEmpty, "A reset over an hour ago is not announced: \(state)")

    // A window that never got hot resets quietly; so does an old observation.
    state = LimitAlertState()
    precondition(check([window(used: 85, resetsIn: 9000)]).count == 1, "80% once")
    precondition(check([], at: now.addingTimeInterval(9060)).isEmpty && state.slots.isEmpty, "No reset notice below 90%")
    precondition(check([window(used: 99, resetsIn: 9000)], observedAgo: 7200).isEmpty && state.slots.isEmpty, "Plans observed over an hour ago are skipped")
    // Windows without a length or a reset time, or already reset, have nothing to alert on.
    precondition(check([window(minutes: nil, used: 99, resetsIn: 9000), window(used: 99, resetsIn: nil), window(used: 99, resetsIn: -5)]).isEmpty, "No alert without a window and a reset")
    // A later window with the same id (reset time half a window or more later) starts at level 0.
    precondition(check([window(used: 85, resetsIn: 9000)]).count == 1 && check([window(used: 85, resetsIn: 9000 + 9000)]).count == 1, "Next window notifies again")
    // A report built just before the reset, looked at just after it: nothing to say, nothing remembered.
    var late = LimitAlertState()
    let stale = [plan(.claude, [window(used: 99, resetsIn: 1)])]
    precondition(LimitAlerts.evaluate(stale, state: &late, now: now.addingTimeInterval(2), time: time).isEmpty && late.slots.isEmpty, "A window whose reset has passed is skipped")
    // A token that could start another header line is never sent.
    precondition(CodexAccount.headerSafe("abc.def") && !CodexAccount.headerSafe("abc\nX-Evil: 1") && !CodexAccount.headerSafe(""), "Header-safe tokens only")
    // ChatGPT's plan, and each window, has its own slot.
    alerts = check([window("primary", minutes: 10080, used: 81, resetsIn: 86400, label: "Weekly")], .codex)
    precondition(alerts.map(\.title) == ["ChatGPT Weekly limit: 81% used"] && alerts[0].body == "It resets in 1440m.", "ChatGPT: \(alerts)")
    precondition(Set(state.slots.keys) == ["claude|five_hour", "codex|primary"], "Slots: \(state.slots.keys)")
    // A reset time corrected to a second later, seen just as the remembered one passes, is the same
    // window: no reset notice and no repeat. Its real reset is still announced.
    var edgeState = LimitAlertState()
    func edgeCheck(_ window: LimitWindow, at when: Date) -> [LimitAlert] {
        LimitAlerts.evaluate([plan(.claude, at: when, [window])], state: &edgeState, now: when, time: time)
    }
    precondition(edgeCheck(window(used: 96, resetsIn: 600), at: now).map(\.kind) == [.used(95)], "95% near the reset")
    let passing = now.addingTimeInterval(600)
    alerts = edgeCheck(window(used: 96, resetsIn: 1, at: passing), at: passing)
    precondition(alerts.isEmpty && edgeState.slots["claude|five_hour"]?.level == 95, "Jitter across the reset says nothing: \(alerts)")
    // Two plans reporting the same agent and window id: the first is used, so nothing repeats.
    var shared = edgeState
    let pair = [plan(.claude, at: passing, [window(used: 96, resetsIn: 1, at: passing)]), plan(.claude, at: passing, [window(used: 2, resetsIn: 18000, at: passing)])]
    for _ in 0..<2 {
        precondition(LimitAlerts.evaluate(pair, state: &shared, now: passing, time: time).isEmpty, "A shared slot uses its first window")
    }
    precondition(shared.slots["claude|five_hour"]?.level == 95 && shared.slots["claude|five_hour"]?.hot == true, "A shared slot keeps its level: \(shared)")
    let fresh = passing.addingTimeInterval(300)
    alerts = edgeCheck(window(used: 2, resetsIn: 17700, at: fresh), at: fresh)
    precondition(alerts.map(\.kind) == [.reset] && edgeState.slots["claude|five_hour"]?.level == 0, "The real reset is announced once: \(alerts)")
    // The state survives a relaunch.
    let saved = try! JSONEncoder().encode(state)
    precondition((try? JSONDecoder().decode(LimitAlertState.self, from: saved)) == state, "State round-trips")
    print("Passed the pace and limit alert checks: projections, each level once, jitter, resets, and stale plans.")
}
