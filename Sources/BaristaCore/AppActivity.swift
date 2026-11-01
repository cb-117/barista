import Darwin
import Foundation

/// Desktop apps that run agents but offer no hooks: Claude's Cowork (which works inside a VM the
/// Claude app runs) and the ChatGPT app. Barista keeps the Mac awake while they're busy, judged by the
/// CPU time of the app, its helper processes, and anything macOS says it's responsible for (the VM).
public struct WatchedApp: Sendable, Equatable {
    public let id: String
    /// The awake session's title.
    public let title: String
    /// The app's main executable, relative to the .app bundle.
    let bundleName: String
    let executable: String

    public static let cowork = WatchedApp(id: "cowork", title: "Claude app", bundleName: "Claude.app", executable: "Contents/MacOS/Claude")
    public static let chatgpt = WatchedApp(id: "chatgpt", title: "ChatGPT app", bundleName: "ChatGPT.app", executable: "Contents/MacOS/ChatGPT")
    public static let all = [cowork, chatgpt]

    public var isInstalled: Bool {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return ["/Applications", home + "/Applications"].contains { FileManager.default.fileExists(atPath: "\($0)/\(bundleName)") }
    }
    func isMain(_ path: String) -> Bool { path.hasSuffix("/\(bundleName)/\(executable)") }
}

/// When an app counts as busy. Idle, the Claude and ChatGPT apps use about 1–2% of one core.
public enum ActivityPolicy {
    public static let sampleInterval: TimeInterval = 10
    /// Busy: at least `busyShare` of one core, averaged over `window`.
    public static let window: TimeInterval = 30
    public static let busyShare = 0.10
    /// Each busy sample keeps the Mac awake this long, so pauses while the agent waits on the
    /// server don't let it sleep.
    public static let lease: TimeInterval = 300
    /// After you stop an app's session, it stays stopped until the app has been quiet this long.
    public static let quietBeforeRestart: TimeInterval = 120
    public static let hardCap: TimeInterval = 28800
}

/// One running process, as sampled.
public struct ProcessSample: Sendable {
    public let pid: Int32
    public let parent: Int32
    public let responsible: Int32
    public let path: String
    /// User + system CPU time, in seconds.
    public let cpu: Double
}

public enum ProcessTable {
    private typealias ResponsibleFunction = @convention(c) (pid_t) -> pid_t
    /// macOS's "responsible process" (what Activity Monitor groups by); private, so looked up at run time.
    private static let responsibleFor: ResponsibleFunction? = {
        guard let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "responsibility_get_pid_responsible_for_pid") else { return nil }
        return unsafeBitCast(symbol, to: ResponsibleFunction.self)
    }()

    public static func snapshot() -> [ProcessSample] {
        var count = proc_listallpids(nil, 0)
        guard count > 0 else { return [] }
        var pids = [pid_t](repeating: 0, count: Int(count) + 64)
        count = proc_listallpids(&pids, Int32(pids.count * MemoryLayout<pid_t>.size))
        guard count > 0 else { return [] }
        var timebase = mach_timebase_info()
        mach_timebase_info(&timebase)
        let toSeconds = Double(timebase.numer) / Double(timebase.denom) / 1e9
        var path = [CChar](repeating: 0, count: 4096)
        return pids.prefix(Int(count)).compactMap { pid -> ProcessSample? in
            guard pid > 0 else { return nil }
            var info = proc_bsdinfo()
            let size = Int32(MemoryLayout<proc_bsdinfo>.size)
            guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size) == size else { return nil }
            var usage = rusage_info_v2()
            let read = withUnsafeMutablePointer(to: &usage) {
                $0.withMemoryRebound(to: rusage_info_t?.self, capacity: 1) { proc_pid_rusage(pid, RUSAGE_INFO_V2, $0) }
            }
            let cpu = read == 0 ? Double(usage.ri_user_time + usage.ri_system_time) * toSeconds : 0
            let length = proc_pidpath(pid, &path, UInt32(path.count))
            let executable = length > 0 ? String(decoding: path.prefix(Int(length)).map { UInt8(bitPattern: $0) }, as: UTF8.self) : ""
            return ProcessSample(pid: pid, parent: Int32(info.pbi_ppid), responsible: responsibleFor?(pid) ?? pid, path: executable, cpu: cpu)
        }
    }

    /// The app's main processes, their descendants, and processes macOS says they're responsible for.
    public static func tree(of app: WatchedApp, in table: [ProcessSample]) -> (root: Int32?, members: [Int32: Double]) {
        let roots = Set(table.filter { app.isMain($0.path) }.map(\.pid))
        guard !roots.isEmpty else { return (nil, [:]) }
        var members = roots
        var grew = true
        while grew {
            grew = false
            for process in table where !members.contains(process.pid)
                && (members.contains(process.parent) || roots.contains(process.responsible)) {
                members.insert(process.pid)
                grew = true
            }
        }
        var cpu: [Int32: Double] = [:]
        for process in table where members.contains(process.pid) { cpu[process.pid] = process.cpu }
        return (roots.min(), cpu)
    }
}

/// Decides, sample by sample, whether an app is busy. Pure, so it can be tested with made-up samples.
public struct ActivityTracker: Sendable {
    private var previous: [Int32: Double] = [:]
    private var intervals: [(end: Date, seconds: Double, cpu: Double)] = []
    private var lastSample: Date?
    public private(set) var lastBusy: Date?
    public init() {}

    /// Records the app's per-process CPU times (empty when it isn't running). Returns whether it's busy.
    public mutating func record(_ cpu: [Int32: Double], at time: Date) -> Bool {
        defer { lastSample = time; previous = cpu }
        guard !cpu.isEmpty, let last = lastSample, !previous.isEmpty else {
            intervals.removeAll()
            return false
        }
        // Processes that started since the last sample count from zero.
        let used = cpu.reduce(0.0) { total, entry in total + max(0, entry.value - (previous[entry.key] ?? 0)) }
        intervals.append((time, max(0.001, time.timeIntervalSince(last)), used))
        intervals.removeAll { time.timeIntervalSince($0.end) >= ActivityPolicy.window }
        let seconds = intervals.reduce(0) { $0 + $1.seconds }
        let busy = seconds > 0 && intervals.reduce(0) { $0 + $1.cpu } / seconds >= ActivityPolicy.busyShare
        if busy { lastBusy = time }
        return busy
    }
}
