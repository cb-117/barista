import Foundation

public enum SessionSource: String, Codable, Sendable {
    case manual, application, agent, command
}

public struct CoffeeLease: Codable, Identifiable, Sendable, Equatable {
    public let id: String
    public var title: String
    public let source: SessionSource
    public let startedAt: Date
    public var expiresAt: Date?
    public var hardDeadline: Date?
    public var keepDisplayAwake: Bool
    public let processID: Int32?
    public let processIdentity: String?
    public var progress: Double?
    public var message: String?
    public var lastHeartbeat: Date
    /// Scheduled hours (Settings → Schedule). Their source stays `application` on the wire, so CLI and
    /// MCP processes started by an older version, which don't know a newer source, still decode replies.
    public var scheduled: Bool? = nil
    public var isTask: Bool { source == .agent || source == .command }
    public var isScheduled: Bool { scheduled == true }
}

public enum EndReason: String, Codable, Sendable {
    case expired, released, processExited, batteryLow, stopped, appQuit, powerError
    public var label: String {
        switch self {
        case .expired: "Timer finished"
        case .released: "Work finished"
        case .processExited: "Process finished"
        case .batteryLow: "Battery protection"
        case .stopped: "Stopped"
        case .appQuit: "App quit"
        case .powerError: "Power management error"
        }
    }
}

public struct SessionCompletion: Codable, Identifiable, Sendable {
    public let id: String
    public let title: String
    public let source: SessionSource
    public let startedAt: Date
    public let endedAt: Date
    public let reason: EndReason
    /// Scheduled hours; see `CoffeeLease.scheduled`.
    public var scheduled: Bool? = nil
    public var isScheduled: Bool { scheduled == true }
    public var duration: TimeInterval { max(0, endedAt.timeIntervalSince(startedAt)) }
}

public struct BatteryState: Codable, Sendable, Equatable {
    public let percent: Int
    public let onBattery: Bool
    public init(percent: Int, onBattery: Bool) { self.percent = percent; self.onBattery = onBattery }
}

public struct SessionSnapshot: Codable, Sendable {
    public let version: String
    public let awake: Bool
    public let displayAwake: Bool
    public let battery: BatteryState?
    public let batteryThreshold: Int
    public let batteryBlocked: Bool
    public let sessions: [CoffeeLease]
    public let history: [SessionCompletion]
    public let error: String?
    /// Changes whenever the user stops sessions, battery protection triggers, or the app restarts.
    /// Hooks pass it back to resume a lapsed turn only if nothing was stopped in between.
    public var stopToken: String? = nil
}

public enum ControlAction: String, Codable, Sendable {
    case status, acquire, heartbeat, release, extend, stopAll, history
}

public struct ControlRequest: Codable, Sendable {
    public var action: ControlAction
    public var id: String?
    public var title: String?
    public var seconds: Double?
    public var display: Bool?
    public var processID: Int32?
    public var progress: Double?
    public var message: String?
    public var source: SessionSource?
    /// acquire: hard cap. heartbeat: if set, restarts the hard cap from now (a new user turn).
    public var maxSeconds: Double?
    /// acquire: refuse unless `SessionSnapshot.stopToken` still equals this value.
    public var stopToken: String?
    public init(action: ControlAction, id: String? = nil, title: String? = nil, seconds: Double? = nil,
                display: Bool? = nil, processID: Int32? = nil, progress: Double? = nil,
                message: String? = nil, source: SessionSource? = nil, maxSeconds: Double? = nil,
                stopToken: String? = nil) {
        self.action = action; self.id = id; self.title = title; self.seconds = seconds
        self.display = display; self.processID = processID; self.progress = progress
        self.message = message; self.source = source; self.maxSeconds = maxSeconds
        self.stopToken = stopToken
    }
}

public struct ControlReply: Codable, Sendable {
    public let ok: Bool
    public let session: CoffeeLease?
    public let state: SessionSnapshot?
    public let error: String?
    public init(ok: Bool, session: CoffeeLease? = nil, state: SessionSnapshot? = nil, error: String? = nil) {
        self.ok = ok; self.session = session; self.state = state; self.error = error
    }
}

public enum CoffeeError: LocalizedError {
    case message(String)
    public var errorDescription: String? {
        switch self { case .message(let message): message }
    }
}

public enum CoffeeJSON {
    public static func encoder() -> JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return encoder
    }
    public static func decoder() -> JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }
}
