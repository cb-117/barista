import Carbon
import BaristaCore
import Foundation

@MainActor
final class GlobalHotKey {
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let action: () -> Void
    init(action: @escaping () -> Void) { self.action = action }

    func register(choice: String) throws {
        unregister()
        var event = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let result = InstallEventHandler(GetApplicationEventTarget(), { _, _, context in
            guard let context else { return OSStatus(eventNotHandledErr) }
            MainActor.assumeIsolated {
                Unmanaged<GlobalHotKey>.fromOpaque(context).takeUnretainedValue().action()
            }
            return noErr
        }, 1, &event, Unmanaged.passUnretained(self).toOpaque(), &handler)
        guard result == noErr else { throw CoffeeError.message("Couldn’t install the keyboard shortcut.") }
        let modifiers: UInt32
        switch choice {
        case "controlOption": modifiers = UInt32(controlKey | optionKey)
        case "controlShiftCommand": modifiers = UInt32(controlKey | shiftKey | cmdKey)
        default: modifiers = UInt32(controlKey | optionKey | cmdKey)
        }
        let identifier = EventHotKeyID(signature: 0x43414645, id: 1)
        let status = RegisterEventHotKey(UInt32(kVK_ANSI_C), modifiers, identifier, GetApplicationEventTarget(), 0, &hotKey)
        guard status == noErr else {
            unregister()
            throw CoffeeError.message("That shortcut is in use. Choose another shortcut in Settings.")
        }
    }
    func unregister() {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        if let handler { RemoveEventHandler(handler) }
        hotKey = nil
        handler = nil
    }
}
