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
        runtime.onShowUsage = { [weak self] in
            self?.popover.performClose(nil)
            self?.usageWindow.show()
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
        let tip = active ? runtime.session.sessions.map(\.title).joined(separator: " · ") : "Barista · Right-click for quick timers"
        let status = [active ? "1" : "0", title, tip, "\(runtime.session.sessions.count)"]
        guard status != shownStatus else { return }
        shownStatus = status
        let image = NSImage(systemSymbolName: active ? "cup.and.saucer.fill" : "cup.and.saucer", accessibilityDescription: "Barista")
        image?.isTemplate = true
        button.image = image
        button.imagePosition = .imageLeading
        button.title = title
        button.font = .monospacedDigitSystemFont(ofSize: 12, weight: .medium)
        button.toolTip = tip
        button.setAccessibilityLabel(active ? "Barista, \(runtime.session.sessions.count) awake sessions" : "Barista, resting")
    }
    @objc private func statusItemClicked() {
        let event = NSApp.currentEvent
        if event?.type == .rightMouseUp || event?.modifierFlags.contains(.control) == true { showQuickMenu(); return }
        if event?.modifierFlags.contains(.option) == true { runtime.toggleManual(); return }
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
        if session.sessions.count > 1 || (session.isActive && session.manualSession == nil) {
            menu.addItem(action("Stop all") { session.stop() })
        }
        menu.addItem(.separator())
        menu.addItem(action("Usage & Limits…") { [weak self] in self?.usageWindow.show() })
        menu.addItem(action("Usage Report…") { [weak self] in self?.runtime.openUsageReport(period: .week) })
        menu.addItem(action("Settings…", key: ",") { [weak self] in self?.settings.show() })
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
        NSApp.activate(ignoringOtherApps: true)
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        popover.contentViewController?.view.window?.makeKey()
    }
}

@MainActor
private final class ClosureMenuItem: NSMenuItem {
    private let handler: @MainActor () -> Void
    init(title: String, keyEquivalent: String, handler: @escaping @MainActor () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(run), keyEquivalent: keyEquivalent)
        target = self
    }
    required init(coder: NSCoder) { fatalError("Not used") }
    @objc private func run() { handler() }
}
