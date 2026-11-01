import AppKit
import BaristaCore

/// Installs a signed update in place and relaunches.
///
/// Downloads the release zip and its signature, refuses anything not signed by the release key,
/// unpacks it next to the running app, checks the bundle, and hands off to a tiny shell script
/// that waits for Barista to quit, swaps the bundles, and opens the new one. The app's path stays
/// the same, so Claude Code hooks and the login item keep working.
@MainActor
enum Updater {
    struct Failure: LocalizedError {
        let message: String
        /// Whether downloading by hand would still work.
        let offerDownload: Bool
        var errorDescription: String? { message }
    }

    /// Where the running app lives, or a reason it can't replace itself there.
    static func installLocation() -> Result<URL, Failure> {
        let bundle = Bundle.main.bundleURL.resolvingSymlinksInPath()
        if bundle.path.contains("/AppTranslocation/") || bundle.path.hasPrefix("/Volumes/") {
            return .failure(Failure(message: "Move Barista to your Applications folder, open it from there, then update.", offerDownload: false))
        }
        let manager = FileManager.default
        guard manager.isWritableFile(atPath: bundle.deletingLastPathComponent().path),
              manager.isWritableFile(atPath: bundle.path) else {
            return .failure(Failure(message: "Barista can’t replace itself in \(bundle.deletingLastPathComponent().path). Download the update instead.", offerDownload: true))
        }
        return .success(bundle)
    }

    /// Prepares the new version, then quits so the helper can swap it in. Only returns on failure.
    static func install(_ update: AvailableUpdate, beforeQuit: () -> Void) async throws {
        guard let archiveURL = update.archiveURL, let signatureURL = update.signatureURL else {
            throw Failure(message: "This release can’t be installed automatically. Download it instead.", offerDownload: true)
        }
        let target = try installLocation().get()
        let manager = FileManager.default
        let session = URLSession(configuration: .ephemeral)

        let signature = try await fetch(signatureURL, session: session, limit: 4096)
        let archive = try await fetch(archiveURL, session: session, limit: 200 * 1024 * 1024)
        guard UpdateSignature.isValid(signature: signature, for: archive) else {
            throw Failure(message: "The update’s signature didn’t check out, so Barista didn’t install it.", offerDownload: false)
        }

        // Same volume as the app, so the final swap is a rename.
        let work = try manager.url(for: .itemReplacementDirectory, in: .userDomainMask, appropriateFor: target, create: true)
        var handedOff = false
        defer { if !handedOff { try? manager.removeItem(at: work) } }
        let zip = work.appendingPathComponent("update.zip")
        try archive.write(to: zip)
        let unpacked = work.appendingPathComponent("unpacked", isDirectory: true)
        try await run("/usr/bin/ditto", ["-x", "-k", zip.path, unpacked.path], failure: "Couldn’t unpack the update.")
        let fresh = unpacked.appendingPathComponent("Barista.app", isDirectory: true)
        let info = NSDictionary(contentsOf: fresh.appendingPathComponent("Contents/Info.plist"))
        guard info?["CFBundleIdentifier"] as? String == Bundle.main.bundleIdentifier,
              info?["CFBundleShortVersionString"] as? String == update.version else {
            throw Failure(message: "The downloaded update doesn’t look like Barista \(update.version).", offerDownload: true)
        }
        try await run("/usr/bin/codesign", ["--verify", "--deep", "--strict", fresh.path], failure: "The update’s code signature is broken.")
        try? await run("/usr/bin/xattr", ["-dr", "com.apple.quarantine", fresh.path], failure: "")

        // Wait for this process to exit, swap bundles (restoring the old one if anything fails), relaunch.
        let script = """
        pid="$1"; target="$2"; fresh="$3"; work="$4"
        while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
        if mv "$target" "$work/previous.app"; then
          if ! mv "$fresh" "$target"; then mv "$work/previous.app" "$target"; fi
        fi
        open "$target"
        rm -rf "$work"
        """
        let helper = Process()
        helper.executableURL = URL(fileURLWithPath: "/bin/sh")
        helper.arguments = ["-c", script, "barista-update", String(ProcessInfo.processInfo.processIdentifier), target.path, fresh.path, work.path]
        helper.standardInput = FileHandle.nullDevice
        helper.standardOutput = FileHandle.nullDevice
        helper.standardError = FileHandle.nullDevice
        try helper.run()
        handedOff = true
        beforeQuit()
        NSApp.terminate(nil)
    }

    private static func fetch(_ url: URL, session: URLSession, limit: Int) async throws -> Data {
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 120)
        request.setValue("Barista/\(BaristaVersion.current)", forHTTPHeaderField: "User-Agent")
        let (data, response) = try await session.data(for: request)
        guard (response as? HTTPURLResponse)?.statusCode == 200, data.count <= limit else {
            throw Failure(message: "Couldn’t download the update. Try again later.", offerDownload: true)
        }
        return data
    }

    /// Runs a system tool without blocking the main thread.
    private static func run(_ tool: String, _ arguments: [String], failure: String) async throws {
        let status: Int32 = try await withCheckedThrowingContinuation { continuation in
            let process = Process()
            process.executableURL = URL(fileURLWithPath: tool)
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            process.terminationHandler = { continuation.resume(returning: $0.terminationStatus) }
            do { try process.run() } catch { continuation.resume(throwing: error) }
        }
        guard status == 0 else { throw Failure(message: failure, offerDownload: true) }
    }
}
