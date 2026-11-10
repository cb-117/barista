import Foundation

/// A notification about a plan limit filling up. docs/USAGE.md ("Limit alerts") is the specification;
/// the Rust build implements the same rules.
public struct LimitAlert: Sendable, Equatable {
    public enum Kind: Sendable, Equatable {
        /// Well before 80%, but on course to run out before the reset.
        case runningHot
        /// 80 or 95.
        case used(Int)
        case reached
        /// A window that was nearly used up has started again.
        case reset
    }
    public let kind: Kind
    public let title: String
    public let body: String
}

/// What has already been notified, so each notice is sent once per window.
public struct LimitAlertState: Codable, Sendable, Equatable {
    public struct Slot: Codable, Sendable, Equatable {
        /// The window's reset time (Unix seconds).
        public var resetsAt: Double
        /// The highest level notified: 0, 1 (running hot), 80, 95, or 100.
        public var level: Int
        /// The window reached 90%, so its reset is worth announcing.
        public var hot: Bool
        /// "Claude Session", for the reset notice.
        public var name: String
    }
    /// Keyed by "<agent>|<window id>".
    public var slots: [String: Slot] = [:]
    public init() {}
}

public enum LimitAlerts {
    /// The notices due now, updating `state` so none repeats.
    public static func evaluate(_ limits: [PlanLimits], state: inout LimitAlertState, now: Date = Date(),
                                time: (Date, Date) -> String = LimitAlerts.timeText) -> [LimitAlert] {
        var alerts: [LimitAlert] = []
        let nowSeconds = now.timeIntervalSince1970

        // The windows still running in this report.
        struct Running { let key: String; let name: String; let window: LimitWindow; let resets: Date; let minutes: Int }
        var running: [Running] = []
        for plan in limits where plan.available {
            // An old observation says little about now.
            guard let observed = plan.observedAt, now.timeIntervalSince(observed) <= 3600 else { continue }
            let who = plan.agent == .claude ? "Claude" : plan.agent == .codex ? "ChatGPT" : plan.name
            for window in plan.windows {
                // `reset` was worked out when the report was built; the reset may have passed since.
                guard let minutes = window.windowMinutes, minutes > 0, let resets = window.resetsAt, !window.reset, resets > now else { continue }
                let key = "\(plan.agent.rawValue)|\(window.id)"
                // One window per slot: a second with the same agent and id would fight the first over what is remembered.
                if running.contains(where: { $0.key == key }) { continue }
                running.append(Running(key: key, name: "\(who) \(window.label)", window: window, resets: resets, minutes: minutes))
            }
        }
        /// A reset time later by half a window or more is the next window. Smaller moves are the same
        /// one: reset times can differ by a second between reports.
        func isSameWindow(_ slot: LimitAlertState.Slot, _ item: Running) -> Bool {
            item.resets.timeIntervalSince1970 < slot.resetsAt + Double(item.minutes) * 30
        }

        // Windows whose reset has passed: announce the ones that were nearly used up, then forget them.
        // One the report still shows running (its reset time was corrected to a moment later) hasn't reset.
        for key in state.slots.keys.sorted() {
            guard let slot = state.slots[key], slot.resetsAt <= nowSeconds else { continue }
            if running.contains(where: { $0.key == key && isSameWindow(slot, $0) }) { continue }
            if slot.hot, nowSeconds - slot.resetsAt <= 3600 {
                alerts.append(LimitAlert(kind: .reset, title: "\(slot.name) limit reset", body: "A fresh window has started."))
            }
            state.slots[key] = nil
        }

        for item in running {
            let (key, name, window, resets) = (item.key, item.name, item.window, item.resets)
            var slot = state.slots[key] ?? LimitAlertState.Slot(resetsAt: resets.timeIntervalSince1970, level: 0, hot: false, name: name)
            if !isSameWindow(slot, item) {
                slot.level = 0
                slot.hot = false
            }
            slot.resetsAt = resets.timeIntervalSince1970
            slot.name = name

            let used = window.usedPercent
            let level: Int
            if used >= 99.5 { level = 100 }
            else if used >= 95 { level = 95 }
            else if used >= 80 { level = 80 }
            else if used >= 40, window.pace == "over", window.limitAt != nil { level = 1 }
            else { level = 0 }
            if level > slot.level {
                slot.level = level
                let percent = Int(used.rounded())
                let resetsText = time(resets, now)
                let runsOut = window.limitAt.map { time($0, now) }
                switch level {
                case 100:
                    alerts.append(LimitAlert(kind: .reached, title: "\(name) limit reached", body: "It resets \(resetsText)."))
                case 1:
                    alerts.append(LimitAlert(kind: .runningHot, title: "\(name) limit is running hot",
                                             body: "\(percent)% used. At this pace it runs out \(runsOut ?? resetsText), before it resets \(resetsText)."))
                default:
                    let body = runsOut.map { "At this pace it runs out \($0). It resets \(resetsText)." } ?? "It resets \(resetsText)."
                    alerts.append(LimitAlert(kind: .used(level), title: "\(name) limit: \(percent)% used", body: body))
                }
            }
            if used >= 90 { slot.hot = true }
            state.slots[key] = slot
        }
        return alerts
    }

    /// "at 3:10 PM" when under 24 hours away, else "on Nov 14".
    public static func timeText(_ date: Date, now: Date) -> String {
        date.timeIntervalSince(now) < 86400
            ? "at " + date.formatted(date: .omitted, time: .shortened)
            : "on " + date.formatted(.dateTime.month(.abbreviated).day())
    }
}
