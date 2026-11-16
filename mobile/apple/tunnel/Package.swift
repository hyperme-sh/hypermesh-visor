// swift-tools-version:5.9
import PackageDescription
let package = Package(
    name: "VisorMeshTunnel",
    platforms: [.iOS(.v15), .macOS(.v12)],
    products: [.library(name: "VisorMeshTunnel", targets: ["VisorMeshTunnel"])],
    dependencies: [.package(url: "https://git.zx2c4.com/wireguard-apple", revision: "ccc7472fd7d1c7c19584e6a30c45a56b8ba57790")],
    targets: [.target(name: "VisorMeshTunnel", dependencies: [.product(name: "WireGuardKit", package: "wireguard-apple")])]
)
