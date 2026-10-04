import XCTest
@testable import OrbitNotes

final class NotesTests: XCTestCase {
    func testSingularTitle() { XCTAssertEqual(Notes.title(for: 1), "1 note") }
    func testPluralTitle() { XCTAssertEqual(Notes.title(for: 3), "3 notes") }
}
