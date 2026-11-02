import AppKit
import BaristaCore
import SwiftUI

struct CoffeeSettingsView: View {
    @ObservedObject var runtime: AppRuntime
    @ObservedObject var session: AwakeSession

    var body: some View {
        Form {
            if let notice = runtime.notice {
                Section {
                    NoticeBanner(text: notice, actionTitle: runtime.noticePane.map { _ in "Open System Settings" },
                                 action: { if let pane = runtime.noticePane { runtime.openSystemSettings(pane) } }) { runtime.notice = nil }
                }
            }
            Section {
                Toggle("Launch at login", isOn: Binding(get: { runtime.launchAtLogin }, set: { runtime.setLaunchAtLogin($0) }))
                Toggle("Notify when a session ends", isOn: Binding(get: { runtime.notificationsEnabled }, set: { runtime.setNotifications($0) }))
                Toggle("Show time remaining in the menu bar", isOn: $runtime.showCountdown)
                Toggle("Global shortcut", isOn: $runtime.shortcutEnabled)
                if runtime.shortcutEnabled {
                    Picker("Shortcut", selection: $runtime.shortcutChoice) {
                        Text("⌃⌥⌘C").tag("controlOptionCommand")
                        Text("⌃⌥C").tag("controlOption")
                        Text("⌃⇧⌘C").tag("controlShiftCommand")
                    }
                }
            } header: { Text("General") } footer: {
                Text("Option-click the menu-bar cup to toggle. Right-click it for quick timers.")
            }

            Section {
                permissionRow("Start at login", status: loginText, needsAttention: runtime.loginStatus == .needsApproval, pane: .loginItems)
                permissionRow("Notifications", status: notificationText,
                              needsAttention: runtime.notificationAuthorization == .denied, pane: .notifications)
                permissionRow("Security", status: "Where you allow Barista the first time you open it", needsAttention: false, pane: .privacySecurity)
            } header: { Text("Permissions") } footer: {
                Text("Each button opens the matching page in System Settings.")
            }

            Section {
                Picker("Stop sessions at", selection: Binding(get: { session.batteryThreshold }, set: { runtime.setBatteryThreshold($0) })) {
                    Text("Never").tag(0)
                    ForEach([10, 15, 20, 30], id: \.self) { Text("\($0)%").tag($0) }
                }
            } header: { Text("Battery") } footer: {
                Text("Only while on battery. Your apps and commands keep running.")
            }

            Section {
                agentRow(.claude)
                agentRow(.codex)
                agentRow(.cursor)
                Toggle(isOn: $runtime.claudeAccountLimits) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Show Claude plan limits from your Claude account")
                        Text("Uses Claude Code’s saved login, read-only, to ask Anthropic for your limits every few minutes. Barista never changes, stores, or shares the login.")
                            .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                }
                ForEach(WatchedApp.all.filter(\.isInstalled), id: \.id) { app in
                    Toggle(isOn: Binding(get: { runtime.watches(app) }, set: { runtime.setWatches(app, $0) })) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(app == .cowork ? "Claude app (Cowork)" : "ChatGPT app")
                            Text(app == .cowork
                                 ? "Awake while Cowork, or anything else in the Claude app, is working."
                                 : "Awake while the ChatGPT app is working, including tasks it runs on this Mac.")
                                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
                Toggle("Allow local CLI and MCP access", isOn: $runtime.agentAccess)
                HStack {
                    Button("Copy CLI path") { runtime.copy(runtime.cliPath, message: "CLI path copied. Run it with help to see commands.") }
                    Button("Copy MCP setup") { runtime.copy(runtime.mcpConfiguration, message: "MCP setup copied. Paste it into your agent’s MCP settings.") }
                    Spacer()
                    if let guide = Bundle.main.url(forResource: "AgentGuide", withExtension: "md") {
                        Button("Agent guide") { NSWorkspace.shared.open(guide) }.buttonStyle(.link)
                    }
                }
            } header: { Text("Agents") } footer: {
                Text("Everything stays on this Mac. No network port or account. Codex asks you to trust new hooks once: type /hooks in Codex. The Claude and ChatGPT apps have no hooks, so Barista watches how busy they are and keeps the Mac awake until they’ve been quiet for 5 minutes.")
            }

            Section {
                Button("Open Usage & Limits…") { runtime.onShowUsage?() }
            } header: { Text("Usage & limits") } footer: {
                Text("Your Claude and ChatGPT plan limits, and tokens per agent, in one window.")
            }
            Section {
                Toggle("Check for updates automatically", isOn: $runtime.checksForUpdates)
                HStack {
                    Button("Check now") { runtime.checkForUpdates(userInitiated: true) }
                        .disabled(runtime.isCheckingForUpdates)
                    if let status = runtime.updateStatus {
                        Text(status).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Link("What’s new", destination: URL(string: "https://github.com/cb-117/barista/blob/main/CHANGELOG.md")!)
                    if let update = runtime.availableUpdate {
                        Button(runtime.canInstallUpdate ? "Update to \(update.version)" : "Download \(update.version)") { runtime.installUpdate() }
                            .disabled(runtime.isInstallingUpdate)
                    }
                }
            } header: { Text("Updates") } footer: {
                Text("Once a day, Barista asks GitHub whether there’s a newer release. Nothing about you or your Mac is sent. Updates are signed; Barista installs them and relaunches.")
            }

            Section {
                if session.history.isEmpty {
                    Text("Nothing yet").foregroundStyle(.secondary)
                }
                ForEach(session.history.prefix(8)) { completion in
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(completion.title).lineLimit(1)
                            Text(completion.reason.label).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Text(completion.endedAt, style: .relative).font(.caption).foregroundStyle(.secondary)
                    }
                }
            } header: {
                HStack {
                    Text("Recent")
                    Spacer()
                    if !session.history.isEmpty { Button("Clear") { session.clearHistory() }.buttonStyle(.link) }
                }
            } footer: {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Barista \(BaristaVersion.current) · Running from \(Bundle.main.bundleURL.resolvingSymlinksInPath().path)")
                        .textSelection(.enabled)
                    Text("Prevents idle sleep. Closing the lid or choosing Sleep still sleeps your Mac.")
                }
            }
        }
        .formStyle(.grouped)
        .onAppear { runtime.refreshPermissions() }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in runtime.refreshPermissions() }
        .frame(width: 460, height: 760)
        .tint(CoffeeTheme.accent)
        .onAppear { runtime.refreshAgentStatus() }
    }

    private func agentRow(_ agent: HookAgent) -> some View {
        let status = runtime.status(of: agent)
        return HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(agent == .codex ? "ChatGPT Codex" : agent.displayName)
                Text(agentDetail(agent, status)).font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer()
            switch status {
            case .connected:
                Button("Disconnect") { runtime.setConnected(agent, false) }
            case .moved:
                Button("Repair") { runtime.setConnected(agent, true) }
            case .disconnected, .notFound:
                Button("Connect") { runtime.setConnected(agent, true) }
                    .disabled(status == .notFound)
            }
        }
    }
    private func agentDetail(_ agent: HookAgent, _ status: AgentStatus) -> String {
        switch (agent, status) {
        case (.claude, .connected): "Awake while Claude works, then lets your Mac sleep."
        case (.codex, .connected): "Awake while Codex works. If it isn’t, type /hooks in Codex and trust Barista’s hooks."
        case (_, .moved): "The connection is incomplete or points to an older copy. Reconnect to repair it."
        case (.claude, .disconnected): "Adds hooks to ~/.claude/settings.json. No MCP setup needed."
        case (.codex, .disconnected): "Adds hooks to ~/.codex/hooks.json for Codex in ChatGPT and the Codex CLI."
        case (.cursor, .connected): "Awake while Cursor’s agent works, and its turns appear under Usage."
        case (.cursor, .disconnected): "Adds hooks to ~/.cursor/hooks.json. Cursor picks them up on its own."
        case (.cursor, .notFound): "Cursor isn’t set up for this user."
        case (.claude, .notFound): "Claude Code isn’t installed for this user."
        case (.codex, .notFound): "Codex isn’t set up for this user. Open Codex in ChatGPT once, then connect."
        }
    }

    private var loginText: String {
        switch runtime.loginStatus {
        case .on: "On"
        case .needsApproval: "Needs your approval in Login Items"
        case .off: "Off"
        }
    }
    private var notificationText: String {
        switch runtime.notificationAuthorization {
        case .authorized, .provisional, .ephemeral: "Allowed"
        case .denied: "Turned off in System Settings"
        default: "Not asked yet. Turn on “Notify when a session ends” above."
        }
    }
    private func permissionRow(_ title: String, status: String, needsAttention: Bool, pane: SystemSettingsPane) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                Text(status).font(.caption).foregroundStyle(needsAttention ? CoffeeTheme.accent : .secondary)
            }
            Spacer()
            Button(pane.title) { runtime.openSystemSettings(pane) }
        }
    }
}

/// A plain window for Settings; this is a menu-bar app with no SwiftUI App scene.
@MainActor
final class SettingsWindowController {
    private var window: NSWindow?
    private let runtime: AppRuntime
    init(runtime: AppRuntime) { self.runtime = runtime }

    func show() {
        if window == nil {
            let controller = NSHostingController(rootView: CoffeeSettingsView(runtime: runtime, session: runtime.session))
            let window = NSWindow(contentViewController: controller)
            window.title = "Barista Settings"
            window.styleMask = [.titled, .closable]
            window.isReleasedWhenClosed = false
            window.center()
            self.window = window
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}
