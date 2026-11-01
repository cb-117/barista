import BaristaCore
import CoreFoundation
import Foundation

/// A stdio MCP server. The shared Mac app owns power assertions, not this subprocess.
struct MCPServer {
    let connection: AgentConnection
    private static let versions = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"]

    func run() {
        var initialized = false
        while let line = readLine() {
            var responseID: Any = NSNull()
            do {
                guard line.utf8.count <= 1_048_576, let data = line.data(using: .utf8) else {
                    throw RPCError(code: -32700, message: "Expected a JSON message under 1 MiB.")
                }
                let decoded: Any
                do { decoded = try JSONSerialization.jsonObject(with: data) }
                catch { throw RPCError(code: -32700, message: "Invalid JSON.") }
                guard let request = decoded as? [String: Any] else {
                    throw RPCError(code: -32600, message: "Expected a JSON-RPC request object.")
                }
                if let id = request["id"] {
                    guard id is String || (id is NSNumber && CFGetTypeID(id as! NSNumber) != CFBooleanGetTypeID()) else {
                        throw RPCError(code: -32600, message: "Request ID must be a string or number.")
                    }
                    responseID = id
                }
                guard request["jsonrpc"] as? String == "2.0", let method = request["method"] as? String else {
                    throw RPCError(code: -32600, message: "Expected a JSON-RPC 2.0 request.")
                }
                // Notifications never produce replies (including initialized/cancelled).
                guard request["id"] != nil else { continue }
                guard request["params"] == nil || request["params"] is [String: Any] else {
                    throw RPCError(code: -32602, message: "params must be an object.")
                }
                let params = request["params"] as? [String: Any] ?? [:]
                let result: [String: Any]
                switch method {
                case "initialize":
                    guard let requested = params["protocolVersion"] as? String else {
                        throw RPCError(code: -32602, message: "protocolVersion is required.")
                    }
                    initialized = true
                    result = [
                        "protocolVersion": Self.versions.contains(requested) ? requested : Self.versions[0],
                        "capabilities": ["tools": ["listChanged": false]],
                        "serverInfo": ["name": "barista", "title": "Barista", "version": BaristaVersion.current],
                        "instructions": "Keep this Mac awake only during active work. Acquire a named session, renew it with heartbeats (default TTL: 300 seconds), report progress, and release your own ID when done or waiting for the user. Sessions stop on expiry, process exit, battery protection, or the user's Stop action. Never reacquire to override a user's Stop. Display stays off-friendly by default. No lid-closed sleep override."
                    ]
                case "ping": result = [:]
                case "tools/list":
                    guard initialized else { throw RPCError(code: -32000, message: "Initialize first.") }
                    if params["cursor"] != nil { throw RPCError(code: -32602, message: "This server has no additional tool pages.") }
                    result = ["tools": Self.tools]
                case "tools/call":
                    guard initialized else { throw RPCError(code: -32000, message: "Initialize first.") }
                    result = try call(params)
                default: throw RPCError(code: -32601, message: "Unsupported method: \(method)")
                }
                emit(["jsonrpc": "2.0", "id": responseID, "result": result])
            } catch let error as RPCError {
                emit(["jsonrpc": "2.0", "id": responseID, "error": ["code": error.code, "message": error.message]])
            } catch {
                emit(["jsonrpc": "2.0", "id": responseID, "error": ["code": -32603, "message": error.localizedDescription]])
            }
        }
    }

    private func call(_ params: [String: Any]) throws -> [String: Any] {
        guard let name = params["name"] as? String,
              let tool = Self.tools.first(where: { $0["name"] as? String == name }) else {
            throw RPCError(code: -32602, message: "Unknown tool.")
        }
        guard params["arguments"] == nil || params["arguments"] is [String: Any] else {
            throw RPCError(code: -32602, message: "arguments must be an object.")
        }
        let arguments = params["arguments"] as? [String: Any] ?? [:]
        let schema = tool["inputSchema"] as! [String: Any]
        let properties = schema["properties"] as! [String: Any]
        let required = schema["required"] as? [String] ?? []
        guard Set(arguments.keys).isSubset(of: Set(properties.keys)), required.allSatisfy({ arguments[$0] != nil }) else {
            throw RPCError(code: -32602, message: "Unexpected or missing arguments for \(name).")
        }
        if name == "barista_usage" {
            // Read from local logs in this process; the app isn't involved.
            let period: UsagePeriod
            if arguments["period"] == nil { period = .week }
            else if let text = arguments["period"] as? String, let value = UsagePeriod(rawValue: text) { period = value }
            else { return ["content": [["type": "text", "text": "period must be today, week, month, or all."]], "isError": true] }
            let report = UsageStore.report(periods: [period])
            return ["content": [["type": "text", "text": report.json()]], "structuredContent": report.jsonObject, "isError": false]
        }
        do {
            let request: ControlRequest
            switch name {
            case "barista_status": request = ControlRequest(action: .status)
            case "barista_history": request = ControlRequest(action: .history)
            case "barista_acquire":
                var pid: Int32?
                if arguments["pid"] != nil {
                    let value = try number(arguments, "pid", fallback: 0)
                    guard value > 0, value.rounded() == value, value <= Double(Int32.max) else { throw CoffeeError.message("pid must be a positive process ID.") }
                    pid = Int32(value)
                }
                request = ControlRequest(action: .acquire, title: try string(arguments, "title"),
                                         seconds: try number(arguments, "ttl_seconds", fallback: 300),
                                         display: try boolean(arguments, "display_awake", fallback: false), processID: pid,
                                         maxSeconds: try number(arguments, "max_awake_seconds", fallback: 28800))
            case "barista_heartbeat":
                request = ControlRequest(action: .heartbeat, id: try string(arguments, "id"),
                                         seconds: try number(arguments, "ttl_seconds", fallback: 300),
                                         progress: arguments["progress"] == nil ? nil : try number(arguments, "progress", fallback: 0),
                                         message: arguments["message"] == nil ? nil : try string(arguments, "message"))
            case "barista_extend":
                request = ControlRequest(action: .extend, id: try string(arguments, "id"),
                                         seconds: try number(arguments, "seconds", fallback: 900))
            case "barista_release": request = ControlRequest(action: .release, id: try string(arguments, "id"))
            default: throw CoffeeError.message("Unknown tool.")
            }
            let reply = try connection.send(request)
            let data = try CoffeeJSON.encoder().encode(reply)
            let text = String(decoding: data, as: UTF8.self)
            return ["content": [["type": "text", "text": text]], "structuredContent": try JSONSerialization.jsonObject(with: data), "isError": !reply.ok]
        } catch {
            return ["content": [["type": "text", "text": error.localizedDescription]], "isError": true]
        }
    }

    private func string(_ values: [String: Any], _ key: String) throws -> String {
        guard let value = values[key] as? String else { throw CoffeeError.message("\(key) must be a string.") }
        return value
    }
    private func number(_ values: [String: Any], _ key: String, fallback: Double) throws -> Double {
        guard let value = values[key] else { return fallback }
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite else {
            throw CoffeeError.message("\(key) must be a finite number.")
        }
        return number.doubleValue
    }
    private func boolean(_ values: [String: Any], _ key: String, fallback: Bool) throws -> Bool {
        guard let value = values[key] else { return fallback }
        guard let number = value as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else {
            throw CoffeeError.message("\(key) must be a boolean.")
        }
        return number.boolValue
    }
    private func emit(_ message: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: message, options: [.sortedKeys, .withoutEscapingSlashes]) else { return }
        FileHandle.standardOutput.write(data + Data([10]))
    }

    private static func tool(_ name: String, title: String, description: String, properties: [String: Any] = [:], required: [String] = [], readOnly: Bool = false) -> [String: Any] {
        ["name": name, "title": title, "description": description,
         "inputSchema": ["type": "object", "properties": properties, "required": required, "additionalProperties": false],
         "annotations": ["readOnlyHint": readOnly, "destructiveHint": false, "idempotentHint": readOnly || name == "barista_release", "openWorldHint": false]]
    }
    private static var id: [String: Any] { ["type": "string", "description": "The session ID returned by barista_acquire."] }
    private static var ttl: [String: Any] { ["type": "number", "exclusiveMinimum": 0, "maximum": 86400, "default": 300, "description": "Seconds until automatic expiry unless renewed. Use a short lease, then heartbeat during active work."] }
    private static var tools: [[String: Any]] {
        [
            tool("barista_status", title: "Inspect awake sessions", description: "Get Mac awake/display state, active session owners, progress, expiry, and battery protection.", readOnly: true),
            tool("barista_acquire", title: "Keep Mac awake for a task", description: "Acquire your own expiring awake session. Other agents keep their independent sessions. Default: 5-minute lease, 8-hour hard limit, display may sleep.", properties: [
                "title": ["type": "string", "minLength": 1, "maxLength": 100, "description": "A short human-readable task name shown in the menu bar panel."],
                "ttl_seconds": ttl, "display_awake": ["type": "boolean", "default": false],
                "pid": ["type": "integer", "minimum": 1, "description": "Optional local process to watch; session ends when that process exits."],
                "max_awake_seconds": ["type": "number", "exclusiveMinimum": 0, "maximum": 86400, "default": 28800]
            ], required: ["title"]),
            tool("barista_heartbeat", title: "Renew task and report progress", description: "Renew a live session and optionally publish progress/message. An expired or user-stopped session cannot be revived.", properties: [
                "id": id, "ttl_seconds": ttl, "progress": ["type": "number", "minimum": 0, "maximum": 1],
                "message": ["type": "string", "maxLength": 200]
            ], required: ["id"]),
            tool("barista_extend", title: "Extend a session timer", description: "Add seconds to an existing timer within its original hard limit.", properties: ["id": id, "seconds": ttl], required: ["id", "seconds"]),
            tool("barista_release", title: "Release your awake session", description: "Release your own session when work completes or waits for user input. Other sessions remain active. Repeated releases are safe.", properties: ["id": id], required: ["id"]),
            tool("barista_history", title: "Read recent session completions", description: "Read the last 20 local session summaries and reasons they ended. No command arguments or output are recorded.", readOnly: true),
            tool("barista_usage", title: "Read agent token usage", description: "Sessions, requests, and tokens per agent (Claude Code, Codex, Cursor) and model for a period, from local logs, with costs estimated at API list prices (not what a subscription charges). Cursor reports turns and active time instead of tokens. Also active time, sessions, tokens, models, and cost per project (Git repository).", properties: [
                "period": ["type": "string", "enum": ["today", "week", "month", "all"], "default": "week"]
            ], readOnly: true)
        ]
    }
}

private struct RPCError: Error {
    let code: Int
    let message: String
}
