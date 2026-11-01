import CryptoKit
import Foundation

/// A newer release published on GitHub.
public struct AvailableUpdate: Equatable, Sendable {
    public let version: String
    /// The release page, always available.
    public let pageURL: URL
    /// What a person should download by hand: the disk image, else the zip.
    public let downloadURL: URL?
    /// The zip and its Ed25519 signature, for installing in place. Both or neither.
    public let archiveURL: URL?
    public let signatureURL: URL?
    public var canInstallInPlace: Bool { archiveURL != nil && signatureURL != nil }
}

public enum UpdateCheck {
    public static let latestReleaseURL = URL(string: "https://api.github.com/repos/cb-117/barista/releases/latest")!
    public static let releasesPage = URL(string: "https://github.com/cb-117/barista/releases/latest")!

    /// Parses GitHub's "latest release" JSON. Returns nil unless that release is newer than `current`.
    public static func newerRelease(in data: Data, than current: String = BaristaVersion.current) -> AvailableUpdate? {
        guard let release = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let tag = release["tag_name"] as? String,
              release["draft"] as? Bool != true, release["prerelease"] as? Bool != true,
              isVersion(tag, newerThan: current) else { return nil }
        let version = normalized(tag)
        let page = (release["html_url"] as? String).flatMap(URL.init(string:)).flatMap(trusted) ?? releasesPage
        var assets: [String: URL] = [:]
        for asset in release["assets"] as? [[String: Any]] ?? [] {
            if let name = asset["name"] as? String,
               let url = (asset["browser_download_url"] as? String).flatMap(URL.init(string:)).flatMap(trusted) {
                assets[name] = url
            }
        }
        let archive = assets["Barista-\(version).zip"]
        let signature = assets["Barista-\(version).zip.sig"]
        let paired = archive != nil && signature != nil
        return AvailableUpdate(version: version, pageURL: page,
                               downloadURL: assets["Barista-\(version).dmg"] ?? archive,
                               archiveURL: paired ? archive : nil, signatureURL: paired ? signature : nil)
    }

    /// Compares dotted numeric versions ("v1.2" == "1.2.0"). Anything unparseable is never newer.
    public static func isVersion(_ candidate: String, newerThan current: String) -> Bool {
        guard let a = components(candidate), let b = components(current) else { return false }
        for index in 0..<max(a.count, b.count) {
            let left = index < a.count ? a[index] : 0
            let right = index < b.count ? b[index] : 0
            if left != right { return left > right }
        }
        return false
    }

    static func normalized(_ tag: String) -> String {
        tag.hasPrefix("v") || tag.hasPrefix("V") ? String(tag.dropFirst()) : tag
    }
    static func components(_ version: String) -> [Int]? {
        let parts = normalized(version.trimmingCharacters(in: .whitespaces)).split(separator: ".", omittingEmptySubsequences: false)
        guard (1...4).contains(parts.count) else { return nil }
        let numbers = parts.compactMap { part in part.allSatisfy(\.isASCII) ? Int(part) : nil }
        return numbers.count == parts.count && numbers.allSatisfy { $0 >= 0 } ? numbers : nil
    }
    /// Only ever open or download GitHub URLs from release data.
    static func trusted(_ url: URL) -> URL? {
        guard url.scheme == "https", let host = url.host?.lowercased(),
              host == "github.com" || host.hasSuffix(".github.com") || host == "objects.githubusercontent.com" else { return nil }
        return url
    }
}

/// Barista installs an update only if its archive is signed by the release key.
/// The private half lives in the BARISTA_UPDATE_KEY Actions secret (see scripts/update-signing.swift).
public enum UpdateSignature {
    public static let publicKey = "UY068deVTJyG1/q5vTsjS8OX09wXI9jiJzE+B6sRaz0="

    /// `signature` is the base64 text of a `.sig` file.
    public static func isValid(signature: Data, for archive: Data, publicKey: String = publicKey) -> Bool {
        guard let text = String(data: signature, encoding: .utf8),
              let raw = Data(base64Encoded: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              let keyData = Data(base64Encoded: publicKey),
              let key = try? Curve25519.Signing.PublicKey(rawRepresentation: keyData) else { return false }
        return key.isValidSignature(raw, for: archive)
    }
}
