import Darwin
import Foundation

/// Coding agents Barista can connect to with lifecycle hooks.
/// Claude Code and Codex share one hook file shape: event → [{matcher?, hooks: [{type: "command", command, timeout}]}].
/// Cursor lists entries directly: {"version": 1, "hooks": {event → [{command, timeout}]}}.
public enum HookAgent: String, CaseIterable, Sendable {
    case claude, codex, cursor

    public var displayName: String {
        switch self {
        case .claude: "Claude Code"
        case .codex: "Codex"
        case .cursor: "Cursor"
        }
    }
    /// Where the agent reads user-level hooks.
    public var defaultSettingsURL: URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        switch self {
        case .claude: return home.appendingPathComponent(".claude/settings.json")
        case .codex: return home.appendingPathComponent(".codex/hooks.json")
        case .cursor: return home.appendingPathComponent(".cursor/hooks.json")
        }
    }
    /// The agent has been used by this account (its config folder exists).
    public var isInstalled: Bool { FileManager.default.fileExists(atPath: defaultSettingsURL.deletingLastPathComponent().path) }
    /// Arguments after `barista` in the hook command.
    var hookArguments: String {
        switch self {
        case .claude: "hook"
        case .codex: "hook --agent codex"
        case .cursor: "hook --agent cursor"
        }
    }
    /// Events Barista listens to. Prompts start work, tool calls renew it, stops and session ends release it.
    public var events: [String] {
        self == .cursor
            ? ["beforeSubmitPrompt", "postToolUse", "afterAgentResponse", "stop", "sessionEnd"]
            : ["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop", "SessionEnd"]
    }
    /// Codex titles each hook in its review list "N - <statusMessage>" (else just "Hook N"),
    /// and shows the message while the hook runs. Claude Code and Cursor don't use it.
    func statusMessage(for event: String) -> String? {
        guard self == .codex else { return nil }
        switch event {
        case "UserPromptSubmit": return "Barista: keep this computer awake while Codex works"
        case "PreToolUse": return "Barista: keep the computer awake during this tool call"
        case "PostToolUse": return "Barista: renew the awake session after this tool call"
        case "Stop": return "Barista: let the computer sleep now that Codex is done"
        case "SessionEnd": return "Barista: end this conversation’s awake session"
        default: return nil
        }
    }
    /// Shown by Codex for the hooks file; set only when Barista creates the file.
    static let codexFileDescription = "Barista keeps this computer awake while Codex works, then lets it sleep. https://github.com/cb-117/barista"
    func timeout(for event: String) -> Int {
        // Codex caps SessionEnd hooks at 3 seconds.
        self == .codex && event == "SessionEnd" ? 3 : 10
    }
}

/// Installs and removes Barista's entries in an agent's hook settings file.
/// Only entries whose command runs `barista … hook` are touched; everything else is preserved.
public enum AgentHooks {
    private static let toolEvents: Set<String> = ["PreToolUse", "PostToolUse"]

    public static func command(cliPath: String, agent: HookAgent = .claude) -> String {
        "'" + cliPath.replacingOccurrences(of: "'", with: "'\\''") + "' " + agent.hookArguments
    }
    /// True only for the exact shapes `command(cliPath:agent:)` generates (or a bare `…/barista hook`),
    /// so unrelated user hooks that merely end in "barista hook" are never touched. A leading `& `
    /// (PowerShell's call operator, used by the Windows build for Cursor) is allowed.
    static func isOurs(_ command: String) -> Bool {
        let command = command.hasPrefix("& ") ? String(command.dropFirst(2)) : command
        guard let suffix = HookAgent.allCases.map({ " " + $0.hookArguments }).first(where: command.hasSuffix) else { return false }
        return isBarista(String(command.dropLast(suffix.count)))
    }
    /// The executable word of a command Barista generated: a quoted or plain path to `barista`.
    static func isBarista(_ executable: String) -> Bool {
        let path: String
        if executable.hasPrefix("'") {
            // A single-quoted word where embedded quotes appear only as '\''.
            guard executable.count >= 2, executable.hasSuffix("'") else { return false }
            let body = String(executable.dropFirst().dropLast())
            let parts = body.components(separatedBy: "'\\''")
            guard !parts.contains(where: { $0.contains("'") }) else { return false }
            path = parts.joined(separator: "'")
        } else {
            let safe = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "/._-+@"))
            guard !executable.isEmpty, executable.unicodeScalars.allSatisfy({ safe.contains($0) && $0.isASCII }) else { return false }
            path = executable
        }
        return path == "barista" || (path.hasPrefix("/") && path.hasSuffix("/barista"))
    }

    /// Commands Barista has installed, or an empty array when not connected.
    public static func installedCommands(agent: HookAgent = .claude, settings: URL? = nil) -> [String] {
        let url = settings ?? agent.defaultSettingsURL
        guard let root = try? read(url), let hooks = root["hooks"] as? [String: Any] else { return [] }
        var found: [String] = []
        for event in agent.events {
            for (_, command) in ourEntries(hooks, event: event) where !found.contains(command) { found.append(command) }
        }
        return found
    }

    /// True when every event Barista needs runs exactly this CLI, with tool events matching all tools.
    public static func isFullyInstalled(cliPath: String, agent: HookAgent = .claude, settings: URL? = nil) -> Bool {
        let url = settings ?? agent.defaultSettingsURL
        guard let root = try? read(url), let hooks = root["hooks"] as? [String: Any] else { return false }
        let expected = command(cliPath: cliPath, agent: agent)
        return agent.events.allSatisfy { event in
            let entries = ourEntries(hooks, event: event)
            return !entries.isEmpty && entries.allSatisfy { group, command in
                command == expected && (agent == .cursor || !toolEvents.contains(event) || group["matcher"] as? String == "*")
                    // Hooks from 0.0.7 have no label; labels are cosmetic, so they still count as connected.
                    && ourHooks(in: group).allSatisfy { ($0["statusMessage"] as? String).map { $0 == agent.statusMessage(for: event) } ?? true }
            }
        }
    }

    private static func ourHooks(in group: [String: Any]) -> [[String: Any]] {
        (group["hooks"] as? [[String: Any]] ?? [group]).filter { isOurs($0["command"] as? String ?? "") }
    }

    private static func ourEntries(_ hooks: [String: Any], event: String) -> [(group: [String: Any], command: String)] {
        var result: [(group: [String: Any], command: String)] = []
        for group in hooks[event] as? [[String: Any]] ?? [] {
            // Cursor's entries are the hooks themselves; Claude's and Codex's wrap them in groups.
            for hook in group["hooks"] as? [[String: Any]] ?? [group] {
                if hook["type"] as? String ?? "command" == "command", let command = hook["command"] as? String, isOurs(command) {
                    result.append((group, command))
                }
            }
        }
        return result
    }

    public static func install(cliPath: String, agent: HookAgent = .claude, settings: URL? = nil) throws {
        try edit(settings ?? agent.defaultSettingsURL) { root in
            var root = root
            var hooks = removingOurs(from: root["hooks"] as? [String: Any] ?? [:])
            for event in agent.events {
                let group: [String: Any]
                if agent == .cursor {
                    group = ["command": command(cliPath: cliPath, agent: agent), "timeout": agent.timeout(for: event)]
                } else {
                    var entry: [String: Any] = ["type": "command", "command": command(cliPath: cliPath, agent: agent),
                                                "timeout": agent.timeout(for: event)]
                    if let message = agent.statusMessage(for: event) { entry["statusMessage"] = message }
                    var wrapped: [String: Any] = ["hooks": [entry]]
                    if toolEvents.contains(event) { wrapped["matcher"] = "*" }
                    group = wrapped
                }
                hooks[event] = (hooks[event] as? [[String: Any]] ?? []) + [group]
            }
            root["hooks"] = hooks
            if agent == .cursor && root["version"] == nil { root["version"] = 1 }
            if agent == .codex && root["description"] == nil && root.keys.allSatisfy({ $0 == "hooks" }) {
                root["description"] = HookAgent.codexFileDescription
            }
            return .write(root)
        }
    }

    public static func uninstall(agent: HookAgent = .claude, settings: URL? = nil) throws {
        let url = settings ?? agent.defaultSettingsURL
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        try edit(url) { root in
            var root = root
            guard let existing = root["hooks"] as? [String: Any] else { return .keep }
            let hooks = removingOurs(from: existing)
            if hooks.isEmpty { root.removeValue(forKey: "hooks") } else { root["hooks"] = hooks }
            if root["description"] as? String == HookAgent.codexFileDescription { root.removeValue(forKey: "description") }
            // A hooks file that held only Barista's entries (as Codex's usually does) goes away entirely,
            // unless it's a symlink (a dotfiles repo's file would go with it): then it's emptied instead.
            let isLink = (try? url.resourceValues(forKeys: [.isSymbolicLinkKey]))?.isSymbolicLink == true
            // Cursor's file always carries "version"; on its own it means nothing is left.
            let empty = root.keys.allSatisfy { agent == .cursor && $0 == "version" }
            return empty && agentOwnsWholeFile(url) && !isLink ? .delete : .write(root)
        }
    }

    private enum Edit { case keep, write([String: Any]), delete }
    /// The agent may save its settings while Barista edits them. If the file changed between reading
    /// and writing, the edit is redone on the new contents, so the agent's change is never lost.
    private static func edit(_ url: URL, _ change: ([String: Any]) throws -> Edit) throws {
        // Follow dotfile symlinks so the link itself survives.
        let url = url.resolvingSymlinksInPath()
        for _ in 0..<3 {
            // A missing file starts empty; any other read error (permissions, a directory) is reported.
            let original = FileManager.default.fileExists(atPath: url.path) ? try Data(contentsOf: url) : nil
            let result = try change(try parse(original, path: url.path))
            if case .keep = result { return }
            guard (try? Data(contentsOf: url)) == original else { continue }
            switch result {
            case .keep: return
            case .delete: try FileManager.default.removeItem(at: url)
            case .write(let root): try write(root, to: url)
            }
            return
        }
        throw CoffeeError.message("\(url.path) kept changing while Barista was editing it. Try again.")
    }

    /// Only dedicated hook files may be deleted; a general settings file (Claude's) is kept.
    private static func agentOwnsWholeFile(_ url: URL) -> Bool { url.lastPathComponent == "hooks.json" }

    private static func removingOurs(from hooks: [String: Any]) -> [String: Any] {
        var result = hooks
        for (event, value) in hooks {
            guard let groups = value as? [[String: Any]] else { continue }
            let kept: [[String: Any]] = groups.compactMap { group in
                guard let entries = group["hooks"] as? [[String: Any]] else {
                    // A Cursor entry: the hook itself.
                    return isOurs(group["command"] as? String ?? "") ? nil : group
                }
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
        return try parse(try Data(contentsOf: url), path: url.path)
    }
    private static func parse(_ data: Data?, path: String) throws -> [String: Any] {
        guard let data, !data.allSatisfy({ [9, 10, 13, 32].contains($0) }) else { return [:] }
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw CoffeeError.message("\(path) isn’t valid JSON, so Barista left it unchanged.")
        }
        return object
    }

    private static func write(_ root: [String: Any], to url: URL) throws {
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
        // Create the private control directory too: hooks may run before Barista ever has.
        try ControlEndpoint.ensurePrivateDirectory((directory as NSString).deletingLastPathComponent)
        try ControlEndpoint.ensurePrivateDirectory(directory)
        let fd = open(directory + "/\(key).lock", O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw CoffeeError.message("Cannot lock agent session state.") }
        defer { flock(fd, LOCK_UN); close(fd) }
        flock(fd, LOCK_EX)
        return try body()
    }
    /// The conversation's lease, when its turn began (for the hard cap), and the app's stop token then.
    public func lease(for key: String) -> (id: String, turnStarted: Date, stopToken: String?)? {
        guard let text = try? String(contentsOfFile: directory + "/\(key)", encoding: .utf8) else { return nil }
        let parts = text.split(whereSeparator: \.isWhitespace).map(String.init)
        guard let id = parts.first, UUID(uuidString: id) != nil else { return nil }
        let started = parts.count > 1 ? Double(parts[1]).map(Date.init(timeIntervalSince1970:)) : nil
        return (id, started ?? Date(), parts.count > 2 && parts[2] != "-" ? parts[2] : nil)
    }
    public func save(_ id: String, turnStarted: Date, stopToken: String?, for key: String) {
        let token = stopToken.flatMap { UUID(uuidString: $0) == nil ? nil : $0 } ?? "-"
        let text = "\(id) \(Int(turnStarted.timeIntervalSince1970)) \(token)"
        try? Data(text.utf8).write(to: URL(fileURLWithPath: directory + "/\(key)"), options: .atomic)
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

extension SystemState {
    /// Claude Code's Bash tool runs each command as `<shell> -c source …/.claude/shell-snapshots/…`.
    /// Once Claude has stopped, any of those still running are background commands it's waiting on.
    /// Long-lived children such as MCP servers don't match, so they never keep the Mac awake.
    public static func backgroundCommandPIDs(of agent: Int32, excluding excluded: Set<Int32> = []) -> [Int32] {
        childPIDs(of: agent).filter { pid in
            !excluded.contains(pid) && (commandLine(of: pid)?.contains("/.claude/shell-snapshots/") ?? false)
        }
    }
    public static func childPIDs(of parent: Int32) -> [Int32] {
        var pids = [Int32](repeating: 0, count: 1024)
        let bytes = proc_listpids(UInt32(PROC_PPID_ONLY), UInt32(parent), &pids, Int32(pids.count * MemoryLayout<Int32>.size))
        guard bytes > 0 else { return [] }
        return pids.prefix(Int(bytes) / MemoryLayout<Int32>.size).filter { $0 > 0 }
    }
    /// The process's arguments joined by spaces, or nil if it can't be read.
    /// KERN_PROCARGS2 is argc, the executable path, padding, the arguments, then the environment;
    /// only the arguments are returned, so an environment variable can't make a process match.
    public static func commandLine(of pid: Int32) -> String? {
        var mib: [Int32] = [CTL_KERN, KERN_PROCARGS2, pid]
        var size = 0
        guard sysctl(&mib, 3, nil, &size, nil, 0) == 0, size > MemoryLayout<Int32>.size else { return nil }
        var buffer = [UInt8](repeating: 0, count: size)
        guard sysctl(&mib, 3, &buffer, &size, nil, 0) == 0, size > MemoryLayout<Int32>.size else { return nil }
        let argc = buffer.withUnsafeBytes { Int($0.loadUnaligned(as: Int32.self)) }
        var index = MemoryLayout<Int32>.size
        while index < size && buffer[index] != 0 { index += 1 }  // executable path
        while index < size && buffer[index] == 0 { index += 1 }  // padding
        var arguments: [String] = []
        while arguments.count < argc && index < size {
            let start = index
            while index < size && buffer[index] != 0 { index += 1 }
            arguments.append(String(decoding: buffer[start..<index], as: UTF8.self))
            index += 1
        }
        return arguments.joined(separator: " ")
    }
    /// This process and its ancestors, so a hook never counts its own shell as background work.
    public static func ancestry(of pid: Int32 = getpid()) -> Set<Int32> {
        var chain: Set<Int32> = []
        var current = pid
        for _ in 0..<16 where current > 1 && chain.insert(current).inserted {
            var info = proc_bsdinfo()
            let size = Int32(MemoryLayout<proc_bsdinfo>.size)
            guard proc_pidinfo(current, PROC_PIDTBSDINFO, 0, &info, size) == size else { break }
            current = Int32(info.pbi_ppid)
        }
        return chain
    }
}

/// Claude Code's status line is where Claude Code reports plan limits (`rate_limits`), so Barista
/// installs `barista statusline` there. An existing status line is saved, per settings file, and run in
/// front of it: `barista statusline --chain KEY` runs the one saved for that file.
public enum ClaudeStatusLine {
    /// FNV-1a 64-bit hash of the settings file's resolved absolute path, as 16 lowercase hex digits.
    public static func key(for settings: URL) -> String {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in settings.absoluteURL.resolvingSymlinksInPath().path.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        return String(format: "%016llx", hash)
    }
    static func backupFile(_ key: String) -> URL {
        UsageLocations.dataDirectory.appendingPathComponent("statusline-chain-\(key).json")
    }

    public static func command(cliPath: String, chain: String? = nil) -> String {
        "'" + cliPath.replacingOccurrences(of: "'", with: "'\\''") + "' statusline" + (chain.map { " --chain \($0)" } ?? "")
    }
    /// The backup key in one of our commands, if it chains a saved status line.
    static func chainKey(in command: String) -> String? {
        let parts = command.components(separatedBy: " ")
        guard parts.count >= 3, parts[parts.count - 2] == "--chain", isKey(parts[parts.count - 1]) else { return nil }
        return parts[parts.count - 1]
    }
    private static func isKey(_ text: String) -> Bool {
        text.count == 16 && text.unicodeScalars.allSatisfy { ("0"..."9").contains($0) || ("a"..."f").contains($0) }
    }
    static func isOurs(_ command: String) -> Bool {
        var command = command
        if let key = chainKey(in: command) { command = String(command.dropLast(" --chain \(key)".count)) }
        return command.hasSuffix(" statusline") && AgentHooks.isBarista(String(command.dropLast(" statusline".count)))
    }

    /// The status line saved under `key`, or nil if missing or unreadable.
    public static func saved(_ key: String) -> [String: Any]? {
        guard let data = try? Data(contentsOf: backupFile(key)),
              let object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
        return object["statusLine"] as? [String: Any]
    }

    public static func install(cliPath: String, settings: URL? = nil) throws {
        let url = settings ?? HookAgent.claude.defaultSettingsURL
        let key = key(for: url)
        try AgentHooks.editSettings(url) { root in
            var root = root
            var chain: String?
            if let existing = root["statusLine"] as? [String: Any] {
                let current = existing["command"] as? String ?? ""
                if isOurs(current) {
                    chain = chainKey(in: current)
                } else {
                    // Keep the user's own status line working: save it for this settings file, and run it from ours.
                    let backup: [String: Any] = ["settings": url.absoluteURL.resolvingSymlinksInPath().path, "statusLine": existing]
                    let data = try JSONSerialization.data(withJSONObject: backup, options: [.sortedKeys, .withoutEscapingSlashes])
                    try FileManager.default.createDirectory(at: UsageLocations.dataDirectory, withIntermediateDirectories: true)
                    try data.write(to: backupFile(key), options: .atomic)
                    chain = key
                }
            }
            root["statusLine"] = ["type": "command", "command": command(cliPath: cliPath, chain: chain)]
            return root
        }
    }

    public static func uninstall(settings: URL? = nil) throws {
        let url = settings ?? HookAgent.claude.defaultSettingsURL
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        var restored: String?
        try AgentHooks.editSettings(url) { root in
            guard let existing = root["statusLine"] as? [String: Any], let current = existing["command"] as? String,
                  isOurs(current) else { return nil }
            var root = root
            if let key = chainKey(in: current) {
                guard let previous = saved(key) else {
                    throw CoffeeError.message("Barista’s saved copy of your previous status line is missing, so \(url.path) was left unchanged. The original is in \(url.lastPathComponent).barista-backup next to it.")
                }
                root["statusLine"] = previous
                restored = key
            } else {
                root.removeValue(forKey: "statusLine")
            }
            return root
        }
        if let restored { try? FileManager.default.removeItem(at: backupFile(restored)) }
    }

    public enum Status { case connected, chained, notConnected }
    public static func status(settings: URL? = nil) -> Status {
        let url = settings ?? HookAgent.claude.defaultSettingsURL
        guard let data = try? Data(contentsOf: url.resolvingSymlinksInPath()),
              let root = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let line = root["statusLine"] as? [String: Any], let current = line["command"] as? String, isOurs(current) else { return .notConnected }
        return chainKey(in: current) == nil ? .connected : .chained
    }
}

extension AgentHooks {
    /// Edits a settings file with the same safety as hooks: backup, symlinks followed, concurrent edits redone.
    /// `change` returns the new root, or nil to leave the file alone.
    static func editSettings(_ url: URL, _ change: ([String: Any]) throws -> [String: Any]?) throws {
        try edit(url) { root in try change(root).map { .write($0) } ?? .keep }
    }
}
