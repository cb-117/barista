import Foundation

public enum BaristaVersion {
    /// The one place the version lives. scripts/build-app.sh stamps it into the app's Info.plist,
    /// and the Release workflow publishes a GitHub release whenever it changes on main.
    public static let current = "0.0.2"
}

/// A newer release published on GitHub.
public struct AvailableUpdate: Equatable, Sendable {
    public let version: String
    public let pageURL: URL
    public let downloadURL: URL?
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
        let page = (release["html_url"] as? String).flatMap(URL.init(string:)).flatMap(trusted) ?? releasesPage
        let assets = release["assets"] as? [[String: Any]] ?? []
        let zip = assets.compactMap { $0["browser_download_url"] as? String }
            .first { $0.hasSuffix(".zip") }
            .flatMap(URL.init(string:)).flatMap(trusted)
        return AvailableUpdate(version: normalized(tag), pageURL: page, downloadURL: zip)
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
    /// Only ever open GitHub pages from release data.
    static func trusted(_ url: URL) -> URL? {
        guard url.scheme == "https", let host = url.host?.lowercased(),
              host == "github.com" || host.hasSuffix(".github.com") || host == "objects.githubusercontent.com" else { return nil }
        return url
    }
}
