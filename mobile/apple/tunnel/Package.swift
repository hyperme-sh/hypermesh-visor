// swift-tools-version:5.9
import PackageDescription
let package = Package(
    name: "VisorMeshTunnel",
    platforms: [.iOS(.v15), .macOS(.v12)],
    products: [.library(name: "VisorMeshTunnel", targets: ["VisorMeshTunnel"])],
    dependencies: [.package(path: "Vendor/wireguard-apple")],
    targets: [.target(name: "VisorMeshTunnel", dependencies: [.product(name: "WireGuardKit", package: "wireguard-apple")])]
)
