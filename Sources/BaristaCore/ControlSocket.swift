import Darwin
import Foundation

public enum ControlEndpoint {
    public static var directory: String { "/tmp/com.connorbescos.barista-\(getuid())" }
    public static var path: String { directory + "/control.sock" }
}

/// Single-request Unix sockets. No TCP listener, web server, or network permission.
public struct ControlClient: Sendable {
    public let path: String
    public init(path: String = ControlEndpoint.path) { self.path = path }

    public func send(_ request: ControlRequest) throws -> ControlReply {
        let fd = try UnixWire.connect(path: path)
        defer { Darwin.close(fd) }
        try UnixWire.write(try CoffeeJSON.encoder().encode(request), to: fd)
        return try CoffeeJSON.decoder().decode(ControlReply.self, from: UnixWire.read(from: fd, limit: 262144))
    }
}

@MainActor
public final class ControlServer {
    private var source: DispatchSourceRead?
    private var boundPath: String?
    public init() {}

    public func start(path: String = ControlEndpoint.path,
                      handler: @escaping @Sendable (ControlRequest) async -> ControlReply) throws {
        guard source == nil else { return }
        // A client that disconnects mid-reply must never kill the host process.
        signal(SIGPIPE, SIG_IGN)
        let directory = (path as NSString).deletingLastPathComponent
        if mkdir(directory, 0o700) != 0 && errno != EEXIST { throw UnixWire.lastError() }
        var directoryInfo = stat()
        guard lstat(directory, &directoryInfo) == 0,
              directoryInfo.st_mode & S_IFMT == S_IFDIR,
              directoryInfo.st_uid == getuid(), directoryInfo.st_mode & 0o077 == 0 else {
            throw CoffeeError.message("The control directory must be a private, user-owned directory (mode 700).")
        }
        var info = stat()
        if lstat(path, &info) == 0 {
            guard info.st_mode & S_IFMT == S_IFSOCK, info.st_uid == getuid() else {
                throw CoffeeError.message("Refusing to replace an unexpected file at the control socket path.")
            }
            do {
                let existing = try UnixWire.connect(path: path)
                Darwin.close(existing)
                throw CoffeeError.message("Another Barista instance is already serving this account.")
            } catch let error as POSIXError where error.code == .ECONNREFUSED || error.code == .ENOENT {
                guard unlink(path) == 0 || errno == ENOENT else { throw UnixWire.lastError() }
            }
        }
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw UnixWire.lastError() }
        var didBind = false
        do {
            var address = try UnixWire.address(path)
            let result = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else { throw UnixWire.lastError() }
            didBind = true
            guard chmod(path, 0o600) == 0, listen(fd, 16) == 0,
                  fcntl(fd, F_SETFL, O_NONBLOCK) == 0,
                  fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else { throw UnixWire.lastError() }
            let source = DispatchSource.makeReadSource(fileDescriptor: fd, queue: DispatchQueue.global(qos: .utility))
            source.setEventHandler { @Sendable in
                while true {
                    let peer = accept(fd, nil, nil)
                    if peer < 0 { break }
                    DispatchQueue.global(qos: .utility).async {
                        do {
                            try UnixWire.configure(peer)
                            try UnixWire.checkPeer(peer)
                            let data = try UnixWire.read(from: peer, limit: 65536)
                            let request = try CoffeeJSON.decoder().decode(ControlRequest.self, from: data)
                            Task {
                                let reply = await handler(request)
                                if let data = try? CoffeeJSON.encoder().encode(reply) { try? UnixWire.write(data, to: peer) }
                                Darwin.close(peer)
                            }
                        } catch {
                            let reply = ControlReply(ok: false, error: "Invalid control request: \(error.localizedDescription)")
                            if let data = try? CoffeeJSON.encoder().encode(reply) { try? UnixWire.write(data, to: peer) }
                            Darwin.close(peer)
                        }
                    }
                }
            }
            source.setCancelHandler { @Sendable in Darwin.close(fd) }
            boundPath = path
            self.source = source
            source.resume()
        } catch {
            Darwin.close(fd)
            // Only remove a socket successfully bound by this instance.
            if didBind { unlink(path) }
            throw error
        }
    }

    public func stop() {
        // Unlink synchronously; a queued cancellation must never unlink a newer listener.
        if let boundPath { unlink(boundPath) }
        boundPath = nil
        source?.cancel()
        source = nil
    }
}

private enum UnixWire {
    static func lastError() -> POSIXError { POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    static func address(_ path: String) throws -> sockaddr_un {
        var address = sockaddr_un()
        let bytes = Array(path.utf8) + [0]
        guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw CoffeeError.message("Control socket path is too long.") }
        address.sun_family = sa_family_t(AF_UNIX)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        withUnsafeMutableBytes(of: &address.sun_path) { target in target.copyBytes(from: bytes) }
        return address
    }
    static func configure(_ fd: Int32) throws {
        // Darwin may inherit O_NONBLOCK on accept; workers use bounded blocking I/O.
        let flags = fcntl(fd, F_GETFL, 0)
        guard flags >= 0, fcntl(fd, F_SETFL, flags & ~O_NONBLOCK) == 0 else { throw lastError() }
        var timeout = timeval(tv_sec: 3, tv_usec: 0)
        var noSignal: Int32 = 1
        guard setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size)) == 0,
              setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size)) == 0,
              setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &noSignal, socklen_t(MemoryLayout<Int32>.size)) == 0 else { throw lastError() }
        _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
    }
    static func checkPeer(_ fd: Int32) throws {
        var user = uid_t(0), group = gid_t(0)
        guard getpeereid(fd, &user, &group) == 0, user == getuid() else {
            throw CoffeeError.message("Only processes belonging to your Mac account can use Barista.")
        }
    }
    static func connect(path: String) throws -> Int32 {
        let fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw lastError() }
        do {
            try configure(fd)
            var address = try address(path)
            let result = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else { throw lastError() }
            try checkPeer(fd)
            return fd
        } catch { Darwin.close(fd); throw error }
    }
    static func read(from fd: Int32, limit: Int) throws -> Data {
        var data = Data()
        var bytes = [UInt8](repeating: 0, count: 4096)
        while data.count <= limit {
            let count = Darwin.recv(fd, &bytes, bytes.count, 0)
            if count < 0 {
                if errno == EINTR { continue }
                throw lastError()
            }
            guard count > 0 else { throw CoffeeError.message("Control connection closed before replying.") }
            if let newline = bytes[..<count].firstIndex(of: 10) {
                data.append(contentsOf: bytes[..<newline])
                guard data.count <= limit else { break }
                return data
            }
            data.append(contentsOf: bytes[..<count])
        }
        throw CoffeeError.message("Control message exceeds the size limit.")
    }
    static func write(_ data: Data, to fd: Int32) throws {
        var payload = data
        payload.append(10)
        try payload.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let written = Darwin.send(fd, bytes.baseAddress!.advanced(by: offset), bytes.count - offset, 0)
                if written < 0 {
                    if errno == EINTR { continue }
                    throw lastError()
                }
                guard written > 0 else { throw CoffeeError.message("Couldn’t write to the control connection.") }
                offset += written
            }
        }
    }
}
