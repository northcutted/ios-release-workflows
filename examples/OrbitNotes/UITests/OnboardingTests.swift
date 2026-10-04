import XCTest

final class OnboardingTests: XCTestCase {
    func testLaunch() {
        let app = XCUIApplication()
        app.launch()
        XCTAssertTrue(app.navigationBars["Orbit Notes"].waitForExistence(timeout: 15))
    }
    func testStoreScreenshot() throws {
        let app = XCUIApplication()
        IosReleaseSnapshot.launch(app)
        XCTAssertTrue(app.navigationBars["Orbit Notes"].waitForExistence(timeout: 15))
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "01-notes"
        attachment.lifetime = .keepAlways
        add(attachment)
        try IosReleaseSnapshot.capture("01-notes", app: app)
    }
}
