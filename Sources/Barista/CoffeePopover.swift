import AppKit
import BaristaCore
import SwiftUI

struct CoffeeTheme {
    static let accent = Color(red: 0.78, green: 0.35, blue: 0.17)
}

enum BrewDuration: String, CaseIterable {
    case quarter = "15m", hour = "1h", fourHours = "4h", indefinite = "∞"
    case custom = "custom", untilTime = "until"
    static let quick: [BrewDuration] = [.quarter, .hour, .fourHours, .indefinite]
    var quickSeconds: TimeInterval? {
        switch self {
        case .quarter: 900
        case .hour: 3600
        case .fourHours: 14400
        default: nil
        }
    }
    var accessibilityName: String {
        switch self {
        case .quarter: "15 minutes"
        case .hour: "1 hour"
        case .fourHours: "4 hours"
        case .indefinite: "Until stopped"
        case .custom: "Custom minutes"
        case .untilTime: "Until a time"
        }
    }
}

struct CoffeePopover: View {
    @ObservedObject var runtime: AppRuntime
    @ObservedObject var session: AwakeSession
    @AppStorage("brewDuration") private var duration: BrewDuration = .hour
    @AppStorage("customMinutes") private var customMinutes = 45
    @AppStorage("keepDisplayAwake") private var keepDisplayAwake = true
    @State private var customText = "45"
    @State private var until = Date().addingTimeInterval(3600)
    @State private var inputError: String?
    private let accent = CoffeeTheme.accent

    init(runtime: AppRuntime) {
        self.runtime = runtime
        session = runtime.session
    }
    private var tasks: [CoffeeLease] { session.sessions.filter { $0.source != .manual } }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header
            if let notice = runtime.notice {
                NoticeBanner(text: notice, actionTitle: runtime.noticePane.map { _ in "Open System Settings" },
                             action: { if let pane = runtime.noticePane { runtime.openSystemSettings(pane) } }) { runtime.notice = nil }
            }
            if let update = runtime.visibleUpdate { UpdateBanner(update: update, runtime: runtime) }
            if !runtime.agentsToSuggest.isEmpty { AgentSuggestionBanner(runtime: runtime) }
            if session.batteryBlocked {
                Label("Battery protection is on. Connect power to start a session.", systemImage: "battery.25")
                    .font(.system(size: 11)).foregroundStyle(accent)
                    .padding(10).frame(maxWidth: .infinity, alignment: .leading)
                    .background(accent.opacity(0.1), in: RoundedRectangle(cornerRadius: 9))
            }
            // With no timer of your own, what's keeping the Mac awake matters most: show it first.
            if session.manualSession == nil && !tasks.isEmpty { taskList }
            if let manual = session.manualSession { manualCard(manual) } else { durationPicker }
            HStack {
                Text("Keep display on").font(.system(size: 12))
                Spacer()
                Toggle("Keep display on", isOn: $keepDisplayAwake).labelsHidden()
                    .toggleStyle(.switch).controlSize(.small)
            }
            .onChange(of: keepDisplayAwake) { session.keepDisplayAwake = $0 }
            if let error = inputError ?? session.error {
                Text(error).font(.caption).foregroundStyle(.red).accessibilityLabel("Error: \(error)")
            }
            primaryButton
            if session.manualSession != nil && !tasks.isEmpty { taskList }
            if let schedule = runtime.scheduleSummary {
                HStack(spacing: 6) {
                    Label(schedule.text, systemImage: "calendar").font(.system(size: 11)).foregroundStyle(.secondary)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if schedule.paused {
                        Button("Resume") { runtime.resumeSchedule() }
                            .buttonStyle(.plain).font(.system(size: 11, weight: .semibold)).foregroundStyle(accent)
                            .help("Keep your Mac awake for the rest of these scheduled hours")
                    }
                }
            }
            if let days = runtime.usage?.dailyActiveSeconds(days: 7), days.contains(where: { $0.seconds > 0 }) {
                WeekSparkline(days: days, accent: accent) { runtime.onShowUsage?(.history) }
            }
            footer
        }
        .padding(18)
        .frame(width: 320)
        .tint(accent)
        .onAppear {
            session.refresh()
            runtime.refreshAgentStatus()
            runtime.refreshUsage()
            customMinutes = min(1440, max(1, customMinutes))
            customText = String(customMinutes)
        }
    }

    private var header: some View {
        HStack(spacing: 7) {
            Image(systemName: session.isActive ? "cup.and.saucer.fill" : "cup.and.saucer").foregroundStyle(accent)
            Text("barista").font(.system(size: 16, weight: .bold, design: .rounded))
            Spacer()
            Circle().fill(session.isActive ? Color.green : Color.secondary.opacity(0.4)).frame(width: 6, height: 6)
            Text(session.isActive ? "Awake" : "Resting").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }

    private var durationPicker: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                ForEach(BrewDuration.quick, id: \.self) { option in
                    chip(option.rawValue, selected: duration == option, label: option.accessibilityName) {
                        duration = option; inputError = nil
                    }
                }
                moreMenu
            }
            if duration == .custom {
                HStack {
                    Text("Minutes").font(.system(size: 12))
                    Spacer()
                    TextField("1–1,440", text: $customText).textFieldStyle(.roundedBorder).frame(width: 64)
                        .accessibilityLabel("Custom duration in minutes")
                        .onSubmit(startSession)
                    Stepper("", value: $customMinutes, in: CustomDuration.minutes).labelsHidden()
                }
                .onChange(of: customText) { text in
                    inputError = nil
                    if let minutes = CustomDuration.parse(text) { customMinutes = minutes }
                }
                .onChange(of: customMinutes) { value in
                    if CustomDuration.parse(customText) != value { customText = String(value) }
                }
            }
            if duration == .untilTime {
                HStack {
                    Text("Awake until").font(.system(size: 12))
                    Spacer()
                    DatePicker("End time", selection: $until, in: Date()...Date().addingTimeInterval(86400),
                               displayedComponents: [.date, .hourAndMinute])
                        .labelsHidden().datePickerStyle(.compact).controlSize(.small)
                }
            }
        }
    }

    private var moreMenu: some View {
        let selected = duration == .custom || duration == .untilTime
        return Menu {
            Button("Custom minutes…") { duration = .custom; inputError = nil }
            Button("Until a time…") { duration = .untilTime; inputError = nil }
            Divider()
            Menu("While an app runs") {
                let apps = runtime.runningApps
                if apps.isEmpty { Text("No running apps") }
                ForEach(apps, id: \.processIdentifier) { app in
                    Button(app.localizedName ?? "Application") { runtime.watch(app) }
                }
            }
            .disabled(session.batteryBlocked)
        } label: {
            Text(selected ? (duration == .custom ? "\(customMinutes)m" : "Until") : "More")
                .font(.system(size: 12, weight: selected ? .semibold : .medium))
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden)
        .tint(selected ? accent : .primary)
        .frame(maxWidth: .infinity).frame(height: 30)
        .foregroundStyle(selected ? accent : .primary)
        .background(chipBackground(selected), in: RoundedRectangle(cornerRadius: 8))
        .overlay { RoundedRectangle(cornerRadius: 8).strokeBorder(chipBorder(selected)) }
        .accessibilityLabel("More durations")
    }

    private func chip(_ title: String, selected: Bool, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(.system(size: 12, weight: selected ? .semibold : .medium))
                .frame(maxWidth: .infinity).frame(height: 30)
                .foregroundStyle(selected ? accent : .primary)
                .background(chipBackground(selected), in: RoundedRectangle(cornerRadius: 8))
                .overlay { RoundedRectangle(cornerRadius: 8).strokeBorder(chipBorder(selected)) }
                .contentShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain).accessibilityLabel(label).accessibilityAddTraits(selected ? .isSelected : [])
    }
    private func chipBackground(_ selected: Bool) -> Color { selected ? accent.opacity(0.11) : Color.primary.opacity(0.04) }
    private func chipBorder(_ selected: Bool) -> Color { selected ? accent.opacity(0.5) : Color.primary.opacity(0.06) }

    private func manualCard(_ lease: CoffeeLease) -> some View {
        VStack(spacing: 8) {
            Text(lease.expiresAt.map { AwakeSession.formatTime($0.timeIntervalSince(session.currentDate)) } ?? "∞")
                .font(.system(size: 34, weight: .light, design: .rounded)).monospacedDigit()
                .accessibilityLabel("Time remaining in your session")
            if let end = lease.expiresAt {
                ProgressView(value: session.progress)
                HStack {
                    Text("Until \(end.formatted(date: .omitted, time: .shortened))").font(.system(size: 11)).foregroundStyle(.secondary)
                    Spacer()
                    Button { session.addTime(900) } label: { Label("15 min", systemImage: "plus").font(.system(size: 11, weight: .semibold)) }
                        .buttonStyle(.plain).foregroundStyle(accent).accessibilityLabel("Add 15 minutes")
                }
            } else {
                Text("Until you stop it").font(.system(size: 11)).foregroundStyle(.secondary)
            }
        }
        .padding(14).frame(maxWidth: .infinity)
        .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 12))
    }

    private var primaryButton: some View {
        let running = session.manualSession != nil
        return Button {
            if running { session.stopManual() } else { startSession() }
        } label: {
            Label(running ? "Stop" : "Keep awake", systemImage: running ? "stop.fill" : "cup.and.saucer.fill")
                .font(.system(size: 13, weight: .semibold)).frame(maxWidth: .infinity).frame(height: 38)
                .foregroundStyle(.white).background(accent, in: RoundedRectangle(cornerRadius: 10))
                .contentShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .keyboardShortcut(.defaultAction)
        .disabled(session.batteryBlocked && !running)
    }

    private var taskList: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(session.manualSession == nil ? "Keeping your Mac awake" : "Also keeping your Mac awake").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
            ScrollView {
                VStack(spacing: 6) { ForEach(tasks) { TaskRow(lease: $0, session: session) } }
            }
            .frame(maxHeight: min(CGFloat(tasks.count) * 64, 220))
        }
    }

    private var usageButtonTitle: String {
        if let limits = runtime.limitsSummary { return limits }
        if let today = runtime.usage?.periods.first, today.totalTokens > 0 {
            return "Today \(UsageReport.formatTokens(today.totalTokens)) tokens"
        }
        return "Usage & limits"
    }

    private var footer: some View {
        HStack(spacing: 4) {
            if session.sessions.count > 1 {
                Button("Stop all") { session.stop() }
                    .help("End every awake session. Commands and agents keep running.")
            }
            Button { runtime.onShowUsage?(.overview) } label: {
                Label(usageButtonTitle, systemImage: "chart.bar").lineLimit(1)
            }
            .help("Usage & Limits: your plan limits and each agent’s tokens. Costs are API-price estimates, not your subscription bill.")
            Spacer()
            Button { runtime.onShowSettings?() } label: { Image(systemName: "gearshape") }
                .help("Settings").accessibilityLabel("Settings")
                .keyboardShortcut(",", modifiers: .command)
            Button { runtime.shutdown(); NSApplication.shared.terminate(nil) } label: { Image(systemName: "power") }
                .help("Quit Barista").accessibilityLabel("Quit Barista")
        }
        .buttonStyle(.plain).font(.system(size: 12)).foregroundStyle(.secondary)
    }

    private func startSession() {
        var seconds = duration.quickSeconds
        switch duration {
        case .custom:
            guard let value = CustomDuration.parse(customText) else { inputError = "Enter whole minutes from 1 to 1,440."; return }
            customMinutes = value
            seconds = TimeInterval(value * 60)
        case .untilTime:
            seconds = until.timeIntervalSinceNow
            guard let seconds, seconds > 0, seconds <= 86400 else { inputError = "Choose a time within the next 24 hours."; return }
        default: break
        }
        inputError = nil
        runtime.startManual(seconds: seconds)
    }
}

struct NoticeBanner: View {
    let text: String
    var actionTitle: String? = nil
    var action: () -> Void = {}
    let dismiss: () -> Void
    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 6) {
                Text(text).font(.system(size: 11)).fixedSize(horizontal: false, vertical: true)
                if let actionTitle {
                    Button(actionTitle, action: action)
                        .buttonStyle(.plain).font(.system(size: 11, weight: .semibold)).foregroundStyle(CoffeeTheme.accent)
                }
            }
            Spacer(minLength: 0)
            Button(action: dismiss) { Image(systemName: "xmark.circle.fill") }
                .buttonStyle(.plain).foregroundStyle(.secondary).accessibilityLabel("Dismiss notice")
        }
        .padding(10).background(CoffeeTheme.accent.opacity(0.1), in: RoundedRectangle(cornerRadius: 9))
    }
}

struct UpdateBanner: View {
    let update: AvailableUpdate
    @ObservedObject var runtime: AppRuntime
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "arrow.down.circle.fill").foregroundStyle(CoffeeTheme.accent)
            Text(runtime.isInstallingUpdate ? "Updating to \(update.version)…" : "Barista \(update.version) is available")
                .font(.system(size: 11, weight: .medium))
            Spacer(minLength: 0)
            if runtime.isInstallingUpdate {
                ProgressView().controlSize(.small)
            } else {
                Button(runtime.canInstallUpdate ? "Update" : "Download") { runtime.installUpdate() }
                    .buttonStyle(.plain).font(.system(size: 11, weight: .semibold)).foregroundStyle(CoffeeTheme.accent)
                    .help(runtime.canInstallUpdate ? "Install the update and relaunch Barista. Your sessions stop while it restarts." : "Open the download")
                Button { runtime.dismissUpdate() } label: { Image(systemName: "xmark.circle.fill") }
                    .buttonStyle(.plain).foregroundStyle(.secondary).accessibilityLabel("Dismiss update")
            }
        }
        .padding(10).background(CoffeeTheme.accent.opacity(0.1), in: RoundedRectangle(cornerRadius: 9))
    }
}

struct AgentSuggestionBanner: View {
    @ObservedObject var runtime: AppRuntime
    var body: some View {
        let names = ListFormatter.localizedString(byJoining: runtime.agentsToSuggest.map(\.displayName))
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "sparkles").foregroundStyle(CoffeeTheme.accent)
            VStack(alignment: .leading, spacing: 6) {
                Text("Keep your Mac awake while \(names) works?").font(.system(size: 11, weight: .medium))
                    .fixedSize(horizontal: false, vertical: true)
                Button("Connect") { runtime.connectSuggestedAgents() }
                    .buttonStyle(.plain).font(.system(size: 11, weight: .semibold)).foregroundStyle(CoffeeTheme.accent)
            }
            Spacer(minLength: 0)
            Button { runtime.dismissAgentSuggestion() } label: { Image(systemName: "xmark.circle.fill") }
                .buttonStyle(.plain).foregroundStyle(.secondary).accessibilityLabel("Not now")
        }
        .padding(10).background(CoffeeTheme.accent.opacity(0.1), in: RoundedRectangle(cornerRadius: 9))
    }
}

/// The last seven days of active time; click for History.
struct WeekSparkline: View {
    let days: [(date: Date, seconds: Int)]
    let accent: Color
    let action: () -> Void

    var body: some View {
        let peak = max(1, days.map(\.seconds).max() ?? 1)
        let total = days.reduce(0) { $0 + $1.seconds }
        Button(action: action) {
            HStack(alignment: .bottom, spacing: 10) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Last 7 days").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
                    Text(UsageReport.formatDuration(total)).font(.system(size: 13, weight: .semibold)).monospacedDigit()
                }
                Spacer(minLength: 8)
                HStack(alignment: .bottom, spacing: 3) {
                    ForEach(Array(days.enumerated()), id: \.offset) { index, day in
                        RoundedRectangle(cornerRadius: 2)
                            .fill(day.seconds > 0 ? accent.opacity(index == days.count - 1 ? 1 : 0.55) : Color.primary.opacity(0.12))
                            .frame(width: 9, height: max(3, 26 * CGFloat(day.seconds) / CGFloat(peak)))
                            .help("\(day.date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day())): \(UsageReport.formatDuration(day.seconds))")
                    }
                }
                .frame(height: 26, alignment: .bottom)
                Image(systemName: "chevron.right").font(.system(size: 10, weight: .semibold)).foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 12).padding(.vertical, 9)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 10))
            .contentShape(RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
        .help("Active time per day across your projects. Click for History.")
        .accessibilityLabel("Last 7 days: \(UsageReport.formatDuration(total)) active. Open History.")
    }
}
