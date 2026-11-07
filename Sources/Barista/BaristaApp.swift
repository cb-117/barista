import AppKit
import BaristaCore
import Combine
import SwiftUI

@main
struct BaristaApp {
    @MainActor static func main() {
        let application = NSApplication.shared
        let delegate = AppDelegate()
        application.delegate = delegate
        application.setActivationPolicy(.accessory)
        withExtendedLifetime(delegate) { application.run() }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let runtime = AppRuntime()
    private var item: NSStatusItem?
    private let popover = NSPopover()
    private lazy var settings = SettingsWindowController(runtime: runtime)
    private lazy var usageWindow = UsageWindowController(runtime: runtime)
    private var observation: AnyCancellable?
    private var wakeObserver: NSObjectProtocol?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        self.item = item
        item.button?.target = self
        item.button?.action = #selector(statusItemClicked)
        item.button?.sendAction(on: [.leftMouseUp, .rightMouseUp])
        let controller = NSHostingController(rootView: CoffeePopover(runtime: runtime))
        controller.sizingOptions = [.preferredContentSize]
        popover.contentViewController = controller
        popover.behavior = .transient
        popover.animates = true
        runtime.onAppearanceChange = { [weak self] in self?.updateStatus() }
        runtime.onShowPanel = { [weak self] in self?.showPanel() }
        runtime.onShowUsage = { [weak self] tab in
            self?.popover.performClose(nil)
            self?.usageWindow.show(tab: tab)
        }
        runtime.onShowSettings = { [weak self] in
            self?.popover.performClose(nil)
            self?.settings.show()
        }
        observation = runtime.session.objectWillChange.sink { [weak self] _ in
            Task { @MainActor in self?.updateStatus() }
        }
        wakeObserver = NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.runtime.session.refresh() }
        }
        runtime.start()
        updateStatus()
        // `--usage overview|projects|history` opens Usage & Limits on that tab (handy for screenshots and support).
        if let index = CommandLine.arguments.firstIndex(of: "--usage"), index + 1 < CommandLine.arguments.count,
           let tab = UsageTab.allCases.first(where: { $0.title.lowercased() == CommandLine.arguments[index + 1] }) {
            DispatchQueue.main.async { [weak self] in self?.usageWindow.show(tab: tab) }
        }
        if !CommandLine.arguments.contains("--background") && !launchedAsLoginItem {
            DispatchQueue.main.async { [weak self] in self?.showPanel() }
        }
    }
    /// Fallback for when the app itself is the login item (the helper passes --background).
    private var launchedAsLoginItem: Bool {
        guard let event = NSAppleEventManager.shared().currentAppleEvent,
              event.eventID == AEEventID(kAEOpenApplication) else { return false }
        return event.paramDescriptor(forKeyword: AEKeyword(keyAEPropData))?.enumCodeValue == AEKeyword(keyAELaunchedAsLogInItem)
    }
    func applicationWillTerminate(_ notification: Notification) { runtime.shutdown() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool { showPanel(); return true }

    /// What the status item last showed, so unchanged updates skip rebuilding the image and text.
    private var shownStatus: [String]?
    private func updateStatus() {
        guard let button = item?.button else { return }
        let active = runtime.session.isActive
        let title = active && runtime.showCountdown ? " \(runtime.session.menuBarCountdown)" : ""
        let tip: String
        if runtime.clickToToggle {
            let state = runtime.session.manualSession != nil ? "Awake · Click to let your Mac sleep" : "Click to keep your Mac awake"
            let others = runtime.session.sessions.filter { $0.source != .manual }.map(\.title)
            tip = ([state] + others).joined(separator: " · ") + "\nOption-click for the panel. Right-click for timers."
        } else {
            tip = active ? runtime.session.sessions.map(\.title).joined(separator: " · ") : "Barista · Right-click for quick timers"
        }
        // Turning one-click mode off mid-pour settles the cup right away.
        if cupTimer != nil, !runtime.clickToToggle { setCupLevel(active ? 1 : 0) }
        let status = [active ? "1" : "0", title, tip, "\(runtime.session.sessions.count)"]
        guard status != shownStatus else { return }
        let wasActive = shownStatus.map { $0[0] == "1" }
        shownStatus = status
        // One-click mode pours the coffee in (or drains it) instead of swapping the cup.
        // Other updates (title, tooltip, session count) leave a pour in progress alone.
        if wasActive != active {
            if wasActive != nil, runtime.clickToToggle, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
                animateCup(to: active ? 1 : 0)
            } else {
                setCupLevel(active ? 1 : 0)
            }
        } else if cupTimer == nil, cupLevel != (active ? 1 : 0) {
            setCupLevel(active ? 1 : 0)
        }
        button.imagePosition = .imageLeading
        button.title = title
        button.font = .monospacedDigitSystemFont(ofSize: 12, weight: .medium)
        button.toolTip = tip
        button.setAccessibilityLabel(active ? "Barista, \(runtime.session.sessions.count) awake sessions" : "Barista, resting")
    }

    // MARK: Cup fill animation

    /// How full the menu-bar cup is drawn, from 0 (empty) to 1 (full).
    private var cupLevel: CGFloat = 0
    private var cupTimer: Timer?

    private func setCupLevel(_ level: CGFloat) {
        cupTimer?.invalidate()
        cupTimer = nil
        drawCup(level)
    }
    private func drawCup(_ level: CGFloat) {
        cupLevel = level
        item?.button?.image = CupIcon.image(level: level)
    }
    /// Raises or lowers the coffee from wherever it is now: 0.6 s to fill, a little less to drain.
    private func animateCup(to target: CGFloat) {
        cupTimer?.invalidate()
        let start = cupLevel, began = CACurrentMediaTime()
        let duration = (target > start ? 0.6 : 0.45) * abs(target - start)
        guard duration > 0 else { return setCupLevel(target) }
        let timer = Timer(timeInterval: 1.0 / 60, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                let t = min(1, (CACurrentMediaTime() - began) / duration)
                let eased = t * t * (3 - 2 * t) // ease in and out
                // Reduce Motion turned on mid-pour finishes it at once.
                if t >= 1 || NSWorkspace.shared.accessibilityDisplayShouldReduceMotion { self.setCupLevel(target) }
                else { self.drawCup(start + (target - start) * eased) }
            }
        }
        // .common keeps it running while a menu is open.
        RunLoop.main.add(timer, forMode: .common)
        cupTimer = timer
    }
    @objc private func statusItemClicked() {
        let event = NSApp.currentEvent
        if event?.type == .rightMouseUp || event?.modifierFlags.contains(.control) == true { showQuickMenu(); return }
        let option = event?.modifierFlags.contains(.option) == true
        // In one-click mode a plain click toggles and Option-click opens the panel; otherwise the reverse.
        if option != runtime.clickToToggle {
            if runtime.clickToToggle {
                popover.performClose(nil)
                // Couldn't start (battery protection, say): open the panel so the reason shows.
                if !runtime.quickToggle() { showPanel() }
            } else { runtime.toggleManual() }
            return
        }
        if popover.isShown { popover.performClose(nil) }
        else { showPanel() }
    }
    private func showQuickMenu() {
        guard let item, let button = item.button else { return }
        popover.performClose(nil)
        let session = runtime.session
        let menu = NSMenu()
        let tasks = session.sessions.filter { $0.source != .manual }
        if !tasks.isEmpty {
            menu.addItem(NSMenuItem(title: "Keeping your Mac awake", action: nil, keyEquivalent: ""))
            for lease in tasks {
                let item = NSMenuItem(title: lease.title, action: nil, keyEquivalent: "")
                let submenu = NSMenu()
                submenu.addItem(action("Stop keeping awake") { session.cancel(id: lease.id) })
                item.submenu = submenu
                menu.addItem(item)
            }
            menu.addItem(.separator())
        }
        if let manual = session.manualSession {
            let remaining = manual.expiresAt.map { _ in "\(session.menuBarCountdown) left" } ?? "Until stopped"
            menu.addItem(NSMenuItem(title: "Your timer · \(remaining)", action: nil, keyEquivalent: ""))
            if manual.expiresAt != nil { menu.addItem(action("Add 15 minutes") { session.addTime(900) }) }
            menu.addItem(action("Stop") { session.stopManual() })
        } else {
            menu.addItem(NSMenuItem(title: "Keep awake for", action: nil, keyEquivalent: ""))
            for (title, seconds) in [("15 minutes", 900.0), ("1 hour", 3600), ("4 hours", 14400)] {
                menu.addItem(action(title) { [weak self] in self?.runtime.startManual(seconds: seconds) })
            }
            menu.addItem(action("Until stopped") { [weak self] in self?.runtime.startManual(seconds: nil) })
            if session.batteryBlocked { menu.items.dropFirst().forEach { $0.isEnabled = false } }
        }
        if runtime.schedulePausedUntil != nil {
            menu.addItem(action("Resume scheduled hours") { [weak self] in self?.runtime.resumeSchedule() })
        }
        if session.sessions.count > 1 || (session.isActive && session.manualSession == nil) {
            menu.addItem(action("Stop all") { session.stop() })
        }
        menu.addItem(.separator())
        menu.addItem(action("Usage & Limits…") { [weak self] in self?.usageWindow.show(tab: .overview) })
        menu.addItem(action("Projects & History…") { [weak self] in self?.usageWindow.show(tab: .projects) })
        menu.addItem(action("Settings…", key: ",") { [weak self] in self?.settings.show() })
        let support = NSMenuItem(title: "Support Barista", action: nil, keyEquivalent: "")
        support.submenu = NSMenu()
        support.submenu?.addItem(action("Star on GitHub") { NSWorkspace.shared.open(BaristaVersion.repoPage) })
        support.submenu?.addItem(action("Buy Me a Coffee") { NSWorkspace.shared.open(BaristaVersion.supportPage) })
        menu.addItem(support)
        menu.addItem(action("Quit Barista", key: "q") { [weak self] in
            self?.runtime.shutdown()
            NSApp.terminate(nil)
        })
        menu.popUp(positioning: nil, at: NSPoint(x: 0, y: button.bounds.height + 4), in: button)
    }
    private func action(_ title: String, key: String = "", _ handler: @escaping @MainActor () -> Void) -> NSMenuItem {
        let item = ClosureMenuItem(title: title, keyEquivalent: key, handler: handler)
        return item
    }
    private func showPanel() {
        guard let button = item?.button else { return }
        runtime.session.refresh()
        runtime.panelWillShow.send()
        NSApp.activate(ignoringOtherApps: true)
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }
}

/// The menu-bar cup at any fill level. Deliberately not on the main actor: AppKit may run an image's
/// drawing handler on whatever thread renders it, and a main-actor closure there would trap.
enum CupIcon {
    /// Where the saucer stops and where the cup's rim is, as fractions of the symbol's height from the bottom.
    private static let saucerTop: CGFloat = 0.48, rimTop: CGFloat = 0.95
    /// The first part of the fill fades in the saucer and the bottom of the cup; the rest raises the coffee to the rim.
    private static let pourPhase: CGFloat = 0.25
    /// The full cup below a rising line and the empty one above it. Draining runs the same frames backwards.
    static func image(level: CGFloat) -> NSImage? {
        guard let empty = NSImage(systemSymbolName: "cup.and.saucer", accessibilityDescription: "Barista"),
              let full = NSImage(systemSymbolName: "cup.and.saucer.fill", accessibilityDescription: "Barista") else { return nil }
        if level <= 0 || level >= 1 {
            let image = level >= 1 ? full : empty
            image.isTemplate = true
            return image
        }
        let image = NSImage(size: empty.size, flipped: false) { rect in
            func draw(_ symbol: NSImage, from bottom: CGFloat, to top: CGFloat, alpha: CGFloat = 1) {
                guard top > bottom, alpha > 0 else { return }
                NSGraphicsContext.saveGraphicsState()
                NSRect(x: rect.minX, y: bottom, width: rect.width, height: top - bottom).clip()
                symbol.draw(in: rect, from: .zero, operation: .sourceOver, fraction: alpha)
                NSGraphicsContext.restoreGraphicsState()
            }
            let saucer = rect.minY + rect.height * saucerTop
            if level < pourPhase {
                let pour = level / pourPhase
                draw(full, from: rect.minY, to: saucer, alpha: pour)
                draw(empty, from: rect.minY, to: saucer, alpha: 1 - pour)
                draw(empty, from: saucer, to: rect.maxY)
            } else {
                let rise = (level - pourPhase) / (1 - pourPhase)
                let line = saucer + (rect.minY + rect.height * rimTop - saucer) * rise
                draw(full, from: rect.minY, to: line)
                draw(empty, from: line, to: rect.maxY)
            }
            return true
        }
        image.alignmentRect = empty.alignmentRect
        image.isTemplate = true
        image.accessibilityDescription = "Barista"
        return image
    }
}

@MainActor
final class ClosureMenuItem: NSMenuItem {
    private let handler: @MainActor () -> Void
    init(title: String, keyEquivalent: String, handler: @escaping @MainActor () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(run), keyEquivalent: keyEquivalent)
        target = self
    }
    required init(coder: NSCoder) { fatalError("Not used") }
    @objc private func run() { handler() }
}
