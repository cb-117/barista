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
        let booleanFlags: Set<String> = ["--json", "--display", "--all", "--no-launch", "--no-open"]
        let valueFlags: Set<String> = ["--socket", "--name", "--minutes", "--seconds", "--ttl", "--max-awake", "--pid", "--progress", "--message", "--settings", "--agent", "--period", "--chain", "--output"]
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
            if options.command == "limits" {
                // Fetches Claude account limits once (the app does this every few minutes).
                try options.validate(positionalCount: 1)
                guard options.positionals[0] == "refresh" else { throw CoffeeError.message("Use: barista limits refresh") }
                // An explicit command, so macOS may ask for Keychain access; the app's background reads never do.
                let outcome = runBlocking { await ClaudeAccount.refresh(interactive: true) }
                if options.flags.contains("--json") {
                    let object: [String: Any] = ["ok": outcome.ok, "message": outcome.message]
                    if let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]) {
                        print(String(decoding: data, as: UTF8.self))
                    }
                } else {
                    print(outcome.message)
                }
                if !outcome.ok { exit(1) }
                return
            }
            if options.command == "usage" {
                // Reads local logs directly; never needs the app.
                try options.validate(values: ["--period"])
                var periods: [UsagePeriod] = [.today, .week, .month]
                if let name = options.values["--period"] {
                    guard let period = UsagePeriod(rawValue: name) else { throw CoffeeError.message("--period must be today, week, month, or all.") }
                    periods = [period]
                }
                let report = UsageStore.report(periods: periods)
                print(options.flags.contains("--json") ? report.json() : report.text(detailed: options.values["--period"] != nil))
                return
            }
            if options.command == "report" {
                try options.validate(values: ["--period", "--output"], flags: ["--no-open"])
                var initial = UsagePeriod.week
                if let name = options.values["--period"] {
                    guard let period = UsagePeriod(rawValue: name) else { throw CoffeeError.message("--period must be today, week, month, or all.") }
                    initial = period
                }
                let target = options.values["--output"].map { URL(fileURLWithPath: $0) } ?? ReportTemplate.defaultURL
                let url: URL
                do { url = try ReportTemplate.write(initialPeriod: initial, to: target) } catch {
                    throw CoffeeError.message("Couldn't write the report to \(target.path): \(error.localizedDescription)")
                }
                var opened = false
                if !options.flags.contains("--no-open") {
                    let open = Process()
                    open.executableURL = URL(fileURLWithPath: "/usr/bin/open")
                    open.arguments = [url.path]
                    if (try? open.run()) != nil { open.waitUntilExit(); opened = open.terminationStatus == 0 }
                }
                if options.flags.contains("--json") {
                    let data = try JSONSerialization.data(withJSONObject: ["path": url.path, "opened": opened], options: [.sortedKeys, .withoutEscapingSlashes])
                    print(String(decoding: data, as: UTF8.self))
                } else {
                    print("Wrote \(url.path)")
                }
                return
            }
            if options.command == "statusline" {
                if let action = options.positionals.first {
                    try options.validate(values: ["--settings"], positionalCount: 1)
                    let url = options.values["--settings"].map { URL(fileURLWithPath: $0) }
                    let path = (url ?? HookAgent.claude.defaultSettingsURL).path
                    switch action {
                    case "install":
                        let cli = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath().path
                        try ClaudeStatusLine.install(cliPath: cli, settings: url)
                        print("Added Claude plan limits to Claude Code’s status line: \(path)")
                        if ClaudeStatusLine.status(settings: url) == .chained { print("Your own status line still shows; Barista runs it.") }
                    case "uninstall":
                        try ClaudeStatusLine.uninstall(settings: url)
                        print("Removed Barista from Claude Code’s status line: \(path)")
                    case "status":
                        switch ClaudeStatusLine.status(settings: url) {
                        case .connected: print("Connected")
                        case .chained: print("Connected (in front of your own status line)")
                        case .notConnected: print("Not connected")
                        }
                    default: throw CoffeeError.message("Use: barista statusline install|uninstall|status [--settings PATH]")
                    }
                    return
                }
                // Run by Claude Code as its status line: never fail, never block it.
                try? options.validate(values: ["--chain"])
                StatusLine.run(chain: options.values["--chain"])
                exit(0)
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
                          noLaunch: options.flags.contains("--no-launch"),
                          agent: options.values["--agent"].flatMap(HookAgent.init(rawValue:)) ?? .claude).run()
                exit(0)
            }
            if options.command == "setup" {
                // One step for people and agents: connect every coding agent found, then start Barista.
                try options.validate(values: ["--agent"])
                let cli = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath().path
                let agents: [HookAgent]
                if let name = options.values["--agent"] {
                    guard let agent = HookAgent(rawValue: name) else { throw CoffeeError.message("--agent must be claude, codex, or cursor.") }
                    agents = [agent]
                } else {
                    agents = HookAgent.allCases.filter(\.isInstalled)
                }
                for agent in agents {
                    try AgentHooks.install(cliPath: cli, agent: agent)
                    print("Connected \(agent.displayName) (\(agent.defaultSettingsURL.path))")
                }
                if agents.contains(.claude) {
                    try ClaudeStatusLine.install(cliPath: cli)
                    print("Added Claude plan limits to Claude Code’s status line")
                }
                if agents.isEmpty { print("No Claude Code, Codex, or Cursor setup found for this user. Run barista setup again after using one of them.") }
                let state = try connection.send(ControlRequest(action: .status)).state
                print("Barista \(state?.version ?? BaristaVersion.current) is running in the menu bar (the coffee cup).")
                if agents.contains(.codex) {
                    print("One step left for Codex: it runs new hooks only after you trust them. Open Codex, type /hooks, and trust Barista’s hooks.")
                }
                return
            }
            if options.command == "hooks" {
                try options.validate(values: ["--settings", "--agent"], positionalCount: 1)
                guard let agent = HookAgent(rawValue: options.values["--agent"] ?? "claude") else {
                    throw CoffeeError.message("--agent must be claude, codex, or cursor.")
                }
                let url = options.values["--settings"].map { URL(fileURLWithPath: $0) } ?? agent.defaultSettingsURL
                let cli = URL(fileURLWithPath: CommandLine.arguments[0]).resolvingSymlinksInPath().path
                switch options.positionals[0] {
                case "install":
                    try AgentHooks.install(cliPath: cli, agent: agent, settings: url)
                    print("Connected \(agent.displayName): \(url.path)")
                    if agent == .codex { print("Codex runs new hooks only after you trust them: open Codex, run /hooks, and trust Barista’s hooks.") }
                case "uninstall":
                    try AgentHooks.uninstall(agent: agent, settings: url)
                    print("Disconnected \(agent.displayName): \(url.path)")
                case "status":
                    let commands = AgentHooks.installedCommands(agent: agent, settings: url)
                    if commands.isEmpty { print("Not connected") }
                    else if AgentHooks.isFullyInstalled(cliPath: cli, agent: agent, settings: url) { print("Connected: \(commands.joined(separator: ", "))") }
                    else { print("Needs repair (incomplete or another copy): run barista hooks install\(agent == .claude ? "" : " --agent \(agent.rawValue)")") }
                default: throw CoffeeError.message("Use: barista hooks install|uninstall|status [--agent claude|codex|cursor] [--settings PATH]")
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
        let release = { _ = try? connection.send(ControlRequest(action: .release, id: lease.id)) }
        defer { release() }
        // Signals the caller chose to ignore (nohup ignores SIGHUP; shells ignore SIGINT for `&` jobs)
        // stay ignored in the child; everything else gets its default back. SIGPIPE is ignored by
        // this CLI itself, so the child always gets the default.
        let inheritedIgnored = Set((1..<NSIG).filter { number in
            var current = sigaction()
            return number != SIGPIPE && sigaction(number, nil, &current) == 0
                && unsafeBitCast(current.__sigaction_u.__sa_handler, to: Int.self) == unsafeBitCast(SIG_IGN, to: Int.self)
        })
        // In a terminal's foreground job, Ctrl-C and a hangup already reach the child (it shares this
        // process's group), so forwarding them would deliver them twice. Without a terminal they can
        // only arrive by PID, so they're forwarded. SIGTERM usually arrives by PID (`kill`, `timeout`,
        // supervisors) and is always forwarded. This process ignores them all and waits, so the
        // session lasts until the child actually exits, even if it catches a signal and keeps going.
        let terminalDelivers = [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO].contains { isatty($0) == 1 && tcgetpgrp($0) == getpgrp() }
        let forwarded = (terminalDelivers ? [SIGTERM] : [SIGINT, SIGHUP, SIGTERM]).filter { !inheritedIgnored.contains($0) }
        for number in [SIGINT, SIGHUP, SIGTERM] { signal(number, SIG_IGN) }
        let queue = DispatchQueue(label: "barista.command-heartbeat")
        var childPID: pid_t = 0         // Read and written only on `queue`.
        var pending: [Int32] = []       // Signals that arrived before the child existed.
        let sources = forwarded.map { number in
            let source = DispatchSource.makeSignalSource(signal: number, queue: queue)
            source.setEventHandler { if childPID > 0 { kill(childPID, number) } else { pending.append(number) } }
            source.resume()
            return source
        }
        defer { sources.forEach { $0.cancel() } }
        let pid = try spawnInForeground(["/usr/bin/env"] + options.child, ignoring: inheritedIgnored)
        queue.sync {
            childPID = pid
            pending.forEach { kill(pid, $0) }
        }
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + 10, repeating: 10)
        timer.setEventHandler {
            // Never reacquire a lease the user stopped, or defeat battery protection.
            // Once Barista reports the lease gone, stop renewing; the command keeps running.
            if let reply = try? connection.send(ControlRequest(action: .heartbeat, id: lease.id, seconds: 45)), !reply.ok { timer.cancel() }
        }
        timer.resume()
        var status: Int32 = 0
        while waitpid(pid, &status, 0) < 0 && errno == EINTR {}
        timer.cancel()
        let signalNumber = status & 0x7f
        if signalNumber == SIGINT && !inheritedIgnored.contains(SIGINT) {
            // Die from SIGINT too, so a calling shell script stops at Ctrl-C instead of carrying on.
            release()
            signal(SIGINT, SIG_DFL)
            kill(getpid(), SIGINT)
        }
        return signalNumber == 0 ? (status >> 8) & 0xff : 128 + signalNumber
    }

    /// Spawns the command in this process's group, so it stays in the terminal's foreground job:
    /// it can read the terminal, prompt for passwords, run pagers, and receive Ctrl-C and Ctrl-Z.
    /// (Foundation's Process starts children in their own background group, which macOS stops
    /// as soon as they touch the terminal.) Only stdin, stdout, and stderr are inherited.
    private static func spawnInForeground(_ arguments: [String], ignoring ignored: Set<Int32>) throws -> pid_t {
        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        var defaults = sigset_t(), none = sigset_t()
        sigfillset(&defaults)
        for number in ignored { sigdelset(&defaults, number) }
        sigemptyset(&none)
        posix_spawnattr_setsigdefault(&attributes, &defaults)
        posix_spawnattr_setsigmask(&attributes, &none)
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_SETSIGDEF | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_CLOEXEC_DEFAULT))
        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        // A stream the caller closed (`<&-`) stays closed rather than failing the spawn.
        for descriptor in [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO] where fcntl(descriptor, F_GETFD) != -1 {
            posix_spawn_file_actions_addinherit_np(&actions, descriptor)
        }
        var argv: [UnsafeMutablePointer<CChar>?] = arguments.map { strdup($0) } + [nil]
        defer { argv.forEach { free($0) } }
        var pid: pid_t = 0
        let result = posix_spawn(&pid, arguments[0], &actions, &attributes, &argv, environ)
        guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: result) ?? .EIO) }
        return pid
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
    barista hooks install|uninstall|status [--agent claude|codex|cursor] [--settings PATH]
    barista setup [--agent claude|codex|cursor]
    barista usage [--period today|week|month|all] [--json]
    barista report [--period today|week|month|all] [--output PATH] [--no-open] [--json]
    barista limits refresh [--json]
    barista statusline install|uninstall|status [--settings PATH]
    barista version

    Agent sessions default to screen-off-friendly, expire after 5 minutes without renewal,
    and have an 8-hour hard limit (--max-awake accepts seconds, up to 24 hours).
    run renews automatically while its command runs, preserves output/exit code,
    and releases its own session on completion. Other sessions remain active.
    Stopping a session never terminates the underlying command.

    setup connects every agent it finds (Claude Code, Codex, Cursor) and starts Barista.
    limits refresh fetches your Claude plan limits from your Claude account now, using
    Claude Code's saved login read-only (the app does this every few minutes).
    usage reports plan limits, then sessions and tokens per agent and model from local logs,
    with costs estimated at API list prices (not what a subscription charges); with --period,
    also active time, sessions, tokens, and cost per project (Git repository).
    report writes a visual usage report (an HTML page, nothing uploaded) and opens it.
    statusline install makes Claude Code's status line show (and Barista record) your Claude
    plan limits; an existing status line keeps working.
    hooks install connects Claude Code (or Codex with --agent codex, which then asks you to
    trust the hooks once with /hooks): prompts start a session, tool calls renew it,
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
    var agent: HookAgent = .claude
    static let ttl: Double = 1200
    static let hardCap: Double = 28800

    func run() {
        // Read everything: a large tool input or output cut short would fail to parse and skip the renewal.
        let input = FileHandle.standardInput.readDataToEndOfFile()
        let event = (try? JSONSerialization.jsonObject(with: input)) as? [String: Any]
        let name = event?["hook_event_name"] as? String
        if agent == .cursor {
            // Cursor reads JSON from every hook: carry on, and let the prompt through.
            defer { print(name == "beforeSubmitPrompt" ? #"{"continue":true}"# : "{}") }
            if let event, let name { cursor(event, name: name) }
            return
        }
        // Codex requires JSON on stdout from Stop hooks; an empty object means "carry on".
        defer { if agent == .codex && name == "Stop" { print("{}") } }
        guard let event, let name, let session = event["session_id"] as? String,
              // Codex and Claude sessions never share a lease, even if their IDs collide.
              let key = HookLeaseStore.key(agent == .claude ? session : "codex-\(session)") else { return }
        let store = HookLeaseStore(socketPath: socketPath)
        let cwd = (event["cwd"] as? String).map { ($0 as NSString).lastPathComponent } ?? ""
        let title = cwd.isEmpty ? agent.displayName : "\(agent.displayName) · \(cwd.prefix(80))"
        switch name {
        case "UserPromptSubmit": ensure(key, store: store, title: title, userInitiated: true)
        case "PreToolUse", "PostToolUse": ensure(key, store: store, title: title, userInitiated: false)
        case "Stop": stop(key, store: store)
        case "SessionEnd": release(key, store: store)
        default: break
        }
    }

    /// Cursor: records prompts and stops for usage tracking, and keeps the Mac awake per conversation.
    private func cursor(_ event: [String: Any], name: String) {
        guard let conversation = event["conversation_id"] as? String else { return }
        let generation = event["generation_id"] as? String ?? ""
        let model = event["model"] as? String
        let root = (event["workspace_roots"] as? [Any])?.first as? String
        // Recorded first, so usage is tracked even when Barista isn't running.
        switch name {
        case "beforeSubmitPrompt":
            UsageStore.recordCursor(event: "prompt", conversation: conversation, generation: generation, model: model, status: nil, project: root)
        case "stop":
            UsageStore.recordCursor(event: "stop", conversation: conversation, generation: generation, model: model, status: event["status"] as? String, project: root)
        default: break
        }
        guard let key = HookLeaseStore.key("cursor-\(conversation)") else { return }
        let store = HookLeaseStore(socketPath: socketPath)
        let folder = ((event["workspace_roots"] as? [Any])?.first as? String).map { ($0 as NSString).lastPathComponent } ?? ""
        let title = folder.isEmpty ? agent.displayName : "\(agent.displayName) · \(folder.prefix(80))"
        switch name {
        case "beforeSubmitPrompt": ensure(key, store: store, title: title, userInitiated: true)
        case "postToolUse", "afterAgentResponse": ensure(key, store: store, title: title, userInitiated: false)
        case "stop", "sessionEnd": release(key, store: store)
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
        // Background-command tracking relies on how Claude Code's Bash tool runs commands.
        if agent == .claude, let agent = SystemState.agentProcessID() {
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
                // A tool call since Stop means Claude resumed this turn and renews the lease itself;
                // a short "waiting" heartbeat now would cut its 20-minute lease to two minutes.
                // Hooks hold this lock while renewing, so the check and the heartbeat can't interleave.
                let status = try connection.send(ControlRequest(action: .status))
                guard status.state?.sessions.first(where: { $0.id == start.id })?.message == Self.waitingMessage else { return false }
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

/// Claude Code's status line: records `rate_limits`, then shows your own status line or a short one.
enum StatusLine {
    static func run(chain: String?) {
        let input = FileHandle.standardInput.readDataToEndOfFile()
        let object = (try? JSONSerialization.jsonObject(with: input)) as? [String: Any]
        let limits = object?["rate_limits"] as? [String: Any]
        if let limits { ClaudeLimits.record(limits) }
        if let chain, let command = ClaudeStatusLine.saved(chain)?["command"] as? String {
            FileHandle.standardOutput.write(runShell(command, input: input))
            return
        }
        var parts: [String] = []
        if let name = (object?["model"] as? [String: Any])?["display_name"] as? String { parts.append(name) }
        let now = Date().timeIntervalSince1970
        for (key, short) in [("five_hour", "5h"), ("seven_day", "7d")] {
            guard let window = limits?[key] as? [String: Any], let used = window["used_percentage"] as? NSNumber else { continue }
            if let resets = (window["resets_at"] as? NSNumber)?.doubleValue, resets <= now { continue }
            parts.append("\(short) \(Int(used.doubleValue.rounded()))%")
        }
        print(parts.joined(separator: " · "))
    }

    /// Runs the user's own status line with the same input and returns what it printed.
    private static func runShell(_ command: String, input: Data) -> Data {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        let stdin = Pipe(), stdout = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return Data() }
        try? stdin.fileHandleForWriting.write(contentsOf: input)
        try? stdin.fileHandleForWriting.close()
        let output = stdout.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return output
    }
}

/// Runs an async operation to completion from synchronous CLI code.
private final class BlockingBox<T>: @unchecked Sendable { var value: T? }

func runBlocking<T: Sendable>(_ operation: @escaping @Sendable () async -> T) -> T {
    let box = BlockingBox<T>()
    let done = DispatchSemaphore(value: 0)
    Task.detached {
        box.value = await operation()
        done.signal()
    }
    done.wait()
    return box.value!
}
