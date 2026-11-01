import Foundation

public enum CustomDuration {
    public static let minutes = 1...1440

    /// Parses typed custom minutes, returning nil for anything that isn't a whole number in range.
    public static func parse(_ text: String) -> Int? {
        let digits = text.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: ",", with: "")
        guard let value = Int(digits), minutes.contains(value) else { return nil }
        return value
    }
}
