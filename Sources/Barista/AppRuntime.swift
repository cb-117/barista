import AppKit
import BaristaCore
import Combine
import ServiceManagement
import UserNotifications

enum ClaudeStatus { case notFound, disconnected, connected, moved }

@MainActor
final class AppRuntime: ObservableObject {
    let session: AwakeSession
    @Published var notice: String?
    @Published private(set) var launchAtLogin = false
    @Published private(set) var notificationsEnabled = false
    @Published var showCountdown: Bool { didSet { defaults.set(showCountdown, forKey: "showCountdown"); onAppearanceChange?() } }
    @Published var shortcutEnabled: Bool { didSet { defaults.set(shortcutEnabled, forKey: "shortcutEnabled"); configureShortcut() } }
    @Published var shortcutChoice: String { didSet { defaults.set(shortcutChoice, forKey: "shortcutChoice"); configureShortcut() } }
    @Published var agentAccess: Bool { didSet { defaults.set(agentAccess, forKey: "agentAccess"); configureAgentAccess() } }
    var onAppearanceChange: (() -> Void)?
    var onShowPanel: (() -> Void)?
    var onShowSettings: (() -> Void)?
    @Published private(set) var claudeStatus: ClaudeStatus = .notFound
    private let defaults = UserDefaults.standard
    private let server = ControlServer()
    private var shortcut: GlobalHotKey?

    init() {
        let defaults = UserDefaults.standard
        session = AwakeSession(defaults: defaults)
        showCountdown = defaults.object(forKey: "showCountdown") as? Bool ?? true
        shortcutEnabled = defaults.object(forKey: "shortcutEnabled") as? Bool ?? true
        shortcutChoice = defaults.string(forKey: "shortcutChoice") ?? "controlOptionCommand"
        agentAccess = defaults.object(forKey: "agentAccess") as? Bool ?? true
        notificationsEnabled = defaults.bool(forKey: "notificationsEnabled")
        session.keepDisplayAwake = defaults.object(forKey: "keepDisplayAwake") as? Bool ?? true
        session.batteryThreshold = defaults.object(forKey: "batteryThreshold") as? Int ?? 20
        session.onCompletion = { [weak self] completion in self?.notify(completion) }
        launchAtLogin = SMAppService.mainApp.status == .enabled
    }

    /// Turns on Launch at login the first time Barista runs from an Applications folder.
    /// Runs once; if the user later turns it off in Settings, it stays off.
    func enableLaunchAtLoginOnFirstInstall() {
        let key = "didAutoEnableLaunchAtLogin"
        guard !defaults.bool(forKey: key) else { return }
        let path = Bundle.main.bundleURL.resolvingSymlinksInPath().path
        // Skip dev builds and quarantined copies macOS runs from a temporary
        // location; registering those paths would break once the app moves.
        let applicationFolders = ["/Applications/", NSHomeDirectory() + "/Applications/"]
        guard applicationFolders.contains(where: path.hasPrefix), !path.contains("/AppTranslocation/") else { return }
        if SMAppService.mainApp.status != .enabled {
            do { try SMAppService.mainApp.register() } catch { return }
        }
        defaults.set(true, forKey: key)
        launchAtLogin = SMAppService.mainApp.status == .enabled
        if !launchAtLogin, SMAppService.mainApp.status == .requiresApproval {
            notice = "Approve Barista in System Settings → General → Login Items to start it at login."
        }
    }

    func start() {
        enableLaunchAtLoginOnFirstInstall()
        configureShortcut()
        configureAgentAccess()
        refreshClaudeStatus()
    }
    func shutdown() {
        session.stop(reason: .appQuit)
        server.stop()
        shortcut?.unregister()
    }
    func toggleManual() {
        if session.manualSession != nil { session.stopManual() }
        else {
            let saved = defaults.object(forKey: "lastSessionSeconds") as? Double ?? 1800
            session.start(duration: saved == -1 ? nil : min(86400, max(1, saved)))
        }
    }
    func startManual(seconds: TimeInterval?) {
        defaults.set(seconds ?? -1, forKey: "lastSessionSeconds")
        session.start(duration: seconds)
    }
    func watch(_ app: NSRunningApplication) {
        do {
            _ = try session.acquire(title: app.localizedName ?? "Application", source: .application,
                                    duration: 28800, display: session.keepDisplayAwake,
                                    processID: app.processIdentifier, maxDuration: 28800)
        } catch { notice = error.localizedDescription }
    }
    var runningApps: [NSRunningApplication] {
        NSWorkspace.shared.runningApplications.filter {
            $0.activationPolicy == .regular && $0.processIdentifier != ProcessInfo.processInfo.processIdentifier && !$0.isTerminated
        }.sorted { ($0.localizedName ?? "") < ($1.localizedName ?? "") }
    }
    func setBatteryThreshold(_ value: Int) {
        guard [0, 10, 15, 20, 30].contains(value) else { return }
        defaults.set(value, forKey: "batteryThreshold")
        session.batteryThreshold = value
    }
    func setLaunchAtLogin(_ enabled: Bool) {
        do {
            if enabled { try SMAppService.mainApp.register() }
            else { try SMAppService.mainApp.unregister() }
            launchAtLogin = SMAppService.mainApp.status == .enabled
            if enabled && !launchAtLogin { notice = "Approve Barista in System Settings → General → Login Items." }
        } catch { notice = "Couldn’t change login settings: \(error.localizedDescription). Move Barista to Applications and try again." }
    }
    func setNotifications(_ enabled: Bool) {
        if !enabled {
            notificationsEnabled = false
            defaults.set(false, forKey: "notificationsEnabled")
            return
        }
        Task { @MainActor in
            do {
                let granted = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
                notificationsEnabled = granted
                defaults.set(granted, forKey: "notificationsEnabled")
                if !granted { notice = "Allow Barista notifications in System Settings → Notifications." }
            } catch { notice = "Couldn’t enable notifications: \(error.localizedDescription)" }
        }
    }
    var cliPath: String { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/barista").path }
    var mcpConfiguration: String {
        let value: [String: Any] = ["mcpServers": ["barista": ["command": cliPath, "args": ["mcp"]]]]
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]) else { return "" }
        return String(decoding: data, as: UTF8.self)
    }
    func refreshClaudeStatus() {
        let installed = ClaudeHooks.installedCommands()
        if !installed.isEmpty {
            claudeStatus = installed == [ClaudeHooks.command(cliPath: cliPath)] ? .connected : .moved
        } else {
            claudeStatus = ClaudeHooks.isClaudeInstalled ? .disconnected : .notFound
        }
    }
    func setClaudeConnected(_ connected: Bool) {
        do {
            if connected {
                try ClaudeHooks.install(cliPath: cliPath)
                if !agentAccess { agentAccess = true }
                notice = "Claude Code connected. New prompts keep your Mac awake until Claude finishes."
            } else {
                try ClaudeHooks.uninstall()
                notice = "Claude Code disconnected."
            }
        } catch { notice = "Couldn’t update Claude Code settings: \(error.localizedDescription)" }
        refreshClaudeStatus()
    }
    func copy(_ text: String, message: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
        notice = message
    }
    private func configureAgentAccess() {
        if agentAccess {
            do {
                try server.start { [weak self] request in
                    await MainActor.run {
                        guard let self else { return ControlReply(ok: false, error: "Barista is shutting down.") }
                        guard self.agentAccess else { return ControlReply(ok: false, error: "Local agent access is disabled in Settings.") }
                        return self.session.handle(request)
                    }
                }
            } catch { notice = "Agent access: \(error.localizedDescription)" }
        } else {
            server.stop()
            for lease in session.sessions where lease.isTask { session.cancel(id: lease.id) }
        }
    }
    private func configureShortcut() {
        shortcut?.unregister()
        shortcut = nil
        guard shortcutEnabled else { return }
        do {
            let shortcut = GlobalHotKey { [weak self] in self?.toggleManual() }
            try shortcut.register(choice: shortcutChoice)
            self.shortcut = shortcut
        } catch { notice = error.localizedDescription }
    }
    private func notify(_ completion: SessionCompletion) {
        guard notificationsEnabled, [.expired, .released, .processExited, .batteryLow].contains(completion.reason) else { return }
        let content = UNMutableNotificationContent()
        content.title = completion.reason == .batteryLow ? "Time to save some battery" : "Coffee break complete"
        content.body = completion.reason == .batteryLow ? "Awake sessions stopped at \(session.batteryThreshold)% battery. Your work is still running." : "\(completion.title) · \(completion.reason.label)."
        content.sound = .default
        let identifier = completion.reason == .batteryLow ? "battery-protection" : completion.id
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: identifier, content: content, trigger: nil))
    }
}
