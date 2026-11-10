import AppKit
import BaristaCore
import Combine
import ServiceManagement
import UserNotifications

enum AgentStatus { case notFound, disconnected, connected, moved }

/// Pages in System Settings that Barista can open for you.
enum SystemSettingsPane {
    case loginItems, notifications, privacySecurity

    var title: String {
        switch self {
        case .loginItems: "Login Items"
        case .notifications: "Notifications"
        case .privacySecurity: "Privacy & Security"
        }
    }
}

enum LoginStatus { case on, needsApproval, off }

@MainActor
final class AppRuntime: ObservableObject {
    let session: AwakeSession
    @Published var notice: String? { didSet { if notice == nil || notice != oldValue { noticePane = nil } } }
    /// A System Settings page that would resolve the current notice.
    @Published private(set) var noticePane: SystemSettingsPane?
    @Published private(set) var loginStatus: LoginStatus = .off
    @Published private(set) var notificationAuthorization: UNAuthorizationStatus = .notDetermined
    @Published private(set) var launchAtLogin = false
    @Published private(set) var notificationsEnabled = false
    @Published var showCountdown: Bool { didSet { defaults.set(showCountdown, forKey: "showCountdown"); onAppearanceChange?() } }
    /// Left-clicking the menu-bar cup turns keeping awake on and off instead of opening the panel.
    @Published var clickToToggle: Bool { didSet { defaults.set(clickToToggle, forKey: "clickToToggle"); onAppearanceChange?() } }
    @Published var shortcutEnabled: Bool { didSet { defaults.set(shortcutEnabled, forKey: "shortcutEnabled"); configureShortcut() } }
    @Published var shortcutChoice: String { didSet { defaults.set(shortcutChoice, forKey: "shortcutChoice"); configureShortcut() } }
    @Published var agentAccess: Bool { didSet { defaults.set(agentAccess, forKey: "agentAccess"); configureAgentAccess() } }
    var onAppearanceChange: (() -> Void)?
    var onShowPanel: (() -> Void)?
    /// Fires each time the panel is about to open. The popover's view is reused, so SwiftUI's onAppear runs only once.
    let panelWillShow = PassthroughSubject<Void, Never>()
    var onShowSettings: (() -> Void)?
    var onShowUsage: ((UsageTab?) -> Void)?
    /// "Claude 24% · ChatGPT 6%": each plan's most-used current window, for the panel.
    var limitsSummary: String? {
        let parts = (usage?.limits ?? []).compactMap { plan -> String? in
            // Claude's session window is what the Claude app leads with; otherwise the most-used window.
            let session = plan.agent == .claude ? plan.windows.first { $0.id == "session" } : nil
            guard plan.available, let top = session ?? plan.windows.max(by: { $0.usedPercent < $1.usedPercent }) else { return nil }
            let name = plan.agent == .claude ? "Claude" : plan.agent == .codex ? "ChatGPT" : plan.name
            return "\(name) \(Int(top.usedPercent.rounded()))%"
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
    @Published private(set) var agentStatus: [HookAgent: AgentStatus] = [:]
    /// Every period (today through all time), from local agent logs. Computed off the main thread.
    @Published private(set) var usage: UsageReport?
    @Published private(set) var isRefreshingUsage = false
    /// A notification on Mondays with last week's active time, projects, and estimated cost.
    @Published var weeklySummary: Bool = UserDefaults.standard.object(forKey: "weeklySummary") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(weeklySummary, forKey: "weeklySummary")
            // Ask macOS once, without turning on "Notify when a session ends".
            if weeklySummary && notificationAuthorization == .notDetermined {
                Task { @MainActor in
                    _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
                    refreshPermissions()
                }
            }
        }
    }
    /// Notifications as a plan limit fills: on pace to run out early, 80%, 95%, used up, and reset.
    @Published var limitAlerts: Bool = UserDefaults.standard.object(forKey: "limitAlerts") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(limitAlerts, forKey: "limitAlerts")
            if limitAlerts && notificationAuthorization == .notDetermined {
                Task { @MainActor in
                    _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
                    refreshPermissions()
                }
            }
        }
    }
    /// Ask ChatGPT for the ChatGPT plan's limits with Codex's saved login (read-only).
    @Published var codexAccountLimits: Bool = UserDefaults.standard.object(forKey: "codexAccountLimits") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(codexAccountLimits, forKey: "codexAccountLimits")
            if codexAccountLimits {
                fetchCodexAccountLimits(force: true)
            } else {
                CodexAccount.forget()
                refreshUsage(force: true)
            }
        }
    }
    private var isFetchingCodexLimits = false
    private var lastCodexAttempt: Date?
    /// Once a day, tell baristatime.app that a copy is running, to count active users. No ID is sent
    /// (docs/ACTIVE-USERS.md).
    @Published var shareActiveCount: Bool = UserDefaults.standard.object(forKey: "shareActiveCount") as? Bool ?? true {
        didSet { UserDefaults.standard.set(shareActiveCount, forKey: "shareActiveCount") }
    }
    private var isSendingActivePing = false
    private var activePingTimer: Timer?
    private let notificationDelegate = NotificationDelegate()
    /// Ask Anthropic for the Claude plan's limits with Claude Code's saved login (read-only).
    @Published var claudeAccountLimits: Bool = UserDefaults.standard.object(forKey: "claudeAccountLimits") as? Bool ?? true {
        didSet {
            UserDefaults.standard.set(claudeAccountLimits, forKey: "claudeAccountLimits")
            if claudeAccountLimits {
                // Turning it on is an explicit choice: start over, and allow macOS's Keychain prompt once.
                claudeKeychainDenied = false
                fetchClaudeAccountLimits(interactive: true)
            } else {
                ClaudeAccount.forget()
                claudeNeedsPermission = false
                refreshUsage(force: true)
            }
        }
    }
    /// Background reads can't show the Keychain prompt; the Usage window offers "Allow access" instead.
    @Published private(set) var claudeNeedsPermission = false
    /// You denied or cancelled the Keychain prompt: no more background attempts until you allow access
    /// or turn the setting off and on.
    @Published private(set) var claudeKeychainDenied: Bool = UserDefaults.standard.bool(forKey: "claudeKeychainDenied") {
        didSet { UserDefaults.standard.set(claudeKeychainDenied, forKey: "claudeKeychainDenied") }
    }
    private var isFetchingClaudeLimits = false
    private var lastClaudeAttempt: Date?
    private var usageTimer: Timer?
    /// Apps without hooks that Barista keeps the Mac awake for while they're busy (Cowork, ChatGPT).
    private var activityTimer: Timer?
    private var activityTrackers: [String: ActivityTracker] = [:]
    private var activityLeases: [String: String] = [:]
    /// When each app's current burst of activity began: the 8-hour cap covers the burst, not each session.
    private var activityBurstStart: [String: Date] = [:]
    /// Apps whose session you stopped: they wait for a quiet spell before starting another.
    private var activityStopped: Set<String> = []
    private var isSamplingActivity = false
    func watches(_ app: WatchedApp) -> Bool { defaults.object(forKey: "watch.\(app.id)") as? Bool ?? true }
    func setWatches(_ app: WatchedApp, _ enabled: Bool) {
        defaults.set(enabled, forKey: "watch.\(app.id)")
        if !enabled, let id = activityLeases.removeValue(forKey: app.id) { session.cancel(id: id) }
        if !enabled { activityBurstStart[app.id] = nil; activityTrackers[app.id] = nil }
        objectWillChange.send()
    }
    // MARK: Schedule state

    /// Hours Barista keeps the Mac awake on its own (Settings → Schedule), by the system clock.
    @Published var schedule: AwakeSchedule {
        didSet {
            guard schedule != oldValue else { return }
            if let data = try? JSONEncoder().encode(schedule) { defaults.set(data, forKey: "schedule") }
            // New hours (or turning it back on) start fresh; changing only the display setting doesn't resume.
            if schedule.rules != oldValue.rules || schedule.enabled != oldValue.enabled { schedulePausedUntil = nil }
            applySchedule()
        }
    }
    /// You stopped scheduled hours: they stay off until this window ends (or you resume them).
    @Published private(set) var schedulePausedUntil: Date? {
        didSet { defaults.set(schedulePausedUntil, forKey: "schedulePausedUntil") }
    }
    private var scheduleTimer: Timer?
    private var scheduleObservers: [NSObjectProtocol] = []
    @Published var checksForUpdates: Bool { didSet { defaults.set(checksForUpdates, forKey: "checksForUpdates"); scheduleUpdateChecks() } }
    @Published private(set) var availableUpdate: AvailableUpdate?
    @Published private(set) var updateStatus: String?
    @Published private(set) var isCheckingForUpdates = false
    @Published private(set) var isInstallingUpdate = false
    private let defaults = UserDefaults.standard
    private let server = ControlServer()
    private var shortcut: GlobalHotKey?
    private var updateTimer: Timer?

    init() {
        let defaults = UserDefaults.standard
        session = AwakeSession(defaults: defaults)
        showCountdown = defaults.object(forKey: "showCountdown") as? Bool ?? true
        clickToToggle = defaults.bool(forKey: "clickToToggle")
        shortcutEnabled = defaults.object(forKey: "shortcutEnabled") as? Bool ?? true
        shortcutChoice = defaults.string(forKey: "shortcutChoice") ?? "controlOptionCommand"
        agentAccess = defaults.object(forKey: "agentAccess") as? Bool ?? true
        checksForUpdates = defaults.object(forKey: "checksForUpdates") as? Bool ?? true
        notificationsEnabled = defaults.bool(forKey: "notificationsEnabled")
        schedule = defaults.data(forKey: "schedule").flatMap { try? JSONDecoder().decode(AwakeSchedule.self, from: $0) } ?? AwakeSchedule()
        schedulePausedUntil = (defaults.object(forKey: "schedulePausedUntil") as? Date).flatMap { $0 > Date() ? $0 : nil }
        session.keepDisplayAwake = defaults.object(forKey: "keepDisplayAwake") as? Bool ?? true
        session.batteryThreshold = defaults.object(forKey: "batteryThreshold") as? Int ?? 20
        session.onCompletion = { [weak self] completion in
            self?.scheduleEnded(completion)
            self?.notify(completion)
        }
        launchAtLogin = loginItem.status == .enabled || SMAppService.mainApp.status == .enabled
        // Relaunched by an update: carry on with the sessions the previous version handed over.
        if let handoff = defaults.data(forKey: Self.handoffKey) {
            defaults.removeObject(forKey: Self.handoffKey)
            let count = session.restore(handoff)
            notice = count > 0
                ? "Updated to Barista \(BaristaVersion.current). \(count == 1 ? "Your session is" : "All \(count) sessions are") still running."
                : "Updated to Barista \(BaristaVersion.current)."
        }
    }
    private static let handoffKey = "updateHandoff"
    /// Called right before an update quits the app, so the new version continues these sessions.
    private func prepareForUpdateRelaunch() {
        if let data = session.handoff() { defaults.set(data, forKey: Self.handoffKey) }
        session.suspendForHandoff()
        defaults.synchronize()
        shutdown()
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
                show("Approve Barista in System Settings → General → Login Items to start it at login.", pane: .loginItems)
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
        notificationDelegate.onOpen = { [weak self] tab in
            if let tab { self?.onShowUsage?(tab) } else { self?.onShowPanel?() }
        }
        UNUserNotificationCenter.current().delegate = notificationDelegate
        enableLaunchAtLoginOnFirstInstall()
        configureShortcut()
        configureAgentAccess()
        refreshAgentStatus()
        scheduleUpdateChecks()
        refreshPermissions()
        refreshUsage()
        fetchClaudeAccountLimits()
        fetchCodexAccountLimits()
        let timer = Timer(timeInterval: 300, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.refreshUsage()
                self?.fetchClaudeAccountLimits()
                self?.fetchCodexAccountLimits()
            }
        }
        timer.tolerance = 30
        RunLoop.main.add(timer, forMode: .common)
        usageTimer = timer
        let activity = Timer(timeInterval: ActivityPolicy.sampleInterval, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.sampleActivity() }
        }
        activity.tolerance = 2
        RunLoop.main.add(activity, forMode: .common)
        activityTimer = activity
        startScheduleChecks()
        startActivePings()
    }
    /// Checks hourly whether today's (UTC) ping has been sent, so sleep and restarts neither skip nor repeat it.
    private func startActivePings() {
        let timer = Timer(timeInterval: 3600, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.sendActivePingIfDue() }
        }
        timer.tolerance = 60
        RunLoop.main.add(timer, forMode: .common)
        activePingTimer = timer
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { [weak self] in self?.sendActivePingIfDue() }
    }
    private func sendActivePingIfDue() {
        guard shareActiveCount, !isSendingActivePing else { return }
        let today = ActivePing.day(Date())
        guard let flags = ActivePing.flags(last: defaults.string(forKey: "lastActivePing"), today: today) else { return }
        isSendingActivePing = true
        Task.detached(priority: .utility) {
            let sent = await ActivePing.send(flags)
            await MainActor.run {
                self.isSendingActivePing = false
                // Only a ping that arrived is remembered; otherwise the next hourly check tries again.
                if sent { self.defaults.set(today, forKey: "lastActivePing") }
            }
        }
    }

    // MARK: Schedule

    private func startScheduleChecks() {
        applySchedule()
        let timer = Timer(timeInterval: 15, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.applySchedule() }
        }
        timer.tolerance = 2
        RunLoop.main.add(timer, forMode: .common)
        scheduleTimer = timer
        // Waking from sleep and clock or time-zone changes can move us in or out of scheduled hours at once.
        let workspace = NSWorkspace.shared.notificationCenter
        let center = NotificationCenter.default
        let apply: @Sendable (Notification) -> Void = { [weak self] _ in Task { @MainActor in self?.applySchedule() } }
        scheduleObservers = [
            workspace.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main, using: apply),
            center.addObserver(forName: .NSSystemClockDidChange, object: nil, queue: .main, using: apply),
            center.addObserver(forName: .NSSystemTimeZoneDidChange, object: nil, queue: .main, using: apply),
        ]
    }

    /// Starts, extends, or ends the scheduled session to match the schedule and the system clock.
    func applySchedule(now: Date = Date()) {
        session.refresh()
        let lease = session.sessions.first(where: \.isScheduled)
        guard let end = schedule.activeUntil(now) else {
            if let lease { session.release(id: lease.id) }
            schedulePausedUntil = nil
            return
        }
        if let paused = schedulePausedUntil {
            if now < paused {
                if let lease { session.release(id: lease.id) }
                return
            }
            schedulePausedUntil = nil
        }
        let seconds = min(AwakeSchedule.maxLease, end.timeIntervalSince(now))
        guard seconds > 0 else { return }
        if let current = lease {
            // The schedule changed, or a 24-hour lease needs topping up.
            if abs((current.expiresAt ?? .distantFuture).timeIntervalSince(now.addingTimeInterval(seconds))) > 1 {
                session.renew(id: current.id, seconds: seconds)
            }
            session.setDisplay(id: current.id, keepDisplayAwake: schedule.keepDisplayAwake)
        } else {
            // Fails only while battery protection is on (or at the session limit); the next check tries again.
            _ = try? session.acquire(title: "Scheduled hours", source: .application, duration: seconds,
                                     display: schedule.keepDisplayAwake, scheduled: true)
        }
    }
    /// Stopped by you (its row, or Stop all): stay off for the rest of the window it was in. Saved right
    /// away, so quitting or sleeping before the next check can't lose the pause or move it to a later window.
    private func scheduleEnded(_ completion: SessionCompletion) {
        guard completion.isScheduled, completion.reason == .stopped,
              let end = schedule.activeUntil(completion.endedAt) else { return }
        schedulePausedUntil = end
    }
    /// Turns scheduled hours back on after you stopped them.
    func resumeSchedule() {
        schedulePausedUntil = nil
        applySchedule()
    }
    /// What the panel says about the schedule: nil when it's off.
    var scheduleSummary: (text: String, paused: Bool)? {
        guard schedule.enabled, schedule.rules.contains(where: { !$0.days.isEmpty }) else { return nil }
        let now = Date()
        if let paused = schedulePausedUntil, paused > now {
            return (paused == .distantFuture ? "Schedule paused" : "Schedule paused until \(Self.when(paused, now: now))", true)
        }
        if session.sessions.contains(where: \.isScheduled) { return nil }
        if schedule.activeUntil(now) != nil, session.batteryBlocked { return ("Schedule waits for power", false) }
        guard let next = schedule.nextStart(after: now) else { return nil }
        return ("Next scheduled hours \(Self.when(next, now: now))", false)
    }
    /// "5:00 PM", "tomorrow at 8:00 AM", or "Mon at 8:00 AM".
    private static func when(_ date: Date, now: Date) -> String {
        let calendar = Calendar.current
        let time = date.formatted(date: .omitted, time: .shortened)
        if calendar.isDate(date, inSameDayAs: now) { return "at \(time)" }
        if calendar.isDateInTomorrow(date) { return "tomorrow at \(time)" }
        return "\(date.formatted(.dateTime.weekday(.abbreviated))) at \(time)"
    }

    /// Samples watched apps' CPU off the main thread, then starts, renews, or lets lapse their sessions.
    private func sampleActivity() {
        let apps = WatchedApp.all.filter { watches($0) }
        guard !apps.isEmpty, !isSamplingActivity else { return }
        isSamplingActivity = true
        Task.detached(priority: .utility) {
            let table = ProcessTable.snapshot()
            let trees = apps.map { app in (app, ProcessTable.tree(of: app, in: table)) }
            let now = Date()
            await MainActor.run {
                self.isSamplingActivity = false
                for (app, tree) in trees { self.update(app, root: tree.root, cpu: tree.members, now: now) }
            }
        }
    }

    private func update(_ app: WatchedApp, root: Int32?, cpu: [Int32: Double], now: Date) {
        // Turned off while this sample was being taken: never start (or keep) a session for it.
        guard watches(app) else {
            if let id = activityLeases.removeValue(forKey: app.id) { session.cancel(id: id) }
            activityTrackers[app.id] = nil
            activityBurstStart[app.id] = nil
            activityStopped.remove(app.id)
            return
        }
        var tracker = activityTrackers[app.id] ?? ActivityTracker()
        let busy = tracker.record(cpu, at: now)
        activityTrackers[app.id] = tracker
        // A burst ends after a quiet spell, or when the app isn't running.
        let quiet = root == nil || (!busy && now.timeIntervalSince(tracker.lastBusy ?? .distantPast) >= ActivityPolicy.quietBeforeRestart)
        if quiet { activityBurstStart[app.id] = nil }
        if let id = activityLeases[app.id], !session.sessions.contains(where: { $0.id == id }) {
            // Ended. If you stopped it (or battery protection did), don't restart it during this burst.
            let reason = session.history.first { $0.id == id }?.reason
            if reason != .expired && reason != .processExited { activityStopped.insert(app.id) }
            activityLeases[app.id] = nil
        }
        if activityStopped.contains(app.id) {
            if quiet { activityStopped.remove(app.id) }
            return
        }
        guard busy, let root else { return }
        let burstStart = activityBurstStart[app.id] ?? now
        activityBurstStart[app.id] = burstStart
        if let id = activityLeases[app.id] {
            session.renew(id: id, seconds: ActivityPolicy.lease)
            return
        }
        // A new session within a long burst gets only what's left of the burst's 8 hours.
        let remaining = ActivityPolicy.hardCap - now.timeIntervalSince(burstStart)
        guard remaining > 1 else {
            activityStopped.insert(app.id)
            return
        }
        if let lease = try? session.acquire(title: app.title, source: .application, duration: min(ActivityPolicy.lease, remaining),
                                            display: false, processID: root, maxDuration: remaining) {
            activityLeases[app.id] = lease.id
        }
    }
    /// Fetches Claude account limits off the main actor, then refreshes the report. Never more than one at a
    /// time. Background fetches (launch and the timer) are at least 5 minutes apart and never show the
    /// Keychain prompt; `interactive` fetches come from an explicit choice (Allow access, turning it on).
    func fetchClaudeAccountLimits(interactive: Bool = false) {
        guard claudeAccountLimits, !isFetchingClaudeLimits else { return }
        if !interactive {
            guard !claudeKeychainDenied else { return }
            if let last = lastClaudeAttempt, Date().timeIntervalSince(last) < 295 { return }
        }
        lastClaudeAttempt = Date()
        isFetchingClaudeLimits = true
        Task.detached(priority: .utility) {
            let outcome = await ClaudeAccount.refresh(interactive: interactive)
            await MainActor.run {
                self.isFetchingClaudeLimits = false
                // Turned off while asking: what was just saved is forgotten too.
                guard self.claudeAccountLimits else { return ClaudeAccount.forget() }
                switch outcome {
                case .updated:
                    self.claudeNeedsPermission = false
                    self.claudeKeychainDenied = false
                    self.refreshUsage(force: true)
                case .needsPermission: self.claudeNeedsPermission = true
                case .denied:
                    self.claudeKeychainDenied = true
                    self.claudeNeedsPermission = true
                default: break
                }
            }
        }
    }
    /// Fetches ChatGPT account limits off the main actor, then refreshes the report. Never more than one at
    /// a time, and background fetches (launch and the timer) are at least 5 minutes apart. Codex's login is
    /// a file, so there is nothing to ask permission for; without one, nothing is sent.
    func fetchCodexAccountLimits(force: Bool = false) {
        guard codexAccountLimits, !isFetchingCodexLimits else { return }
        if !force, let last = lastCodexAttempt, Date().timeIntervalSince(last) < 295 { return }
        lastCodexAttempt = Date()
        isFetchingCodexLimits = true
        Task.detached(priority: .utility) {
            let outcome = await CodexAccount.refresh()
            await MainActor.run {
                self.isFetchingCodexLimits = false
                // Turned off while asking: what was just saved is forgotten too.
                guard self.codexAccountLimits else { return CodexAccount.forget() }
                if outcome == .updated { self.refreshUsage(force: true) }
            }
        }
    }
    /// The Usage window's "Allow access": one read that may show macOS's Keychain prompt.
    func allowClaudeAccess() {
        claudeKeychainDenied = false
        fetchClaudeAccountLimits(interactive: true)
    }
    /// Re-reads agent logs in the background (skipped if a refresh ran in the last minute, unless forced).
    func refreshUsage(force: Bool = false) {
        guard !isRefreshingUsage else { return }
        if !force, let usage, Date().timeIntervalSince(usage.generatedAt) < 60 { return }
        isRefreshingUsage = true
        Task.detached(priority: .utility) {
            let report = UsageStore.report(periods: UsagePeriod.allCases)
            await MainActor.run {
                self.usage = report
                self.isRefreshingUsage = false
                self.sendWeeklySummaryIfDue(report)
                self.sendLimitAlerts(report)
            }
        }
    }
    /// Once a week, from Monday 9:00 through Tuesday: last week's totals, if there were any.
    private func sendWeeklySummaryIfDue(_ report: UsageReport, now: Date = Date()) {
        guard weeklySummary, notificationAuthorization != .denied else { return }
        var calendar = Calendar.current
        calendar.firstWeekday = 2
        let weekday = calendar.component(.weekday, from: now) // 2 = Monday
        guard weekday == 3 || (weekday == 2 && calendar.component(.hour, from: now) >= 9),
              let summary = report.lastWeekSummary(now: now, calendar: calendar) else { return }
        // The week's local Monday, so moving to another time zone doesn't make it look like a new week.
        let monday = calendar.dateComponents([.year, .month, .day], from: summary.weekStart)
        let key = String(format: "%04d-%02d-%02d", monday.year ?? 0, monday.month ?? 0, monday.day ?? 0)
        let sent = defaults.string(forKey: "weeklySummarySent")
        guard sent != key else { return }
        // 0.0.14 saved the instant of Monday midnight in whatever time zone you were in. Any time zone's
        // Monday midnight is within a day and a half of ours, and weeks are 7 days apart: same week.
        if let sent, let legacy = ISO8601DateFormatter().date(from: sent),
           abs(legacy.timeIntervalSince(summary.weekStart)) < 36 * 3600 {
            defaults.set(key, forKey: "weeklySummarySent")
            return
        }
        if notificationAuthorization == .notDetermined {
            // On by default, so nobody has been asked yet. Ask quietly: provisional notifications go
            // straight to Notification Center without a prompt, and turning on session notifications
            // later still asks for full permission.
            guard !requestingSummaryPermission else { return }
            requestingSummaryPermission = true
            Task { @MainActor in
                let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .provisional])) ?? false
                requestingSummaryPermission = false
                refreshPermissions()
                if granted, weeklySummary, defaults.string(forKey: "weeklySummarySent") != key { postWeeklySummary(summary, key: key) }
            }
            return
        }
        postWeeklySummary(summary, key: key)
    }
    private var requestingSummaryPermission = false
    /// Notifies as a plan limit fills (docs/USAGE.md, "Limit alerts"). What was already sent is remembered
    /// across launches, so each notice comes once per window.
    private func sendLimitAlerts(_ report: UsageReport, now: Date = Date()) {
        guard limitAlerts, notificationAuthorization != .denied else { return }
        var state = defaults.data(forKey: "limitAlertState").flatMap { try? JSONDecoder().decode(LimitAlertState.self, from: $0) } ?? LimitAlertState()
        let alerts = LimitAlerts.evaluate(report.limits, state: &state, now: now)
        if let saved = try? JSONEncoder().encode(state) { defaults.set(saved, forKey: "limitAlertState") }
        guard !alerts.isEmpty else { return }
        if notificationAuthorization == .notDetermined {
            // On by default, so nobody has been asked yet: ask quietly, as the weekly summary does.
            Task { @MainActor in
                let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .provisional])) ?? false
                refreshPermissions()
                if granted, limitAlerts { postLimitAlerts(alerts) }
            }
            return
        }
        postLimitAlerts(alerts)
    }
    private func postLimitAlerts(_ alerts: [LimitAlert]) {
        for alert in alerts {
            let content = UNMutableNotificationContent()
            content.title = alert.title
            content.body = alert.body
            content.userInfo = ["open": "overview"]
            UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: "limit-\(UUID().uuidString)", content: content, trigger: nil))
        }
    }
    private func postWeeklySummary(_ summary: WeeklySummary, key: String) {
        defaults.set(key, forKey: "weeklySummarySent")
        let content = UNMutableNotificationContent()
        content.title = "Your week in Barista"
        content.body = summary.text
        content.userInfo = ["open": "history"]
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: "weekly-summary", content: content, trigger: nil))
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
    /// One-click mode: a click starts keeping awake until the next click. Returns false when it couldn't start.
    func quickToggle() -> Bool {
        if session.manualSession != nil { session.stopManual(); return true }
        session.start(duration: nil)
        return session.manualSession != nil
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
            if enabled && !launchAtLogin { show("Approve Barista in System Settings → General → Login Items.", pane: .loginItems) }
            refreshPermissions()
        } catch { notice = "Couldn’t change login settings: \(error.localizedDescription). Move Barista to Applications and try again." }
    }
    /// Shows a notice with a button that opens the System Settings page that fixes it.
    func show(_ message: String, pane: SystemSettingsPane) {
        notice = message
        noticePane = pane
    }
    func openSystemSettings(_ pane: SystemSettingsPane) {
        switch pane {
        case .loginItems:
            SMAppService.openSystemSettingsLoginItems()
        case .notifications:
            let id = Bundle.main.bundleIdentifier ?? "com.connorbescos.barista"
            open("x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=\(id)",
                 fallback: "x-apple.systempreferences:com.apple.preference.notifications")
        case .privacySecurity:
            open("x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension",
                 fallback: "x-apple.systempreferences:com.apple.preference.security")
        }
    }
    private func open(_ address: String, fallback: String) {
        if let url = URL(string: address), NSWorkspace.shared.open(url) { return }
        if let url = URL(string: fallback) { NSWorkspace.shared.open(url) }
    }
    /// Re-reads login-item and notification status, e.g. after returning from System Settings.
    func refreshPermissions() {
        launchAtLogin = loginEnabled
        loginStatus = loginEnabled ? .on : (loginNeedsApproval ? .needsApproval : .off)
        // Read only the (Sendable) status on the callback's thread; older SDKs reject sending the
        // non-Sendable settings object across actors. The closure must be @Sendable: written here
        // it would otherwise be main-actor isolated, and macOS calls it on a background queue,
        // which traps at runtime (the 0.0.6 launch crash).
        UNUserNotificationCenter.current().getNotificationSettings { @Sendable [weak self] settings in
            let status = settings.authorizationStatus
            Task { @MainActor in
                guard let self else { return }
                self.notificationAuthorization = status
                if status == .denied, self.notificationsEnabled {
                    self.notificationsEnabled = false
                    self.defaults.set(false, forKey: "notificationsEnabled")
                }
            }
        }
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
                if !granted { show("Allow Barista notifications in System Settings → Notifications.", pane: .notifications) }
            } catch { show("Couldn’t enable notifications. Allow them in System Settings → Notifications.", pane: .notifications) }
            refreshPermissions()
        }
    }
    var cliPath: String { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/barista").path }
    var mcpConfiguration: String {
        let value: [String: Any] = ["mcpServers": ["barista": ["command": cliPath, "args": ["mcp"]]]]
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]) else { return "" }
        return String(decoding: data, as: UTF8.self)
    }
    func status(of agent: HookAgent) -> AgentStatus { agentStatus[agent] ?? .notFound }
    /// Adds Barista's status line to Claude Code (keeping any existing one in front of it).
    func addClaudeStatusLine() {
        do {
            try ClaudeStatusLine.install(cliPath: cliPath)
            notice = "Added to Claude Code’s status line. Your Claude limits appear here after Claude Code’s next reply."
        } catch { notice = "Couldn’t update Claude Code settings: \(error.localizedDescription)" }
        objectWillChange.send()
    }
    func refreshAgentStatus() {
        for agent in HookAgent.allCases {
            if !AgentHooks.installedCommands(agent: agent).isEmpty {
                agentStatus[agent] = AgentHooks.isFullyInstalled(cliPath: cliPath, agent: agent) ? .connected : .moved
            } else {
                agentStatus[agent] = agent.isInstalled ? .disconnected : .notFound
            }
        }
    }
    /// Agents installed on this Mac but not yet connected, unless the user waved the suggestion off.
    var agentsToSuggest: [HookAgent] {
        guard !defaults.bool(forKey: "agentSuggestionDismissed") else { return [] }
        return HookAgent.allCases.filter { status(of: $0) == .disconnected }
    }
    func connectSuggestedAgents() {
        let agents = agentsToSuggest
        for agent in agents { setConnected(agent, true) }
        if agents.count > 1 {
            let names = ListFormatter.localizedString(byJoining: agents.map(\.displayName))
            notice = agents.contains(.codex)
                ? "\(names) connected. One step left for Codex: type /hooks in Codex and trust Barista’s hooks."
                : "\(names) connected."
        }
    }
    func dismissAgentSuggestion() {
        defaults.set(true, forKey: "agentSuggestionDismissed")
        objectWillChange.send()
    }
    func setConnected(_ agent: HookAgent, _ connected: Bool) {
        do {
            if connected {
                try AgentHooks.install(cliPath: cliPath, agent: agent)
                // Claude Code reports your plan limits only to its status line.
                if agent == .claude { try ClaudeStatusLine.install(cliPath: cliPath) }
                if !agentAccess { agentAccess = true }
                switch agent {
                case .claude: notice = "Claude Code connected. New prompts keep your Mac awake until Claude finishes."
                case .codex: notice = "Codex connected. One more step: Codex runs new hooks only after you trust them. In Codex, type /hooks and trust Barista’s hooks. Until then, Codex won’t keep your Mac awake."
                case .cursor: notice = "Cursor connected. Its agent keeps your Mac awake while it works, and its turns appear under Usage."
                }
            } else {
                try AgentHooks.uninstall(agent: agent)
                if agent == .claude { try ClaudeStatusLine.uninstall() }
                notice = "\(agent.displayName) disconnected."
            }
        } catch { notice = "Couldn’t update \(agent.displayName) settings: \(error.localizedDescription)" }
        refreshAgentStatus()
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
    /// Whether clicking Update will install in place (otherwise it downloads).
    var canInstallUpdate: Bool {
        guard let update = availableUpdate, update.canInstallInPlace else { return false }
        if case .success = Updater.installLocation() { return true }
        return false
    }
    /// Downloads, verifies, installs, and relaunches. Falls back to the download page when it can't.
    func installUpdate() {
        guard let update = availableUpdate, !isInstallingUpdate else { return }
        guard canInstallUpdate else {
            if case .failure(let failure) = Updater.installLocation(), !failure.offerDownload { notice = failure.message }
            else { openUpdate() }
            return
        }
        isInstallingUpdate = true
        updateStatus = "Updating to \(update.version)…"
        Task { @MainActor in
            do {
                try await Updater.install(update) { self.prepareForUpdateRelaunch() }
            } catch {
                isInstallingUpdate = false
                updateStatus = nil
                let failure = error as? Updater.Failure
                notice = failure?.message ?? "Couldn’t install the update: \(error.localizedDescription)"
                if failure?.offerDownload ?? true { openUpdate() }
            }
        }
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
                let session = URLSession(configuration: .ephemeral)
                defer { session.finishTasksAndInvalidate() }
                let (data, response) = try await session.data(for: request)
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
        // Scheduled hours are released when you turn the schedule off or change it: nothing to announce.
        if completion.isScheduled && completion.reason == .released { return }
        let content = UNMutableNotificationContent()
        content.title = completion.reason == .batteryLow ? "Time to save some battery" : "Coffee break complete"
        content.body = completion.reason == .batteryLow ? "Awake sessions stopped at \(session.batteryThreshold)% battery. Your work is still running." : "\(completion.title) · \(completion.reason.label)."
        content.sound = .default
        let identifier = completion.reason == .batteryLow ? "battery-protection" : completion.id
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: identifier, content: content, trigger: nil))
    }
}

/// Clicking a notification: the weekly summary opens History, a limit alert opens Usage & Limits, and any
/// other (a session ended) opens the panel.
/// macOS calls these on a background queue, so nothing here is main-actor isolated (see the 0.0.6 crash).
final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    @MainActor var onOpen: ((UsageTab?) -> Void)?

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let open = response.notification.request.content.userInfo["open"] as? String
        Task { @MainActor in self.onOpen?(open == "history" ? .history : open == "overview" ? .overview : nil) }
        completionHandler()
    }
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }
}
