import XCTest
@testable import TermySwift

/// Covers the Swift keyCode → terminal-key-name mapping (the event→FFI input
/// path) that hands off to the Rust encoder.
final class TerminalKeyInputMappingTests: XCTestCase {
    func testOptionAsAltEncodesOptionSpaceAsEscapePrefixedSpace() throws {
        let terminal = try LibTermyTerminal(displayCols: 80, rows: 24, loadUserConfig: false)
        let bytes = try terminal.encodeKey(
            TerminalKeyInput(key: "space", keyChar: "\u{a0}", alt: true),
            macosOptionAsAlt: true
        )

        XCTAssertEqual(bytes, Array("\u{1b} ".utf8))
    }

    func testSpecialKeysMapToNames() {
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 36)?.key, "enter")
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 48)?.key, "tab")
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 51)?.key, "backspace")
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 53)?.key, "escape")
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 123)?.key, "left")
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 124)?.key, "right")
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 125)?.key, "down")
        XCTAssertEqual(KeyboardCaptureView.specialKey(for: 126)?.key, "up")
    }

    func testFunctionKeysCarryFunctionFlag() {
        let f1 = KeyboardCaptureView.specialKey(for: 122)
        XCTAssertEqual(f1?.key, "f1")
        XCTAssertEqual(f1?.function, true)
        XCTAssertEqual(f1?.usesCharacter, false)
    }

    func testSpaceCarriesTypedCharacter() {
        let space = KeyboardCaptureView.specialKey(for: 49)
        XCTAssertEqual(space?.key, "space")
        XCTAssertEqual(space?.usesCharacter, true)
    }

    func testNonSpecialKeyFallsThroughToCharacter() {
        // keyCode 0 ('a' on US layouts) is not in the special table.
        XCTAssertNil(KeyboardCaptureView.specialKey(for: 0))
    }

    func testNavigationKeysBypassInputContextWhenNotComposing() {
        XCTAssertFalse(KeyboardCaptureView.shouldRouteThroughInputContext(
            keyCode: 125,
            modifierFlags: [],
            hasMarkedText: false
        ))
        XCTAssertFalse(KeyboardCaptureView.shouldRouteThroughInputContext(
            keyCode: 126,
            modifierFlags: [],
            hasMarkedText: false
        ))
    }

    func testNavigationKeysStillRouteToInputContextDuringComposition() {
        XCTAssertTrue(KeyboardCaptureView.shouldRouteThroughInputContext(
            keyCode: 125,
            modifierFlags: [],
            hasMarkedText: true
        ))
    }

    func testTextKeysStillRouteToInputContext() {
        XCTAssertTrue(KeyboardCaptureView.shouldRouteThroughInputContext(
            keyCode: 0,
            modifierFlags: [],
            hasMarkedText: false
        ))
        XCTAssertTrue(KeyboardCaptureView.shouldRouteThroughInputContext(
            keyCode: 49,
            modifierFlags: [],
            hasMarkedText: false
        ))
    }
}
