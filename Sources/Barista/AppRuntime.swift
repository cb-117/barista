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
    @Published var checksForUpdates: Bool { didSet { defaults.set(checksForUpdates, forKey: "checksForUpdates"); scheduleUpdateChecks() } }
    @Published private(set) var availableUpdate: AvailableUpdate?
    @Published private(set) var updateStatus: String?
    @Published private(set) var isCheckingForUpdates = false
    private let defaults = UserDefaults.standard
    private let server = ControlServer()
    private var shortcut: GlobalHotKey?
    private var updateTimer: Timer?

    init() {
        let defaults = UserDefaults.standard
        session = AwakeSession(defaults: defaults)
        showCountdown = defaults.object(forKey: "showCountdown") as? Bool ?? true
        shortcutEnabled = defaults.object(forKey: "shortcutEnabled") as? Bool ?? true
        shortcutChoice = defaults.string(forKey: "shortcutChoice") ?? "controlOptionCommand"
        agentAccess = defaults.object(forKey: "agentAccess") as? Bool ?? true
        checksForUpdates = defaults.object(forKey: "checksForUpdates") as? Bool ?? true
        notificationsEnabled = defaults.bool(forKey: "notificationsEnabled")
        session.keepDisplayAwake = defaults.object(forKey: "keepDisplayAwake") as? Bool ?? true
        session.batteryThreshold = defaults.object(forKey: "batteryThreshold") as? Int ?? 20
        session.onCompletion = { [weak self] completion in self?.notify(completion) }
        launchAtLogin = loginItem.status == .enabled || SMAppService.mainApp.status == .enabled
    }

    /// Login launches go through a tiny bundled helper that opens Barista with `--background`,
    /// so it starts quietly in the menu bar instead of relying on launch metadata.
    private let loginItem = SMAppService.loginItem(identifier: "com.connorbescos.barista.launcher")
    private let loginChoiceKey = "launchAtLoginChoiceMade"

    private var loginEnabled: Bool { loginItem.status == .enabled || SMAppService.mainApp.status == .enabled }
    private var loginNeedsApproval: Bool { loginItem.status == .requiresApproval || SMAppService.mainApp.status == .requiresApproval }
    /// Prefers the helper (quiet start). Falls back to registering the app itself if macOS
    /// rejects the helper; AppDelegate then detects login launches from the launch event.
    private func registerLogin() throws {
        do {
            try loginItem.register()
            retireMainAppIfHelperEnabled()
        } catch { try SMAppService.mainApp.register() }
    }
    private func unregisterLogin() throws {
        for service in [loginItem, SMAppService.mainApp] where service.status != .notRegistered { try service.unregister() }
    }

    /// Turns on Launch at login the first time Barista runs from an Applications folder,
    /// unless the user already chose a setting. If they turn it off later, it stays off.
    func enableLaunchAtLoginOnFirstInstall() {
        defer {
            launchAtLogin = loginEnabled
            // Registered but not yet approved: nothing will launch at login until the user allows it.
            if !loginEnabled && loginNeedsApproval {
                notice = "Approve Barista in System Settings → General → Login Items to start it at login."
            }
        }
        // Earlier builds registered the app itself; move that to the helper when possible.
        if SMAppService.mainApp.status == .enabled, loginItem.status != .enabled {
            try? loginItem.register()
        }
        retireMainAppIfHelperEnabled()
        if loginEnabled { defaults.set(true, forKey: loginChoiceKey) }
        guard !defaults.bool(forKey: loginChoiceKey) else { return }
        let path = Bundle.main.bundleURL.resolvingSymlinksInPath().path
        // Skip dev builds and quarantined copies macOS runs from a temporary
        // location; registering those paths would break once the app moves.
        let applicationFolders = ["/Applications/", NSHomeDirectory() + "/Applications/"]
        guard applicationFolders.contains(where: path.hasPrefix), !path.contains("/AppTranslocation/") else { return }
        do { try registerLogin() } catch { return }
        defaults.set(true, forKey: loginChoiceKey)
    }
    /// Only drop the app's own registration once the helper is actually allowed to run;
    /// a helper awaiting approval would otherwise leave nothing launching at login.
    private func retireMainAppIfHelperEnabled() {
        if loginItem.status == .enabled, SMAppService.mainApp.status != .notRegistered {
            try? SMAppService.mainApp.unregister()
        }
    }

    func start() {
        enableLaunchAtLoginOnFirstInstall()
        configureShortcut()
        configureAgentAccess()
        refreshClaudeStatus()
        scheduleUpdateChecks()
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
            // Lasts exactly as long as the app runs; Stop and battery protection still apply.
            _ = try session.acquire(title: app.localizedName ?? "Application", source: .application,
                                    duration: nil, display: session.keepDisplayAwake,
                                    processID: app.processIdentifier)
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
            if enabled { try registerLogin() } else { try unregisterLogin() }
            defaults.set(true, forKey: loginChoiceKey)
            launchAtLogin = loginEnabled
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
            claudeStatus = ClaudeHooks.isFullyInstalled(cliPath: cliPath) ? .connected : .moved
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
        // The socket stays up even when access is off, so clients get a clear "disabled"
        // reply instead of concluding Barista isn't running and trying to relaunch it.
        do {
            try server.start { [weak self] request in
                await MainActor.run {
                    guard let self else { return ControlReply(ok: false, error: "Barista is shutting down.") }
                    guard self.agentAccess else { return ControlReply(ok: false, error: "Local agent access is disabled in Barista Settings.") }
                    return self.session.handle(request)
                }
            }
        } catch { notice = "Agent access: \(error.localizedDescription)" }
        if !agentAccess {
            session.revokeAgentTurns()
            for lease in session.sessions where lease.isTask { session.cancel(id: lease.id) }
        }
    }
    // MARK: Updates

    /// The update to show in the panel, unless the user dismissed that version.
    var visibleUpdate: AvailableUpdate? {
        guard let update = availableUpdate, defaults.string(forKey: "dismissedUpdate") != update.version else { return nil }
        return update
    }
    func dismissUpdate() {
        defaults.set(availableUpdate?.version, forKey: "dismissedUpdate")
        objectWillChange.send()
    }
    func openUpdate() {
        guard let update = availableUpdate else { return }
        NSWorkspace.shared.open(update.downloadURL ?? update.pageURL)
    }
    private func scheduleUpdateChecks() {
        updateTimer?.invalidate()
        updateTimer = nil
        guard checksForUpdates else { return }
        // Check hourly whether a day has passed, so sleep and restarts don't skip or repeat checks.
        let timer = Timer(timeInterval: 3600, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.checkForUpdatesIfDue() }
        }
        RunLoop.main.add(timer, forMode: .common)
        updateTimer = timer
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { [weak self] in self?.checkForUpdatesIfDue() }
    }
    private func checkForUpdatesIfDue() {
        guard checksForUpdates else { return }
        let last = defaults.object(forKey: "lastUpdateCheck") as? Date ?? .distantPast
        guard Date().timeIntervalSince(last) >= 86400 else { return }
        checkForUpdates(userInitiated: false)
    }
    /// Asks GitHub for the latest release. Sends no identifiers beyond a standard HTTPS request.
    func checkForUpdates(userInitiated: Bool) {
        guard !isCheckingForUpdates else { return }
        isCheckingForUpdates = true
        if userInitiated { updateStatus = "Checking…" }
        Task { @MainActor in
            defer { isCheckingForUpdates = false }
            var request = URLRequest(url: UpdateCheck.latestReleaseURL, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 15)
            request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
            request.setValue("Barista/\(BaristaVersion.current)", forHTTPHeaderField: "User-Agent")
            do {
                let (data, response) = try await URLSession(configuration: .ephemeral).data(for: request)
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                // 404 means nothing has been released yet.
                guard status == 200 || status == 404 else { throw URLError(.badServerResponse) }
                defaults.set(Date(), forKey: "lastUpdateCheck")
                availableUpdate = status == 200 ? UpdateCheck.newerRelease(in: data) : nil
                if userInitiated {
                    updateStatus = availableUpdate.map { "Barista \($0.version) is available." } ?? "You’re up to date (\(BaristaVersion.current))."
                    if availableUpdate != nil { defaults.removeObject(forKey: "dismissedUpdate") }
                }
            } catch {
                if userInitiated { updateStatus = "Couldn’t reach GitHub. Try again later." }
            }
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
