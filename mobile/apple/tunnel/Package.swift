// swift-tools-version:5.9
import PackageDescription
let package = Package(
    name: "VisorMeshTunnel",
    platforms: [.iOS(.v15), .macOS(.v12)],
    products: [.library(name: "VisorMeshTunnel", targets: ["VisorMeshTunnel"])],
    dependencies: [.package(url: "https://git.zx2c4.com/wireguard-apple", revision: "2fec12a6e1f6e3460b6ee483aa00ad29cddadab1")],
    targets: [.target(name: "VisorMeshTunnel", dependencies: [.product(name: "WireGuardKit", package: "wireguard-apple")])]
)
