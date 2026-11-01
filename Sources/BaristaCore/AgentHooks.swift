import Darwin
import Foundation

/// Installs and removes Barista's entries in a Claude Code settings file.
/// Only entries whose command runs `barista … hook` are touched; everything else is preserved.
public enum ClaudeHooks {
    /// Events Barista listens to. Prompts start work, tool calls renew it, Stop/SessionEnd release it.
    public static let events = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "SessionEnd"]
    private static let toolEvents: Set<String> = ["PreToolUse", "PostToolUse"]

    public static var defaultSettingsURL: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".claude/settings.json")
    }
    public static var isClaudeInstalled: Bool {
        FileManager.default.fileExists(atPath: defaultSettingsURL.deletingLastPathComponent().path)
    }

    public static func command(cliPath: String) -> String {
        "'" + cliPath.replacingOccurrences(of: "'", with: "'\\''") + "' hook"
    }
    static func isOurs(_ command: String) -> Bool {
        command.hasSuffix("/barista' hook") || command.hasSuffix("barista hook")
    }

    /// Commands Barista has installed, or an empty array when not connected.
    public static func installedCommands(settings url: URL = defaultSettingsURL) -> [String] {
        guard let root = try? read(url), let hooks = root["hooks"] as? [String: Any] else { return [] }
        var found: [String] = []
        for event in events {
            for group in hooks[event] as? [[String: Any]] ?? [] {
                for hook in group["hooks"] as? [[String: Any]] ?? [] {
                    if let command = hook["command"] as? String, isOurs(command), !found.contains(command) { found.append(command) }
                }
            }
        }
        return found
    }

    public static func install(cliPath: String, settings url: URL = defaultSettingsURL) throws {
        var root = try read(url)
        var hooks = removingOurs(from: root["hooks"] as? [String: Any] ?? [:])
        let entry: [String: Any] = ["type": "command", "command": command(cliPath: cliPath), "timeout": 10]
        for event in events {
            var group: [String: Any] = ["hooks": [entry]]
            if toolEvents.contains(event) { group["matcher"] = "*" }
            hooks[event] = (hooks[event] as? [[String: Any]] ?? []) + [group]
        }
        root["hooks"] = hooks
        try write(root, to: url)
    }

    public static func uninstall(settings url: URL = defaultSettingsURL) throws {
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        var root = try read(url)
        guard let existing = root["hooks"] as? [String: Any] else { return }
        let hooks = removingOurs(from: existing)
        if hooks.isEmpty { root.removeValue(forKey: "hooks") } else { root["hooks"] = hooks }
        try write(root, to: url)
    }

    private static func removingOurs(from hooks: [String: Any]) -> [String: Any] {
        var result = hooks
        for (event, value) in hooks {
            guard let groups = value as? [[String: Any]] else { continue }
            let kept: [[String: Any]] = groups.compactMap { group in
                guard let entries = group["hooks"] as? [[String: Any]] else { return group }
                let remaining = entries.filter { !isOurs($0["command"] as? String ?? "") }
                if remaining.isEmpty { return nil }
                var copy = group
                copy["hooks"] = remaining
                return copy
            }
            if kept.isEmpty { result.removeValue(forKey: event) } else { result[event] = kept }
        }
        return result
    }

    private static func read(_ url: URL) throws -> [String: Any] {
        let url = url.resolvingSymlinksInPath()
        guard FileManager.default.fileExists(atPath: url.path) else { return [:] }
        let data = try Data(contentsOf: url)
        if data.allSatisfy({ [9, 10, 13, 32].contains($0) }) { return [:] }
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw CoffeeError.message("\(url.path) isn’t valid JSON, so Barista left it unchanged.")
        }
        return object
    }

    private static func write(_ root: [String: Any], to url: URL) throws {
        // Follow dotfile symlinks so the link itself survives.
        let url = url.resolvingSymlinksInPath()
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        if FileManager.default.fileExists(atPath: url.path) {
            let backup = url.appendingPathExtension("barista-backup")
            try? FileManager.default.removeItem(at: backup)
            try FileManager.default.copyItem(at: url, to: backup)
        }
        let data = try JSONSerialization.data(withJSONObject: root, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        try (data + Data([10])).write(to: url, options: .atomic)
    }
}

/// Remembers which lease belongs to which agent conversation, so separate hook processes can share it.
public struct HookLeaseStore {
    public let directory: String
    public init(socketPath: String) {
        directory = (socketPath as NSString).deletingLastPathComponent + "/hooks"
    }

    /// Accepts agent session IDs made of letters, digits, `-` and `_` only.
    public static func key(_ raw: String) -> String? {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_"))
        guard (1...128).contains(raw.count), raw.unicodeScalars.allSatisfy({ allowed.contains($0) && $0.isASCII }) else { return nil }
        return raw
    }

    /// Serializes concurrent hooks for one conversation (parallel tool calls fire hooks at once).
    public func withLock<T>(_ key: String, _ body: () throws -> T) throws -> T {
        if mkdir(directory, 0o700) != 0 && errno != EEXIST { throw CoffeeError.message("Cannot create \(directory).") }
        let fd = open(directory + "/\(key).lock", O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw CoffeeError.message("Cannot lock agent session state.") }
        defer { flock(fd, LOCK_UN); close(fd) }
        flock(fd, LOCK_EX)
        return try body()
    }
    public func lease(for key: String) -> String? {
        guard let text = try? String(contentsOfFile: directory + "/\(key)", encoding: .utf8) else { return nil }
        let id = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return UUID(uuidString: id) == nil ? nil : id
    }
    public func save(_ id: String, for key: String) {
        try? Data(id.utf8).write(to: URL(fileURLWithPath: directory + "/\(key)"), options: .atomic)
    }
    public func remove(_ key: String) {
        unlink(directory + "/\(key)")
    }
}

extension SystemState {
    /// The nearest ancestor that isn't a shell: the agent process that ran this hook.
    public static func agentProcessID(startingAt pid: Int32 = getppid()) -> Int32? {
        let shells: Set<String> = ["sh", "bash", "zsh", "dash", "fish", "env", "barista"]
        var current = pid
        for _ in 0..<8 {
            guard current > 1 else { return nil }
            var info = proc_bsdinfo()
            let size = Int32(MemoryLayout<proc_bsdinfo>.size)
            guard proc_pidinfo(current, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
            let name = withUnsafeBytes(of: info.pbi_comm) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
            if !shells.contains(name.trimmingCharacters(in: CharacterSet(charactersIn: "-"))) { return current }
            current = Int32(info.pbi_ppid)
        }
        return nil
    }
}
