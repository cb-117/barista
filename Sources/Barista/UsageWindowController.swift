import AppKit
import BaristaCore
import Combine
import SwiftUI
import UniformTypeIdentifiers
import WebKit

enum UsageTab: Int, CaseIterable {
    case overview, projects, history
    var title: String {
        switch self {
        case .overview: "Overview"
        case .projects: "Projects"
        case .history: "History"
        }
    }
    /// The report page's view for this tab.
    var pageView: String? {
        switch self {
        case .overview: nil
        case .projects: "projects"
        case .history: "history"
        }
    }
}

/// The Usage & Limits window: Overview (plan limits and each agent's usage, native), then Projects and
/// History (the shared report page, docs/USAGE.md "Visual report", fed live data). The toolbar holds the
/// tabs, the period, Export, and Share. This is a menu-bar app with no SwiftUI App scene.
@MainActor
final class UsageWindowController: NSObject, NSToolbarDelegate, NSWindowDelegate, NSSharingServicePickerToolbarItemDelegate {
    private let runtime: AppRuntime
    private var window: NSWindow?
    private var overview: NSHostingView<UsageView>?
    private var report: ReportWebView?
    private let tabs = NSSegmentedControl(labels: UsageTab.allCases.map(\.title), trackingMode: .selectOne, target: nil, action: nil)
    private let periods = NSPopUpButton(frame: .zero, pullsDown: false)
    private var tab: UsageTab = UsageTab(rawValue: UserDefaults.standard.integer(forKey: "usageTab")) ?? .overview
    private var refresh: Timer?
    private var observation: AnyCancellable?

    private static let tabsItem = NSToolbarItem.Identifier("tabs")
    private static let periodItem = NSToolbarItem.Identifier("period")
    private static let exportItem = NSToolbarItem.Identifier("export")
    private static let shareItem = NSToolbarItem.Identifier("share")

    init(runtime: AppRuntime) {
        self.runtime = runtime
        super.init()
    }

    var period: UsagePeriod {
        get { UsagePeriod(rawValue: UserDefaults.standard.string(forKey: "usagePeriod") ?? "") ?? .week }
        set {
            UserDefaults.standard.set(newValue.rawValue, forKey: "usagePeriod")
            if let index = UsagePeriod.allCases.firstIndex(of: newValue) { periods.selectItem(at: index) }
        }
    }

    func show(tab requested: UsageTab? = nil) {
        if window == nil { build() }
        if let requested { select(requested) }
        runtime.refreshUsage(force: true)
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
        startRefreshing()
    }

    private func build() {
        let overview = NSHostingView(rootView: UsageView(runtime: runtime, showTab: { [weak self] in self?.select($0) }))
        let report = ReportWebView(runtime: runtime)
        report.onPeriod = { [weak self] in self?.period = $0 }
        let container = NSView()
        for view in [overview, report as NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            container.addSubview(view)
            NSLayoutConstraint.activate([
                view.leadingAnchor.constraint(equalTo: container.leadingAnchor),
                view.trailingAnchor.constraint(equalTo: container.trailingAnchor),
                view.topAnchor.constraint(equalTo: container.topAnchor),
                view.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            ])
        }
        self.overview = overview
        self.report = report

        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 860, height: 740),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "Usage & Limits"
        window.contentView = container
        window.contentMinSize = NSSize(width: 560, height: 480)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.toolbarStyle = .unified
        window.titleVisibility = .hidden
        window.center()
        window.setFrameAutosaveName("UsageWindow")

        tabs.target = self
        tabs.action = #selector(tabChanged)
        tabs.segmentStyle = .automatic
        for (index, item) in UsageTab.allCases.enumerated() {
            tabs.setToolTip("\(item.title) (⌘\(index + 1))", forSegment: index)
        }
        periods.removeAllItems()
        periods.addItems(withTitles: UsagePeriod.allCases.map(\.label))
        periods.target = self
        periods.action = #selector(periodChanged)
        periods.toolTip = "The period every tab shows"
        period = period

        let toolbar = NSToolbar(identifier: "UsageToolbar")
        toolbar.delegate = self
        toolbar.displayMode = .iconOnly
        toolbar.allowsUserCustomization = false
        toolbar.centeredItemIdentifier = Self.tabsItem
        window.toolbar = toolbar
        self.window = window

        // ⌘1–⌘3 switch tabs while the window is key.
        NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, let window = self.window, window.isKeyWindow,
                  event.modifierFlags.intersection(.deviceIndependentFlagsMask) == .command,
                  let digit = event.charactersIgnoringModifiers.flatMap(Int.init), let tab = UsageTab(rawValue: digit - 1) else { return event }
            self.select(tab)
            return nil
        }
        observation = runtime.$usage.receive(on: RunLoop.main).sink { [weak self] usage in
            if let usage { self?.report?.send(report: usage) }
        }
        select(tab)
    }

    func select(_ tab: UsageTab) {
        self.tab = tab
        UserDefaults.standard.set(tab.rawValue, forKey: "usageTab")
        tabs.selectedSegment = tab.rawValue
        overview?.isHidden = tab != .overview
        report?.isHidden = tab == .overview
        if let view = tab.pageView { report?.show(view: view) }
        window?.makeFirstResponder(tab == .overview ? overview : report)
    }

    @objc private func tabChanged() { select(UsageTab(rawValue: tabs.selectedSegment) ?? .overview) }
    @objc private func periodChanged() {
        let chosen = UsagePeriod.allCases[max(0, periods.indexOfSelectedItem)]
        UserDefaults.standard.set(chosen.rawValue, forKey: "usagePeriod")
        report?.show(period: chosen)
    }

    /// While the window is open, keep the numbers current (the runtime skips reads less than a minute apart).
    private func startRefreshing() {
        guard refresh == nil else { return }
        let timer = Timer(timeInterval: 60, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.runtime.refreshUsage() }
        }
        timer.tolerance = 10
        RunLoop.main.add(timer, forMode: .common)
        refresh = timer
    }
    func windowWillClose(_ notification: Notification) {
        refresh?.invalidate()
        refresh = nil
    }

    // MARK: Toolbar

    func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        [.flexibleSpace, Self.tabsItem, .flexibleSpace, Self.periodItem, Self.exportItem, Self.shareItem]
    }
    func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] {
        toolbarDefaultItemIdentifiers(toolbar)
    }
    func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier identifier: NSToolbarItem.Identifier, willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
        switch identifier {
        case Self.tabsItem:
            let item = NSToolbarItem(itemIdentifier: identifier)
            item.view = tabs
            item.label = "View"
            return item
        case Self.periodItem:
            let item = NSToolbarItem(itemIdentifier: identifier)
            item.view = periods
            item.label = "Period"
            return item
        case Self.exportItem:
            let item = NSMenuToolbarItem(itemIdentifier: identifier)
            item.image = NSImage(systemSymbolName: "square.and.arrow.down", accessibilityDescription: "Export")
            item.label = "Export"
            item.toolTip = "Save the report as CSV, a web page, an image, or a PDF"
            item.showsIndicator = true
            let menu = NSMenu()
            for (title, format) in [("Save as CSV…", ExportFormat.csv), ("Save as Web Page…", .html), ("Save as Image…", .png), ("Save as PDF…", .pdf)] {
                menu.addItem(ClosureMenuItem(title: title, keyEquivalent: "") { [weak self] in self?.export(format) })
            }
            menu.addItem(.separator())
            menu.addItem(ClosureMenuItem(title: "Open in Browser", keyEquivalent: "") { [weak self] in self?.openInBrowser() })
            item.menu = menu
            return item
        case Self.shareItem:
            let item = NSSharingServicePickerToolbarItem(itemIdentifier: identifier)
            item.label = "Share"
            item.toolTip = "Share the report as a web page"
            item.delegate = self
            return item
        default:
            return nil
        }
    }

    nonisolated func items(for pickerToolbarItem: NSSharingServicePickerToolbarItem) -> [Any] {
        let files: [URL] = MainActor.assumeIsolated { webPageFile().map { [$0] } ?? [] }
        return files
    }

    // MARK: Export

    enum ExportFormat { case csv, html, png, pdf }

    private var fileStem: String {
        let day = Date().formatted(.iso8601.year().month().day())
        return "Barista usage \(period.label.lowercased()) \(day)"
    }

    /// The standalone report (every period, opening on this one), as `barista report` writes it.
    private func webPage() -> String? {
        guard let usage = runtime.usage else { return nil }
        return ReportTemplate.page(report: usage.jsonObject, initialPeriod: period.rawValue)
    }
    private func webPageFile() -> URL? {
        guard let page = webPage() else { return nil }
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("Barista", isDirectory: true)
        let url = folder.appendingPathComponent("\(fileStem).html")
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try Data(page.utf8).write(to: url, options: .atomic)
            return url
        } catch { return nil }
    }

    func export(_ format: ExportFormat) {
        switch format {
        case .csv:
            if tab == .overview { select(.projects) }
            report?.requestCSV()
        case .html:
            guard let page = webPage() else { return }
            save(name: "\(fileStem).html", type: .html) { try Data(page.utf8).write(to: $0, options: .atomic) }
        case .png, .pdf:
            if tab == .overview { select(.projects) }
            report?.capturePDF { [weak self] data in
                guard let self, let data else { return }
                if format == .pdf {
                    self.save(name: "\(self.fileStem).pdf", type: .pdf) { try data.write(to: $0, options: .atomic) }
                } else if let png = Self.png(fromPDF: data) {
                    self.save(name: "\(self.fileStem).png", type: .png) { try png.write(to: $0, options: .atomic) }
                }
            }
        }
    }

    func saveCSV(name: String, text: String) {
        save(name: name, type: .commaSeparatedText) { try Data(text.utf8).write(to: $0, options: .atomic) }
    }

    private func save(name: String, type: UTType, write: @escaping (URL) throws -> Void) {
        guard let window else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = name
        panel.allowedContentTypes = [type]
        panel.canCreateDirectories = true
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            do { try write(url) } catch {
                Task { @MainActor in NSAlert(error: error).beginSheetModal(for: window) }
            }
        }
    }

    private func openInBrowser() {
        guard let page = webPage() else { return }
        let url = ReportTemplate.defaultURL
        do {
            try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data(page.utf8).write(to: url, options: .atomic)
            NSWorkspace.shared.open(url)
        } catch {}
    }

    /// The report's single-page PDF, drawn at 2× into a PNG.
    private static func png(fromPDF data: Data) -> Data? {
        guard let pdf = NSPDFImageRep(data: data) else { return nil }
        let scale: CGFloat = 2
        let size = pdf.bounds.size
        guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size.width * scale), pixelsHigh: Int(size.height * scale),
                                            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                                            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        bitmap.size = size
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        pdf.draw(in: NSRect(origin: .zero, size: size))
        NSGraphicsContext.restoreGraphicsState()
        return bitmap.representation(using: .png, properties: [:])
    }
}

/// The shared report page in a web view. Barista sends it data, the period, and the view; the page asks
/// Barista to copy text, save a CSV, or show a project's folder. Nothing loads from the network: the page
/// is a string, its Content-Security-Policy allows no fetches, and every navigation away is cancelled.
@MainActor
final class ReportWebView: WKWebView, WKNavigationDelegate {
    private let runtime: AppRuntime
    private var ready = false
    private var lastJSON: String?
    private var view = "projects"
    private var pendingCapture: ((Data?) -> Void)?
    var onPeriod: ((UsagePeriod) -> Void)?

    init(runtime: AppRuntime) {
        self.runtime = runtime
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let controller = WKUserContentController()
        configuration.userContentController = controller
        super.init(frame: .zero, configuration: configuration)
        controller.add(MessageProxy(owner: self), name: "barista")
        navigationDelegate = self
        setValue(false, forKey: "drawsBackground") // Show the window's own background.
        allowsBackForwardNavigationGestures = false
        allowsMagnification = false
        loadHTMLString(ReportTemplate.html, baseURL: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

    // The page loads once; links, reloads, and anything else are cancelled.
    private var loaded = false
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void) {
        if !loaded, action.request.url?.absoluteString == "about:blank" {
            loaded = true
            decisionHandler(.allow)
        } else {
            decisionHandler(.cancel)
        }
    }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        ready = false
        loaded = false
        lastJSON = nil
        loadHTMLString(ReportTemplate.html, baseURL: nil)
    }

    private func post(_ message: [String: Any]) {
        guard ready, let data = try? JSONSerialization.data(withJSONObject: message, options: [.withoutEscapingSlashes]) else { return }
        // JSON is valid JavaScript; escape the two line separators JSON allows but older engines don't.
        let json = String(decoding: data, as: UTF8.self)
            .replacingOccurrences(of: "\u{2028}", with: "\\u2028").replacingOccurrences(of: "\u{2029}", with: "\\u2029")
        evaluateJavaScript("window.barista && window.barista.receive(\(json))")
    }

    func send(report usage: UsageReport) {
        guard ready else { return }
        let object = usage.jsonObject
        guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return }
        let json = String(decoding: data, as: UTF8.self)
        guard json != lastJSON else { return }
        lastJSON = json
        post(["type": "data", "report": object, "period": currentPeriod.rawValue])
    }
    private var currentPeriod: UsagePeriod {
        UsagePeriod(rawValue: UserDefaults.standard.string(forKey: "usagePeriod") ?? "") ?? .week
    }
    func show(period: UsagePeriod) { post(["type": "period", "id": period.rawValue]) }
    func show(view: String) {
        self.view = view
        post(["type": "view", "id": view])
    }
    func requestCSV() { post(["type": "export-csv"]) }

    /// The whole report (both views) as one PDF page, on a solid background.
    func capturePDF(_ completion: @escaping (Data?) -> Void) {
        guard ready else { completion(nil); return }
        post(["type": "exporting", "on": true])
        // Let the page lay out before capturing.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in
            guard let self else { return }
            self.createPDF { result in
                self.post(["type": "exporting", "on": false])
                completion(try? result.get())
            }
        }
    }

    fileprivate func receive(_ body: Any) {
        guard let message = body as? [String: Any], let command = message["cmd"] as? String else { return }
        switch command {
        case "ready":
            ready = true
            lastJSON = nil
            show(view: view)
            if let usage = runtime.usage { send(report: usage) }
        case "period":
            if let id = message["id"] as? String, let period = UsagePeriod(rawValue: id) { onPeriod?(period) }
        case "copy":
            guard let text = message["text"] as? String, !text.isEmpty, text.count <= 10_000 else { return }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(text, forType: .string)
            post(["type": "copied"])
        case "open-folder":
            // Only a folder the report itself lists, and only if it's still there.
            guard let path = message["path"] as? String, let usage = runtime.usage,
                  usage.periods.contains(where: { $0.projects.contains { $0.project == path } }) else { return }
            var isFolder: ObjCBool = false
            guard FileManager.default.fileExists(atPath: path, isDirectory: &isFolder), isFolder.boolValue else { return }
            NSWorkspace.shared.selectFile(nil, inFileViewerRootedAtPath: path)
        case "export":
            guard message["format"] as? String == "csv", let text = message["text"] as? String, text.utf8.count <= 20_000_000,
                  let controller = window?.delegate as? UsageWindowController else { return }
            let name = (message["filename"] as? String).flatMap { $0.hasSuffix(".csv") && !$0.contains("/") ? $0 : nil } ?? "Barista usage.csv"
            controller.saveCSV(name: name, text: text)
        default:
            break
        }
    }

    /// WKUserContentController keeps its handlers alive; this keeps the web view from owning itself.
    private final class MessageProxy: NSObject, WKScriptMessageHandler {
        weak var owner: ReportWebView?
        init(owner: ReportWebView) { self.owner = owner }
        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            MainActor.assumeIsolated { owner?.receive(message.body) }
        }
    }
}
