import BaristaCore
import Darwin
import Foundation

struct Options {
    var command: String
    var values: [String: String] = [:]
    var flags: Set<String> = []
    var positionals: [String] = []
    var child: [String] = []

    init(_ arguments: [String]) throws {
        var arguments = arguments
        command = arguments.isEmpty ? "help" : arguments.removeFirst()
        if let separator = arguments.firstIndex(of: "--") {
            child = Array(arguments[(separator + 1)...])
            arguments = Array(arguments[..<separator])
        }
        let booleanFlags: Set<String> = ["--json", "--display", "--all", "--no-launch"]
        let valueFlags: Set<String> = ["--socket", "--name", "--minutes", "--seconds", "--ttl", "--max-awake", "--pid", "--progress", "--message", "--settings"]
        var index = 0
        while index < arguments.count {
            let token = arguments[index]
            if booleanFlags.contains(token) {
                guard flags.insert(token).inserted else { throw CoffeeError.message("Duplicate option: \(token)") }
            } else if valueFlags.contains(token) {
                index += 1
                guard index < arguments.count, values[token] == nil else { throw CoffeeError.message("Missing or duplicate value for \(token).") }
                values[token] = arguments[index]
            } else if token.hasPrefix("--") {
                throw CoffeeError.message("Unknown option: \(token)")
            } else { positionals.append(token) }
            index += 1
        }
    }
    func validate(values allowedValues: Set<String> = [], flags allowedFlags: Set<String> = [], positionalCount: Int = 0) throws {
        let badValues = Set(values.keys).subtracting(allowedValues.union(["--socket"]))
        let badFlags = flags.subtracting(allowedFlags.union(["--json", "--no-launch"]))
        guard badValues.isEmpty, badFlags.isEmpty, positionals.count == positionalCount,
              command == "run" || child.isEmpty else {
            throw CoffeeError.message("Unexpected arguments for \(command). Run barista help for usage.")
        }
    }
    func number(_ key: String, default fallback: Double) throws -> Double {
        guard let text = values[key] else { return fallback }
        guard let value = Double(text), value.isFinite else { throw CoffeeError.message("\(key) must be a finite number.") }
        return value
    }
    func duration(default fallback: Double) throws -> Double {
        guard values["--minutes"] == nil || values["--seconds"] == nil else { throw CoffeeError.message("Use --minutes or --seconds, not both.") }
        let value = values["--minutes"] != nil ? try number("--minutes", default: 0) * 60 : try number("--seconds", default: fallback)
        guard value > 0, value <= 86400 else { throw CoffeeError.message("Choose a duration above zero and no longer than 24 hours.") }
        return value
    }
}

struct AgentConnection: Sendable {
    let path: String
    let autoLaunch: Bool
    func send(_ request: ControlRequest) throws -> ControlReply {
        let client = ControlClient(path: path)
        do { return try client.send(request) }
        catch let error as POSIXError where error.code == .ENOENT || error.code == .ECONNREFUSED {
            guard autoLaunch, path == ControlEndpoint.path else {
                throw CoffeeError.message("Barista is not running. Open Barista.app and try again.")
            }
            try launchApp()
            // Connect only: no mutation is retried after a request has been sent.
            let deadline = Date().addingTimeInterval(6)
            while Date() < deadline {
                do { _ = try client.send(ControlRequest(action: .status)); return try client.send(request) }
                catch let error as POSIXError where error.code == .ENOENT || error.code == .ECONNREFUSED { Thread.sleep(forTimeInterval: 0.1) }
            }
            throw CoffeeError.message("Barista did not start its control service. Quit any older copy and open the current app.")
        }
    }
    private func launchApp() throws {
        let executable = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath()
        var candidates: [URL] = []
        if let override = ProcessInfo.processInfo.environment["BARISTA_APP"] { candidates.append(URL(fileURLWithPath: override)) }
        if executable.deletingLastPathComponent().lastPathComponent == "Helpers" {
            candidates.append(executable.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent())
        }
        var parent = executable.deletingLastPathComponent()
        for _ in 0..<6 {
            candidates.append(parent.appendingPathComponent("build/Barista.app"))
            parent.deleteLastPathComponent()
        }
        candidates.append(URL(fileURLWithPath: "/Applications/Barista.app"))
        candidates.append(FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications/Barista.app"))
        guard let bundle = candidates.first(where: { FileManager.default.fileExists(atPath: $0.appendingPathComponent("Contents/Info.plist").path) }) else {
            throw CoffeeError.message("Cannot locate Barista.app. Open it manually, or set BARISTA_APP to its full path.")
        }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        process.arguments = ["-g", bundle.path, "--args", "--background"]
        process.standardOutput = FileHandle.standardError
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else { throw CoffeeError.message("macOS couldn’t open Barista.app.") }
    }
}

@main
struct CoffeeCLI {
    static func main() {
        signal(SIGPIPE, SIG_IGN)
        do {
            let options = try Options(Array(CommandLine.arguments.dropFirst()))
            let connection = AgentConnection(path: options.values["--socket"] ?? ControlEndpoint.path,
                                             autoLaunch: !options.flags.contains("--no-launch"))
            if options.command == "version" || options.command == "--version" {
                print("barista \(BaristaVersion.current)")
                return
            }
            if options.command == "help" || options.command == "--help" || options.command == "-h" {
                print(help)
                return
            }
            if options.command == "mcp" {
                try options.validate()
                MCPServer(connection: connection).run()
                return
            }
            if options.command == "hook-wait" {
                // Spawned by the Stop hook; detached, silent, and never fails loudly.
                try? options.validate()
                if let key = options.positionals.first.flatMap(HookLeaseStore.key) {
                    HookWaiter(socketPath: options.values["--socket"] ?? ControlEndpoint.path, key: key,
                               pids: options.positionals.dropFirst().compactMap { Int32($0) }.filter { $0 > 1 }).run()
                }
                exit(0)
            }
            if options.command == "hook" {
                // Called by agent hooks: never fail the agent, never write to stdout.
                try? options.validate()
                AgentHook(socketPath: options.values["--socket"] ?? ControlEndpoint.path,
                          noLaunch: options.flags.contains("--no-launch")).run()
                exit(0)
            }
            if options.command == "hooks" {
                try options.validate(values: ["--settings"], positionalCount: 1)
                let url = options.values["--settings"].map { URL(fileURLWithPath: $0) } ?? ClaudeHooks.defaultSettingsURL
                switch options.positionals[0] {
                case "install":
                    let cli = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath().path
                    try ClaudeHooks.install(cliPath: cli, settings: url)
                    print("Connected Claude Code: \(url.path)")
                case "uninstall":
                    try ClaudeHooks.uninstall(settings: url)
                    print("Disconnected Claude Code: \(url.path)")
                case "status":
                    let commands = ClaudeHooks.installedCommands(settings: url)
                    let cli = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath().path
                    if commands.isEmpty { print("Not connected") }
                    else if ClaudeHooks.isFullyInstalled(cliPath: cli, settings: url) { print("Connected: \(commands.joined(separator: ", "))") }
                    else { print("Needs repair (incomplete or another copy): run barista hooks install") }
                default: throw CoffeeError.message("Use: barista hooks install|uninstall|status [--settings PATH]")
                }
                return
            }
            if options.command == "run" {
                try options.validate(values: ["--name", "--max-awake"], flags: ["--display"])
                exit(try runCommand(options, connection: connection))
            }
            var request: ControlRequest
            switch options.command {
            case "status", "history":
                try options.validate()
                request = ControlRequest(action: options.command == "status" ? .status : .history)
            case "start", "acquire":
                try options.validate(values: ["--name", "--minutes", "--seconds", "--pid", "--max-awake"], flags: ["--display"])
                var pid: Int32?
                if let text = options.values["--pid"] {
                    guard let parsed = Int32(text), parsed > 0 else { throw CoffeeError.message("--pid must be a positive process ID.") }
                    pid = parsed
                }
                request = ControlRequest(action: .acquire, title: options.values["--name"] ?? "Agent task",
                                         seconds: try options.duration(default: 300), display: options.flags.contains("--display"),
                                         processID: pid, maxSeconds: try options.number("--max-awake", default: 28800))
            case "heartbeat":
                try options.validate(values: ["--ttl", "--progress", "--message"], positionalCount: 1)
                request = ControlRequest(action: .heartbeat, id: options.positionals[0],
                                         seconds: try options.number("--ttl", default: 300),
                                         progress: options.values["--progress"] == nil ? nil : try options.number("--progress", default: 0),
                                         message: options.values["--message"])
            case "extend":
                try options.validate(values: ["--minutes", "--seconds"], positionalCount: 1)
                request = ControlRequest(action: .extend, id: options.positionals[0], seconds: try options.duration(default: 900))
            case "stop", "release":
                let all = options.flags.contains("--all")
                try options.validate(flags: ["--all"], positionalCount: all ? 0 : 1)
                request = ControlRequest(action: all ? .stopAll : .release, id: options.positionals.first)
            default: throw CoffeeError.message("Unknown command: \(options.command). Run barista help.")
            }
            let reply = try connection.send(request)
            if options.flags.contains("--json") {
                print(String(decoding: try CoffeeJSON.encoder().encode(reply), as: UTF8.self))
            } else if !reply.ok {
                throw CoffeeError.message(reply.error ?? "The request failed.")
            } else if let lease = reply.session {
                print(lease.id)
            } else if options.command == "history" {
                for event in reply.state?.history ?? [] {
                    print("\(event.endedAt.formatted())  \(event.title) — \(event.reason.label)")
                }
            } else if let state = reply.state {
                print(state.awake ? "Awake · \(state.sessions.count) session(s) · display \(state.displayAwake ? "on" : "allowed to sleep")" : "Resting · no active sessions")
                if state.batteryBlocked { print("Battery protection is active at \(state.batteryThreshold)%.") }
                for lease in state.sessions {
                    let remaining = lease.expiresAt.map { AwakeSession.formatTime($0.timeIntervalSinceNow) } ?? "until stopped"
                    print("\(lease.id)  \(lease.title)  [\(lease.source.rawValue)]  \(remaining)")
                }
            }
            if !reply.ok { exit(1) }
        } catch {
            let message = "barista: \(error.localizedDescription)\n"
            if CommandLine.arguments.contains("--json") {
                if let data = try? CoffeeJSON.encoder().encode(ControlReply(ok: false, error: error.localizedDescription)) {
                    FileHandle.standardOutput.write(data + Data([10]))
                }
            } else { FileHandle.standardError.write(Data(message.utf8)) }
            exit(1)
        }
    }

    private static func runCommand(_ options: Options, connection: AgentConnection) throws -> Int32 {
        guard !options.child.isEmpty else { throw CoffeeError.message("Put a command after --, for example: barista run -- make build") }
        let request = ControlRequest(action: .acquire, title: options.values["--name"] ?? "Command task", seconds: 45,
                                     display: options.flags.contains("--display"), processID: getpid(), source: .command,
                                     maxSeconds: try options.number("--max-awake", default: 28800))
        let reply = try connection.send(request)
        guard reply.ok, let lease = reply.session else { throw CoffeeError.message(reply.error ?? "Couldn’t acquire a session.") }
        // Only the initial acquire may open Barista; maintenance never reopens an app the user quit.
        let connection = AgentConnection(path: connection.path, autoLaunch: false)
        defer { _ = try? connection.send(ControlRequest(action: .release, id: lease.id)) }
        let child = Process()
        child.executableURL = URL(fileURLWithPath: "/usr/bin/env")
        child.arguments = options.child
        child.standardInput = FileHandle.standardInput
        child.standardOutput = FileHandle.standardOutput
        child.standardError = FileHandle.standardError
        try child.run()
        let pid = child.processIdentifier
        let queue = DispatchQueue(label: "barista.command-heartbeat")
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 10, repeating: 10)
        timer.setEventHandler {
            // Never reacquire a lease the user stopped, or defeat battery protection.
            // Once Barista reports the lease gone, stop renewing; the command keeps running.
            if let reply = try? connection.send(ControlRequest(action: .heartbeat, id: lease.id, seconds: 45)), !reply.ok { timer.cancel() }
        }
        timer.resume()
        var signals: [DispatchSourceSignal] = []
        for number in [SIGINT, SIGTERM, SIGHUP] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: queue)
            source.setEventHandler {
                _ = try? connection.send(ControlRequest(action: .release, id: lease.id))
                kill(pid, number)
            }
            source.resume()
            signals.append(source)
        }
        child.waitUntilExit()
        timer.cancel()
        signals.forEach { $0.cancel() }
        return child.terminationReason == .uncaughtSignal ? 128 + child.terminationStatus : child.terminationStatus
    }

    static let help = """
    Barista · keeps your Mac awake while work happens

    barista status [--json]
    barista start --name "Build" [--minutes 30 | --seconds 300] [--display] [--pid PID]
    barista heartbeat SESSION_ID [--ttl 300] [--progress 0.5] [--message "Running tests"]
    barista extend SESSION_ID [--minutes 15 | --seconds 900]
    barista stop SESSION_ID
    barista stop --all
    barista history [--json]
    barista run [--name "Build"] [--display] [--max-awake 28800] -- COMMAND [ARG ...]
    barista mcp
    barista hooks install|uninstall|status [--settings PATH]
    barista version

    Agent sessions default to screen-off-friendly, expire after 5 minutes without renewal,
    and have an 8-hour hard limit (--max-awake accepts seconds, up to 24 hours).
    run renews automatically while its command runs, preserves output/exit code,
    and releases its own session on completion. Other sessions remain active.
    Stopping a session never terminates the underlying command.

    hooks install connects Claude Code: prompts start a session, tool calls renew it,
    and the session ends when Claude finishes its turn or exits (or, if Claude left
    background commands running, once they finish).

    Global options: --json, --no-launch, --socket PATH. The app launches automatically
    when needed. MCP uses newline-delimited JSON-RPC over stdio; no TCP port is opened.
    """
}

/// Handles a Claude Code hook event read from stdin.
struct AgentHook {
    let socketPath: String
    let noLaunch: Bool
    static let ttl: Double = 1200
    static let hardCap: Double = 28800

    func run() {
        let input = FileHandle.standardInput.readData(ofLength: 1_048_576)
        guard let event = (try? JSONSerialization.jsonObject(with: input)) as? [String: Any],
              let name = event["hook_event_name"] as? String,
              let key = (event["session_id"] as? String).flatMap(HookLeaseStore.key) else { return }
        let store = HookLeaseStore(socketPath: socketPath)
        let cwd = (event["cwd"] as? String).map { ($0 as NSString).lastPathComponent } ?? ""
        let title = cwd.isEmpty ? "Claude Code" : "Claude Code · \(cwd.prefix(80))"
        switch name {
        case "UserPromptSubmit": ensure(key, store: store, title: title, userInitiated: true)
        case "PreToolUse", "PostToolUse": ensure(key, store: store, title: title, userInitiated: false)
        case "Stop": stop(key, store: store)
        case "SessionEnd": release(key, store: store)
        default: break
        }
    }

    private func connection(launch: Bool) -> AgentConnection {
        AgentConnection(path: socketPath, autoLaunch: launch && !noLaunch)
    }

    private func ensure(_ key: String, store: HookLeaseStore, title: String, userInitiated: Bool) {
        // Only a new prompt may launch the app or start a turn (with a fresh 8-hour cap). Tool calls
        // renew the turn's lease; they may replace it only if it lapsed from a missed renewal (a long
        // tool call) and the app confirms nothing was stopped since: never after a user Stop or
        // Stop all, battery cutoff, app restart, Claude's Stop, or the turn's hard cap.
        let connection = connection(launch: userInitiated)
        _ = try? store.withLock(key) {
            var turnStarted = Date()
            var stopToken: String?
            if let saved = store.lease(for: key) {
                let reply = try connection.send(ControlRequest(action: .heartbeat, id: saved.id, seconds: Self.ttl,
                                                               message: "Working", maxSeconds: userInitiated ? Self.hardCap : nil))
                if reply.ok {
                    if userInitiated { store.save(saved.id, turnStarted: Date(), stopToken: reply.state?.stopToken, for: key) }
                    return
                }
                store.remove(key)
                if !userInitiated {
                    let ended = reply.state?.history.first { $0.id == saved.id }
                    guard ended?.reason == .expired, let token = saved.stopToken else { return }
                    turnStarted = saved.turnStarted
                    stopToken = token
                }
            } else if !userInitiated { return }
            let remaining = Self.hardCap - Date().timeIntervalSince(turnStarted)
            guard remaining >= 1 else { return }
            let reply = try connection.send(ControlRequest(action: .acquire, title: title, seconds: min(Self.ttl, remaining),
                                                           display: false, processID: SystemState.agentProcessID(),
                                                           maxSeconds: remaining, stopToken: stopToken))
            if let lease = reply.session {
                store.save(lease.id, turnStarted: turnStarted, stopToken: reply.state?.stopToken, for: key)
            }
        }
    }

    /// Claude finished its turn. If commands it started in the background are still running
    /// (T3 and Claude Code show this as "Waiting"), keep the Mac awake until they exit.
    private func stop(_ key: String, store: HookLeaseStore) {
        if let agent = SystemState.agentProcessID() {
            let pids = SystemState.backgroundCommandPIDs(of: agent, excluding: SystemState.ancestry())
            let kept = !pids.isEmpty && ((try? store.withLock(key) { () -> Bool in
                guard let saved = store.lease(for: key) else { return false }
                let reply = try connection(launch: false).send(ControlRequest(
                    action: .heartbeat, id: saved.id, seconds: HookWaiter.grace, message: HookWaiter.waitingMessage))
                return reply.ok
            }) ?? false)
            if kept && HookWaiter.spawn(socketPath: socketPath, key: key, pids: pids) { return }
        }
        release(key, store: store)
    }

    private func release(_ key: String, store: HookLeaseStore) {
        _ = try? store.withLock(key) {
            if let saved = store.lease(for: key) {
                _ = try? connection(launch: false).send(ControlRequest(action: .release, id: saved.id))
            }
            store.remove(key)
        }
    }
}

/// Keeps a Claude session's lease alive while background commands it started are still running,
/// then lets it lapse within a couple of minutes. Gives up as soon as the lease is stopped,
/// replaced, or a new prompt starts a new turn (that turn's hooks take over).
struct HookWaiter {
    static let grace: Double = 120
    static let interval: UInt32 = 30
    static let waitingMessage = "Waiting on background work"
    let socketPath: String
    let key: String
    let pids: [Int32]

    func run() {
        let store = HookLeaseStore(socketPath: socketPath)
        guard let start = store.lease(for: key), !pids.isEmpty else { return }
        let identities = Dictionary(uniqueKeysWithValues: pids.map { ($0, SystemState.processIdentity($0)) })
        let connection = AgentConnection(path: socketPath, autoLaunch: false)
        let giveUp = Date().addingTimeInterval(AgentHook.hardCap)
        while Date() < giveUp {
            sleep(Self.interval)
            let alive = identities.contains { pid, identity in identity != nil && SystemState.processIdentity(pid) == identity }
            guard alive else { return }  // The last heartbeat's short grace runs out on its own.
            let keepGoing = (try? store.withLock(key) { () -> Bool in
                guard let current = store.lease(for: key), current.id == start.id,
                      current.turnStarted == start.turnStarted else { return false }
                return try connection.send(ControlRequest(action: .heartbeat, id: start.id, seconds: Self.grace,
                                                          message: Self.waitingMessage)).ok
            }) ?? false
            guard keepGoing else { return }
        }
    }

    /// Starts `barista hook-wait` in its own session so it outlives the hook's 10-second timeout.
    static func spawn(socketPath: String, key: String, pids: [Int32]) -> Bool {
        guard let executable = Bundle.main.executablePath else { return false }
        let arguments = [executable, "hook-wait", "--socket", socketPath, key] + pids.map(String.init)
        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSID))
        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        for descriptor in [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO] {
            posix_spawn_file_actions_addopen(&actions, descriptor, "/dev/null", descriptor == STDIN_FILENO ? O_RDONLY : O_WRONLY, 0)
        }
        var argv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) } + [nil]
        defer { argv.forEach { free($0) } }
        var pid: pid_t = 0
        return posix_spawn(&pid, executable, &actions, &attributes, &argv, environ) == 0
    }
}
