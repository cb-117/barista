// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Barista",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "BaristaApp", targets: ["Barista"]),
        .executable(name: "barista", targets: ["BaristaCLI"]),
        .executable(name: "BaristaLauncher", targets: ["BaristaLauncher"])
    ],
    targets: [
        .target(name: "BaristaCore"),
        .executableTarget(name: "Barista", dependencies: ["BaristaCore"]),
        .executableTarget(name: "BaristaCLI", dependencies: ["BaristaCore"]),
        .executableTarget(name: "BaristaLauncher"),
        .executableTarget(name: "BaristaChecks", dependencies: ["BaristaCore"], path: "Tests/BaristaTests")
    ]
)
