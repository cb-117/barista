import AppKit

// Registered with SMAppService.loginItem. At login, opens Barista in the background
// (with --background, without activating) and exits. Does nothing if Barista is running.
let mainBundleID = "com.connorbescos.barista"
if NSRunningApplication.runningApplications(withBundleIdentifier: mainBundleID).isEmpty {
    // …/Barista.app/Contents/Library/LoginItems/BaristaLauncher.app
    var app = Bundle.main.bundleURL
    for _ in 0..<4 { app.deleteLastPathComponent() }
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = false
    configuration.addsToRecentItems = false
    configuration.arguments = ["--background"]
    let done = DispatchSemaphore(value: 0)
    NSWorkspace.shared.openApplication(at: app, configuration: configuration) { _, _ in done.signal() }
    _ = done.wait(timeout: .now() + 30)
}
