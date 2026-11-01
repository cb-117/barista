import Foundation
import IOKit.pwr_mgt

@MainActor
public protocol PowerAsserting: AnyObject {
    func activate(keepDisplayAwake: Bool) throws
    func deactivate()
}

/// Assertions belong to this process, so macOS also releases them if the app exits.
@MainActor
public final class PowerAssertion: PowerAsserting {
    private var assertions: [IOPMAssertionID] = []

    public init() {}

    deinit {
        assertions.forEach { _ = IOPMAssertionRelease($0) }
    }

    public func activate(keepDisplayAwake: Bool) throws {
        var replacement: [IOPMAssertionID] = []
        var types = [kIOPMAssertionTypePreventUserIdleSystemSleep]
        if keepDisplayAwake {
            types.append(kIOPMAssertionTypePreventUserIdleDisplaySleep)
        }

        for type in types {
            var identifier = IOPMAssertionID(0)
            let result = IOPMAssertionCreateWithName(
                type as CFString,
                IOPMAssertionLevel(kIOPMAssertionLevelOn),
                "Barista awake session" as CFString,
                &identifier
            )
            guard result == kIOReturnSuccess else {
                replacement.forEach { _ = IOPMAssertionRelease($0) }
                throw AssertionError.creationFailed(result)
            }
            replacement.append(identifier)
        }

        deactivate()
        assertions = replacement
    }

    public func deactivate() {
        assertions.forEach { _ = IOPMAssertionRelease($0) }
        assertions.removeAll()
    }

    private enum AssertionError: LocalizedError {
        case creationFailed(IOReturn)

        var errorDescription: String? {
            switch self {
            case .creationFailed(let code):
                "macOS couldn’t start the awake session (\(code)). Please try again."
            }
        }
    }
}
