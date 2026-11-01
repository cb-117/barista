import Foundation

public enum BaristaVersion {
    /// The one place the version lives. scripts/build-app.sh stamps it into the app's Info.plist,
    /// and the Release workflow publishes a GitHub release whenever it changes on main.
    public static let current = "0.0.7"
}
