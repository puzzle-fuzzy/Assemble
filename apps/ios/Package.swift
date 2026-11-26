// swift-tools-version:6.0
// Assemble 手机应用的核心：线上的类型、重复规则的推算、通知的规划、接口与登录的保存。
// 不依赖界面，所以在 Mac 上就能测（`swift test`）；应用壳只是把它装进 SwiftUI。
import PackageDescription

let package = Package(
    name: "AssembleCore",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "AssembleCore", targets: ["AssembleCore"])
    ],
    targets: [
        .target(name: "AssembleCore"),
        // 样本在包根目录的 Fixtures/ 里，由 Rust 侧的测试生成；测试按相对路径读它。
        .testTarget(
            name: "AssembleCoreTests",
            dependencies: ["AssembleCore"]
        )
    ]
)
