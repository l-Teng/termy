import XCTest
@testable import TermySwift

final class TerminalRenderPlanCacheTests: XCTestCase {
    private let config = TerminalRenderConfig.default

    private func frame(_ text: String, cols: Int = 3, rows: Int = 3) -> TerminalFrame {
        TerminalFrame.plainTextPreview(text, cols: cols, rows: rows)
    }

    func testInitialUpdateIsFullRebuild() {
        let cache = TerminalRenderPlanCache()
        cache.update(frame: frame("aaa\nbbb\nccc"), renderConfig: config, damage: .partial([]))

        XCTAssertTrue(cache.stats.wasFullRebuild)
        XCTAssertEqual(cache.stats.rebuiltRowCount, 3)
        XCTAssertEqual(cache.stats.totalRowCount, 3)
    }

    func testPartialRebuildTouchesOnlyDamagedRows() {
        let cache = TerminalRenderPlanCache()
        cache.update(frame: frame("aaa\nbbb\nccc"), renderConfig: config, damage: .full)

        cache.update(
            frame: frame("aaa\nxyz\nccc"),
            renderConfig: config,
            damage: .partial([TerminalDirtySpan(row: 1, leftCol: 0, rightCol: 2)])
        )

        XCTAssertFalse(cache.stats.wasFullRebuild)
        XCTAssertEqual(cache.stats.rebuiltRowCount, 1)
    }

    /// A partial rebuild (reused rows + the one rebuilt row) must produce exactly
    /// the same flattened plan as a full rebuild of the new frame.
    func testPartialRebuildMatchesFullRebuild() {
        let changed = frame("aaa\nxyz\nccc")

        let partialCache = TerminalRenderPlanCache()
        partialCache.update(frame: frame("aaa\nbbb\nccc"), renderConfig: config, damage: .full)
        partialCache.update(
            frame: changed,
            renderConfig: config,
            damage: .partial([TerminalDirtySpan(row: 1, leftCol: 0, rightCol: 2)])
        )

        let fullCache = TerminalRenderPlanCache()
        fullCache.update(frame: changed, renderConfig: config, damage: .full)

        XCTAssertEqual(partialCache.plan, fullCache.plan)
    }

    func testTextSegmentsCarryStableLineCacheKeys() {
        let cache = TerminalRenderPlanCache()
        cache.update(frame: frame("aaa\nbbb\nccc"), renderConfig: config, damage: .full)

        let firstSegment = cache.plan.rows[0].textSegments.first

        cache.update(
            frame: frame("aaa\nbbb\nccc"),
            renderConfig: config,
            damage: .partial([TerminalDirtySpan(row: 0, leftCol: 0, rightCol: 2)])
        )

        XCTAssertEqual(cache.plan.rows[0].textSegments.first?.lineCacheKey, firstSegment?.lineCacheKey)
    }

    func testTextLineCacheKeysIncludeStyleColorAndBoundedCost() {
        let plain = TextLineCacheKey(bold: false, foregroundPackedValue: 1, text: "abc")
        let bold = TextLineCacheKey(bold: true, foregroundPackedValue: 1, text: "abc")
        let italic = TextLineCacheKey(
            bold: false,
            italic: true,
            foregroundPackedValue: 1,
            text: "abc"
        )
        let underlined = TextLineCacheKey(
            bold: false,
            underline: true,
            foregroundPackedValue: 1,
            text: "abc"
        )
        let struck = TextLineCacheKey(
            bold: false,
            strikethrough: true,
            foregroundPackedValue: 1,
            text: "abc"
        )
        let colored = TextLineCacheKey(bold: false, foregroundPackedValue: 2, text: "abc")
        let longer = TextLineCacheKey(bold: false, foregroundPackedValue: 1, text: String(repeating: "x", count: 200))

        XCTAssertNotEqual(plain, bold)
        XCTAssertNotEqual(plain, italic)
        XCTAssertNotEqual(plain, underlined)
        XCTAssertNotEqual(plain, struck)
        XCTAssertNotEqual(plain, colored)
        XCTAssertGreaterThanOrEqual(plain.estimatedCost, 64)
        XCTAssertGreaterThan(longer.estimatedCost, plain.estimatedCost)
    }

    func testTextSegmentsPreserveSGRAttributes() {
        let cells = [
            TerminalCell(
                col: 0,
                row: 0,
                character: "i",
                foreground: .termyForeground,
                background: .termyBackground,
                usesTerminalDefaultBackground: true,
                renderText: true,
                bold: false,
                italic: true
            ),
            TerminalCell(
                col: 1,
                row: 0,
                character: "u",
                foreground: .termyForeground,
                background: .termyBackground,
                usesTerminalDefaultBackground: true,
                renderText: true,
                bold: false,
                underline: true
            ),
            TerminalCell(
                col: 2,
                row: 0,
                character: "s",
                foreground: .termyForeground,
                background: .termyBackground,
                usesTerminalDefaultBackground: true,
                renderText: true,
                bold: false,
                strikethrough: true
            )
        ]
        let styledFrame = TerminalFrame(
            cols: 3,
            rows: 1,
            cells: cells,
            cursor: nil,
            displayOffset: 0,
            historySize: 0
        )
        let cache = TerminalRenderPlanCache()

        cache.update(frame: styledFrame, renderConfig: config, damage: .full)

        let segments = cache.plan.textSegments
        XCTAssertEqual(segments.count, 3)
        XCTAssertTrue(segments[0].italic)
        XCTAssertTrue(segments[1].underline)
        XCTAssertTrue(segments[2].strikethrough)
    }

    func testTextSegmentsTrackCellSpanForDirtyRectClipping() {
        let cache = TerminalRenderPlanCache()
        cache.update(frame: frame("abc", cols: 3, rows: 1), renderConfig: config, damage: .full)

        let segment = cache.plan.textSegments.first
        XCTAssertEqual(segment?.startCol, 0)
        XCTAssertEqual(segment?.cols, 3)
        XCTAssertEqual(segment?.text, "abc")
    }

    func testConfigChangeForcesFullRebuildDespitePartialDamage() {
        let cache = TerminalRenderPlanCache()
        cache.update(frame: frame("aaa\nbbb\nccc"), renderConfig: config, damage: .full)

        var biggerFont = config
        biggerFont.fontSize = config.fontSize + 4

        cache.update(
            frame: frame("aaa\nbbb\nccc"),
            renderConfig: biggerFont,
            damage: .partial([TerminalDirtySpan(row: 0, leftCol: 0, rightCol: 2)])
        )

        XCTAssertTrue(cache.stats.wasFullRebuild)
        XCTAssertEqual(cache.stats.rebuiltRowCount, 3)
    }

    func testDimensionChangeForcesFullRebuild() {
        let cache = TerminalRenderPlanCache()
        cache.update(frame: frame("aaa\nbbb\nccc", cols: 3, rows: 3), renderConfig: config, damage: .full)

        cache.update(
            frame: frame("aaaa\nbbbb", cols: 4, rows: 2),
            renderConfig: config,
            damage: .partial([TerminalDirtySpan(row: 0, leftCol: 0, rightCol: 3)])
        )

        XCTAssertTrue(cache.stats.wasFullRebuild)
        XCTAssertEqual(cache.stats.totalRowCount, 2)
    }
}
