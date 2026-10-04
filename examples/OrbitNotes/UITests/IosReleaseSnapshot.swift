import XCTest
import UIKit

// App-owned scenarios call these helpers; no Fastlane runtime is required.
enum IosReleaseSnapshot {
    static var cache: URL? {
        ProcessInfo.processInfo.environment["IOS_RELEASE_SNAPSHOT_HOME"].map {
            URL(fileURLWithPath: $0).appendingPathComponent("Library/Caches/tools.fastlane")
        }
    }
    static func launch(_ app: XCUIApplication) {
        if let cache, let language = try? String(contentsOf: cache.appendingPathComponent("language.txt"), encoding: .utf8) {
            app.launchArguments += ["-AppleLanguages", "(\(language))", "-AppleLocale", language]
        }
        app.launch()
    }
    static func capture(_ name: String, app: XCUIApplication) throws {
        guard let cache, let device = ProcessInfo.processInfo.environment["IOS_RELEASE_SNAPSHOT_DEVICE"] else { return }
        let folder = cache.appendingPathComponent("screenshots")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let image = app.screenshot().image
        let format = UIGraphicsImageRendererFormat()
        format.opaque = true
        format.scale = 1
        let size = CGSize(width: image.size.width * image.scale, height: image.size.height * image.scale)
        let rendered = UIGraphicsImageRenderer(size: size, format: format).image { _ in image.draw(in: CGRect(origin: .zero, size: size)) }
        try XCTUnwrap(rendered.pngData()).write(to: folder.appendingPathComponent("\(device)-\(name).png"), options: .atomic)
    }
}
