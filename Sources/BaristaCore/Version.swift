import Foundation

public enum BaristaVersion {
    /// The one place the version lives. scripts/build-app.sh stamps it into the app's Info.plist,
    /// and the Release workflow publishes a GitHub release whenever it changes on main.
    public static let current = "0.0.15"
    /// Where people can support Barista: a GitHub star, or a coffee.
    public static let repoPage = URL(string: "https://github.com/cb-117/barista")!
    public static let supportPage = URL(string: "https://buymeacoffee.com/baristatime")!
}
