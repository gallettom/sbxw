// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "SbxwIsland",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "SbxwIsland",
            path: "Sources/SbxwIsland"
        )
    ]
)
