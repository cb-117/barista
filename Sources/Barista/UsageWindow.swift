import AppKit
import BaristaCore
import SwiftUI

/// One place for every plan's limits and every agent's usage.
struct UsageView: View {
    @ObservedObject var runtime: AppRuntime
    @AppStorage("usagePeriod") private var usagePeriod: String = UsagePeriod.week.rawValue

    var body: some View {
        Form {
            Section {
                if let limits = runtime.usage?.limits {
                    ForEach(limits, id: \.agent) { plan in limitRow(plan) }
                } else {
                    Text(runtime.isRefreshingUsage ? "Reading…" : "No data yet").foregroundStyle(.secondary)
                }
            } header: {
                HStack {
                    Text("Limits")
                    Spacer()
                    if runtime.isRefreshingUsage { ProgressView().controlSize(.small) }
                    else { Button("Refresh") { runtime.refreshUsage(force: true) }.buttonStyle(.link) }
                }
            } footer: {
                Text("As reported by the apps themselves. Claude Code, Cowork, and Claude chat share your Claude plan; Codex and the ChatGPT app share your ChatGPT plan. Claude’s limits update each time Claude Code replies.")
            }

            Section {
                Picker("Period", selection: $usagePeriod) {
                    ForEach([UsagePeriod.today, .week, .month], id: \.rawValue) { Text($0.label).tag($0.rawValue) }
                }
                .pickerStyle(.segmented).labelsHidden()
                if let period = runtime.usage?.periods.first(where: { $0.period.rawValue == usagePeriod }) {
                    ForEach(period.agents, id: \.agent) { agent in usageRow(agent) }
                } else {
                    Text(runtime.isRefreshingUsage ? "Reading agent logs…" : "No usage yet").foregroundStyle(.secondary)
                }
            } header: { Text("Usage") } footer: {
                Text("From Claude Code’s and Codex’s logs on this Mac, and Cursor’s hooks once connected (Cursor reports turns, not tokens). Costs are estimates at API list prices, not what your subscription charges. Also in Terminal: barista usage.")
            }
        }
        .formStyle(.grouped)
        .frame(width: 460, height: 640)
        .tint(CoffeeTheme.accent)
        .onAppear { runtime.refreshUsage(force: true) }
    }

    private func limitRow(_ plan: PlanLimits) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(plan.name)
                Spacer()
                Text(sourceText(plan)).font(.caption).foregroundStyle(.secondary)
            }
            if plan.available {
                ForEach(plan.windows, id: \.id) { window in
                    VStack(alignment: .leading, spacing: 2) {
                        ProgressView(value: min(100, max(0, window.usedPercent)), total: 100)
                            .tint(window.usedPercent >= 80 ? .orange : CoffeeTheme.accent)
                        HStack {
                            Text("\(window.label) · \(Int(window.usedPercent.rounded()))% used")
                            Spacer()
                            Text(resetText(window))
                        }
                        .font(.caption).foregroundStyle(.secondary)
                    }
                }
                if let details = planDetails(plan) {
                    Text(details).font(.caption).foregroundStyle(.secondary)
                }
            } else {
                Text(unavailableText(plan.agent)).font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            // Background reads never show macOS's Keychain prompt; this is the one place that asks.
            if plan.agent == .claude && runtime.claudeAccountLimits && (runtime.claudeNeedsPermission || runtime.claudeKeychainDenied) {
                Button("Allow access to your Claude account’s limits") { runtime.allowClaudeAccess() }
                    .help("Barista reads Claude Code’s saved login, read-only, to ask Anthropic for your limits. macOS asks you to allow it; choose Always Allow.")
            }
            // Upgraded from 0.0.7 with Claude Code already connected: offer the status line in one click.
            if plan.agent == .claude && HookAgent.claude.isInstalled && ClaudeStatusLine.status() == .notConnected {
                Button("Add live limits to Claude Code’s status line") { runtime.addClaudeStatusLine() }
                    .help("Claude Code shares your plan limits only with its status line. Barista adds a small one showing them; your settings are backed up first.")
            }
        }
    }

    private func sourceText(_ plan: PlanLimits) -> String {
        guard let source = plan.source, let observed = plan.observedAt else { return "" }
        return "From \(source), \(observed.formatted(.relative(presentation: .named)))"
    }
    private func resetText(_ window: LimitWindow) -> String {
        if window.reset { return "Reset" }
        guard let resets = window.resetsAt else { return "" }
        return Calendar.current.isDateInToday(resets) || resets.timeIntervalSinceNow < 86400
            ? "Resets \(resets.formatted(date: .omitted, time: .shortened))"
            : "Resets \(resets.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()))"
    }
    private func planDetails(_ plan: PlanLimits) -> String? {
        var parts: [String] = []
        if let name = plan.plan { parts.append("Plan: \(name)") }
        if let credits = plan.credits {
            if credits.unlimited { parts.append("Unlimited credits") }
            else if let balance = credits.balance, let value = Double(balance) { parts.append("\(Int(value)) credits") }
        }
        for spend in plan.windows where spend.id == "spend_limit" || spend.id == "extra_usage" {
            guard let used = spend.usedUSD, let limit = spend.limitUSD else { continue }
            let what = spend.id == "extra_usage" ? "Extra usage: " : ""
            parts.append(what + String(format: "$%.2f of $%.2f", used, limit) + (spend.period.map { " \($0)" } ?? ""))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
    private func unavailableText(_ agent: UsageAgent) -> String {
        switch agent {
        case .claude: runtime.claudeAccountLimits
            ? "No data yet. Barista reads your Claude limits from your Claude account every few minutes (and from Claude Code’s status line in a terminal)."
            : "No data yet. Turn on “Show Claude plan limits from your Claude account” in Settings → Agents, or use Claude Code in a terminal with Barista’s status line."
        case .codex: "No data yet. Appears after your next Codex or ChatGPT-app task."
        case .cursor: "Not available on this computer. Cursor doesn’t share plan limits with other apps."
        }
    }

    private func usageRow(_ agent: AgentUsage) -> some View {
        let t = agent.totals
        let active = agent.agent == .cursor ? t.turns > 0 || t.sessions > 0 : t.totalTokens > 0
        return VStack(alignment: .leading, spacing: 4) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(agent.agent.displayName)
                    Text(active ? usageDetail(agent.agent, t) : "No activity").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                if active && agent.agent != .cursor {
                    Text(UsageReport.formatCost(t.estimatedCost)).monospacedDigit()
                        .help(t.unpricedTokens > 0 ? "\(UsageReport.formatTokens(t.unpricedTokens)) tokens from models without a known price aren’t included." : "Estimated at API list prices.")
                }
            }
            if active {
                ForEach(agent.models.prefix(4), id: \.model) { model in
                    HStack {
                        Text(model.model).lineLimit(1).truncationMode(.middle)
                        Spacer()
                        Text(agent.agent == .cursor ? "\(model.turns) turn\(model.turns == 1 ? "" : "s")"
                             : "\(UsageReport.formatTokens(model.totalTokens)) · \(UsageReport.formatCost(model.estimatedCost))")
                            .monospacedDigit()
                    }
                    .font(.caption).foregroundStyle(.secondary).padding(.leading, 12)
                }
            }
        }
    }
    private func usageDetail(_ agent: UsageAgent, _ t: UsageTotals) -> String {
        let sessions = "\(t.sessions) session\(t.sessions == 1 ? "" : "s")"
        if agent == .cursor {
            return "\(sessions) · \(t.turns) turn\(t.turns == 1 ? "" : "s") · \(UsageReport.formatDuration(t.activeSeconds)) active"
        }
        return "\(sessions) · \(UsageReport.formatTokens(t.totalTokens)) tokens (\(UsageReport.formatTokens(t.cacheReadTokens)) from cache)"
    }
}

/// The Usage & Limits window; this is a menu-bar app with no SwiftUI App scene.
@MainActor
final class UsageWindowController {
    private var window: NSWindow?
    private let runtime: AppRuntime
    init(runtime: AppRuntime) { self.runtime = runtime }

    func show() {
        if window == nil {
            let window = NSWindow(contentViewController: NSHostingController(rootView: UsageView(runtime: runtime)))
            window.title = "Usage & Limits"
            window.styleMask = [.titled, .closable]
            window.isReleasedWhenClosed = false
            window.center()
            self.window = window
        }
        runtime.refreshUsage(force: true)
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}
