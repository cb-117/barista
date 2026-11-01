// Ed25519 signing for Barista updates. The app only installs archives signed by this key.
//   swift scripts/update-signing.swift generate     → prints a new private key and its public key
//   BARISTA_UPDATE_KEY=… swift scripts/update-signing.swift sign FILE   → writes FILE.sig
// Keep the private key out of the repo: it lives in the BARISTA_UPDATE_KEY Actions secret.
import CryptoKit
import Foundation

func fail(_ message: String) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(1)
}

let arguments = Array(CommandLine.arguments.dropFirst())
switch arguments.first {
case "generate":
    let key = Curve25519.Signing.PrivateKey()
    print("private: \(key.rawRepresentation.base64EncodedString())")
    print("public:  \(key.publicKey.rawRepresentation.base64EncodedString())")
case "sign" where arguments.count == 2:
    guard let encoded = ProcessInfo.processInfo.environment["BARISTA_UPDATE_KEY"], !encoded.isEmpty else {
        fail("Set BARISTA_UPDATE_KEY to the base64 private key.")
    }
    guard let raw = Data(base64Encoded: encoded.trimmingCharacters(in: .whitespacesAndNewlines)),
          let key = try? Curve25519.Signing.PrivateKey(rawRepresentation: raw) else { fail("BARISTA_UPDATE_KEY isn't a valid key.") }
    let file = URL(fileURLWithPath: arguments[1])
    guard let data = try? Data(contentsOf: file) else { fail("Can't read \(file.path).") }
    let signature = try key.signature(for: data)
    guard key.publicKey.isValidSignature(signature, for: data) else { fail("Signature didn't verify.") }
    try Data((signature.base64EncodedString() + "\n").utf8).write(to: file.appendingPathExtension("sig"))
    print("Signed \(file.lastPathComponent) with public key \(key.publicKey.rawRepresentation.base64EncodedString())")
default:
    fail("Usage: update-signing.swift generate | sign FILE")
}
