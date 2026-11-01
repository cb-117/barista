import AppKit
import BaristaCore
import SwiftUI

struct CoffeeSettingsView: View {
    @ObservedObject var runtime: AppRuntime
    @ObservedObject var session: AwakeSession

    var body: some View {
        Form {
            if let notice = runtime.notice {
                Section { NoticeBanner(text: notice) { runtime.notice = nil } }
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
                Picker("Stop sessions at", selection: Binding(get: { session.batteryThreshold }, set: { runtime.setBatteryThreshold($0) })) {
                    Text("Never").tag(0)
                    ForEach([10, 15, 20, 30], id: \.self) { Text("\($0)%").tag($0) }
                }
            } header: { Text("Battery") } footer: {
                Text("Only while on battery. Your apps and commands keep running.")
            }

            Section {
                claudeRow
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
                Text("Everything stays on this Mac. No network port or account.")
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
                    if let update = runtime.availableUpdate {
                        Button("Download \(update.version)") { runtime.openUpdate() }
                    }
                }
            } header: { Text("Updates") } footer: {
                Text("Once a day, Barista asks GitHub whether there’s a newer release. Nothing about you or your Mac is sent.")
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
                Text("Barista \(BaristaVersion.current) · Prevents idle sleep. Closing the lid or choosing Sleep still sleeps your Mac.")
            }
        }
        .formStyle(.grouped)
        .frame(width: 460, height: 680)
        .tint(CoffeeTheme.accent)
        .onAppear { runtime.refreshClaudeStatus() }
    }

    private var claudeRow: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text("Claude Code")
                Text(claudeDetail).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            switch runtime.claudeStatus {
            case .connected:
                Button("Disconnect") { runtime.setClaudeConnected(false) }
            case .moved:
                Button("Repair") { runtime.setClaudeConnected(true) }
            case .disconnected, .notFound:
                Button("Connect") { runtime.setClaudeConnected(true) }
                    .disabled(runtime.claudeStatus == .notFound)
            }
        }
    }
    private var claudeDetail: String {
        switch runtime.claudeStatus {
        case .connected: "Awake while Claude works, then lets your Mac sleep."
        case .moved: "The connection is incomplete or points to an older copy. Reconnect to repair it."
        case .disconnected: "Adds hooks to ~/.claude/settings.json. No MCP setup needed."
        case .notFound: "Claude Code isn’t installed for this user."
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
